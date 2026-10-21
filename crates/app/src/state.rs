//! 应用状态：全部是 windui `Signal`（Copy 句柄），加上几个运行期句柄与后台消息通道。
//!
//! 后台线程只持有 `Sender<Msg>`；所有信号写入都在 UI 线程的 `on_message` 里完成。

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use apkinfo_core::icon::{IconCandidate, RgbaImage};
use apkinfo_core::info::{ApkInfo, SignatureInfo};
use apkinfo_core::load::IconSession;
use windui::app::ThemeHandle;
use windui::i18n::LocaleHandle;
use windui::prelude::*;

use crate::config::Config;

/// 后台线程 → UI 线程的消息。每条都带 `gen`（打开文件的代次），
/// 旧文件的迟到结果按代次丢弃，否则快速连开两个文件时会串台。
pub enum Msg {
    Parsed {
        gen: u64,
        result: Result<Box<apkinfo_core::load::Loaded>, String>,
    },
    Icon {
        gen: u64,
        index: usize,
        result: Result<RgbaImage, String>,
    },
    Hashes {
        gen: u64,
        result: Result<(String, String), String>,
    },
    Signature {
        gen: u64,
        result: Result<SignatureInfo, String>,
    },
    /// 第二个实例把要打开的文件转过来。
    OpenPath(PathBuf),
    /// 图标导出结束。
    Exported(Result<PathBuf, String>),
    Devices(Result<Vec<apkinfo_core::adb::AdbDevice>, String>),
    Install(apkinfo_core::installer::InstallEvent),
    Download(apkinfo_core::tools::DownloadEvent),
    /// 窗口级快捷键（`App::on_shortcut` 拿不到 ctx，转成消息执行）。
    Shortcut(crate::Shortcut),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashState {
    Idle,
    Computing,
    Done,
    Failed,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigState {
    Idle,
    Computing,
    Done,
    Failed,
    Disabled,
    NotApplicable,
}

#[derive(Clone)]
pub struct AppState {
    // ---- 文件与解析 ----
    pub file_path: Signal<Option<PathBuf>>,
    pub file_size: Signal<u64>,
    /// 0 或 1 个元素：空表示没有打开文件（给 `host_signal` 用）。
    pub info: Signal<Vec<Rc<ApkInfo>>>,
    pub parsing: Signal<bool>,
    pub parse_error: Signal<String>,
    /// 文件打开的代次，见 [`Msg`]。
    pub generation: Rc<Cell<u64>>,
    pub session: Rc<RefCell<Option<Arc<Mutex<IconSession>>>>>,

    // ---- 图标 ----
    pub icon: Signal<Vec<Rc<RgbaImage>>>,
    pub icon_candidates: Signal<Vec<IconCandidate>>,
    pub icon_index: Signal<usize>,
    pub icon_loading: Signal<bool>,

    // ---- 应用名语言 ----
    pub label_locale: Signal<Option<String>>,

    // ---- 哈希 / 签名 ----
    pub md5: Signal<String>,
    pub sha1: Signal<String>,
    pub hash_state: Signal<HashState>,
    pub hash_error: Signal<String>,
    pub signature: Signal<Vec<Rc<SignatureInfo>>>,
    pub sig_state: Signal<SigState>,
    pub sig_error: Signal<String>,

    // ---- 界面 ----
    pub tab: Signal<usize>,
    pub show_install: Signal<bool>,
    pub show_rename: Signal<bool>,
    pub window_title: Signal<String>,
    pub title_width: Signal<i32>,
    pub permission_filter: Signal<String>,

    // ---- 设置派生（界面即时跟随）----
    pub enable_hash: Signal<bool>,
    pub enable_signature: Signal<bool>,

    // ---- 对话框 / 子窗口状态 ----
    pub install: crate::ui::install::InstallState,
    pub rename: crate::ui::rename::RenameState,
    pub download: crate::ui::settings::DownloadState,

    // ---- 基础设施 ----
    pub config: Rc<RefCell<Config>>,
    pub tx: Sender<Msg>,
    pub theme: ThemeHandle,
    pub locale: LocaleHandle,
}

impl AppState {
    pub fn new(config: Config, tx: Sender<Msg>, theme: ThemeHandle, locale: LocaleHandle) -> Self {
        let title_width = config.title_width;
        let enable_hash = config.enable_hash;
        let enable_signature = config.enable_signature;
        let install = crate::ui::install::InstallState::new(&config);
        Self {
            install,
            rename: crate::ui::rename::RenameState::new(),
            download: crate::ui::settings::DownloadState::new(),
            file_path: signal(None),
            file_size: signal(0),
            info: signal(Vec::new()),
            parsing: signal(false),
            parse_error: signal(String::new()),
            generation: Rc::new(Cell::new(0)),
            session: Rc::new(RefCell::new(None)),
            icon: signal(Vec::new()),
            icon_candidates: signal(Vec::new()),
            icon_index: signal(0),
            icon_loading: signal(false),
            label_locale: signal(None),
            md5: signal(String::new()),
            sha1: signal(String::new()),
            hash_state: signal(HashState::Idle),
            hash_error: signal(String::new()),
            signature: signal(Vec::new()),
            sig_state: signal(SigState::Idle),
            sig_error: signal(String::new()),
            tab: signal(0),
            show_install: signal(false),
            show_rename: signal(false),
            window_title: signal(String::from("APK Info Plus")),
            title_width: signal(title_width),
            permission_filter: signal(String::new()),
            enable_hash: signal(enable_hash),
            enable_signature: signal(enable_signature),
            config: Rc::new(RefCell::new(config)),
            tx,
            theme,
            locale,
        }
    }

    pub fn current_info(&self) -> Option<Rc<ApkInfo>> {
        self.info.with(|v| v.first().cloned())
    }

    /// 读配置的便捷方法。
    pub fn cfg<R>(&self, f: impl FnOnce(&Config) -> R) -> R {
        f(&self.config.borrow())
    }

    /// 改配置并立即落盘。
    pub fn update_config(&self, f: impl FnOnce(&mut Config)) {
        let mut c = self.config.borrow_mut();
        f(&mut c);
        c.save();
    }

    /// 开启新一代（打开 / 关闭文件时调用），返回新代次。
    pub fn next_generation(&self) -> u64 {
        let g = self.generation.get() + 1;
        self.generation.set(g);
        g
    }

    pub fn is_current(&self, gen: u64) -> bool {
        self.generation.get() == gen
    }

    /// 当前展示的应用名（随语言选择）。
    pub fn display_label(&self) -> String {
        let locale = self.label_locale.get();
        self.current_info()
            .and_then(|i| i.display_label(locale.as_deref()).map(|s| s.to_string()))
            .unwrap_or_default()
    }

    /// 清空与文件相关的全部状态。
    pub fn clear_file_state(&self) {
        self.info.set(Vec::new());
        self.parse_error.set(String::new());
        self.icon.set(Vec::new());
        self.icon_candidates.set(Vec::new());
        self.icon_index.set(0);
        self.icon_loading.set(false);
        self.label_locale.set(None);
        self.md5.set(String::new());
        self.sha1.set(String::new());
        self.hash_state.set(HashState::Idle);
        self.hash_error.set(String::new());
        self.signature.set(Vec::new());
        self.sig_state.set(SigState::Idle);
        self.sig_error.set(String::new());
        self.permission_filter.set(String::new());
        *self.session.borrow_mut() = None;
    }
}
