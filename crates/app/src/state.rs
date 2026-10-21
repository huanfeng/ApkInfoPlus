//! 应用状态：全部是 windui `Signal`（Copy 句柄），加上几个运行期句柄与后台消息通道。
//!
//! 可以同时打开多个文件：每个文件是一个 [`Doc`]，自己持有解析结果、图标、哈希、签名等
//! 信号；[`AppState`] 只管「开着哪些、当前看哪个」。后台线程只持有 `Sender<Msg>`，
//! 所有信号写入都在 UI 线程的 `on_message` 里完成。

use std::cell::{Cell, RefCell};
use std::mem::ManuallyDrop;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use apkinfo_core::icon::{IconCandidate, RgbaImage};
use apkinfo_core::info::{ApkInfo, SignatureInfo};
use apkinfo_core::load::IconSession;
use windui::app::ThemeHandle;
use windui::i18n::LocaleHandle;
use windui::prelude::*;
use windui::signal::SignalScope;

use crate::config::Config;

/// 后台线程 → UI 线程的消息。每条都带 `gen`（该文档本次解析的代次），
/// 按代次找回文档；文档已关闭或已重新解析时迟到的结果直接丢弃，否则会串台。
pub enum Msg {
    Parsed {
        gen: u64,
        result: Result<Box<apkinfo_core::load::Loaded>, String>,
    },
    Icon {
        gen: u64,
        /// 渲染请求序号：快速连点时多个线程乱序返回，只认最新一次。
        token: u64,
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
    /// 第二个实例 / 文件对话框把要打开的文件转过来。
    OpenPaths(Vec<PathBuf>),
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

/// 一个打开的文件。信号全部归 `scope` 所有，关闭文档时整批回收。
pub struct Doc {
    /// 文档 id（首次解析的代次），生命周期内不变。
    pub id: u64,
    /// 当前这次解析的代次；重新解析时换新，旧线程的结果据此丢弃。
    pub gen: Cell<u64>,
    /// 改名后会变，所以是信号。
    pub path: Signal<PathBuf>,
    pub file_size: Signal<u64>,
    /// 0 或 1 个元素：空表示尚未解析完或解析失败（给 `host_signal` 用）。
    pub info: Signal<Vec<Rc<ApkInfo>>>,
    pub parsing: Signal<bool>,
    pub parse_error: Signal<String>,
    /// 侧栏第二行：包名 / 「正在解析…」/ 错误。
    pub subtitle: Signal<String>,
    pub session: RefCell<Option<Arc<Mutex<IconSession>>>>,

    // ---- 图标 ----
    pub icon: Signal<Vec<Rc<RgbaImage>>>,
    pub icon_candidates: Signal<Vec<IconCandidate>>,
    pub icon_index: Signal<usize>,
    pub icon_loading: Signal<bool>,
    /// 最近一次图标渲染请求的序号，见 [`Msg::Icon`]。
    pub icon_req: Cell<u64>,

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

    // ---- 界面（每个文档记自己的，切换回来时还在原来的页）----
    pub tab: Signal<usize>,
    pub permission_filter: Signal<String>,

    /// `ManuallyDrop`：进程退出时信号 arena（线程局部）可能先于状态销毁，那时再 dispose
    /// 会撞 TLS 已析构而 abort；只在 [`Doc::dispose`] 里手动回收。
    scope: RefCell<ManuallyDrop<SignalScope>>,
}

impl Doc {
    pub fn new(id: u64, path: PathBuf) -> Rc<Doc> {
        let mut scope = SignalScope::new();
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let doc = scope.collect(|| Doc {
            id,
            gen: Cell::new(id),
            path: signal(path),
            file_size: signal(size),
            info: signal(Vec::new()),
            parsing: signal(false),
            parse_error: signal(String::new()),
            subtitle: signal(String::new()),
            session: RefCell::new(None),
            icon: signal(Vec::new()),
            icon_candidates: signal(Vec::new()),
            icon_index: signal(0),
            icon_loading: signal(false),
            icon_req: Cell::new(0),
            label_locale: signal(None),
            md5: signal(String::new()),
            sha1: signal(String::new()),
            hash_state: signal(HashState::Idle),
            hash_error: signal(String::new()),
            signature: signal(Vec::new()),
            sig_state: signal(SigState::Idle),
            sig_error: signal(String::new()),
            tab: signal(0),
            permission_filter: signal(String::new()),
            // 占位，下面换成真正收集了信号的那个
            scope: RefCell::new(ManuallyDrop::new(SignalScope::new())),
        });
        *doc.scope.borrow_mut() = ManuallyDrop::new(scope);
        Rc::new(doc)
    }

    pub fn file_name(&self) -> String {
        self.path.with(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
    }

    pub fn current_info(&self) -> Option<Rc<ApkInfo>> {
        self.info.with(|v| v.first().cloned())
    }

    /// 当前展示的应用名（随语言选择）。
    pub fn display_label(&self) -> String {
        let locale = self.label_locale.get();
        self.current_info()
            .and_then(|i| i.display_label(locale.as_deref()).map(|s| s.to_string()))
            .unwrap_or_default()
    }

    /// 重新解析前清掉上一次的结果。
    pub fn reset(&self) {
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
        *self.session.borrow_mut() = None;
    }

    /// 关闭时先放掉大块内存（zip 会话、资源表、位图）；信号槽位稍后再回收。
    pub fn release(&self) {
        self.reset();
        self.subtitle.set(String::new());
    }

    /// 回收全部信号。**只能**在界面上已经没有任何节点引用这些信号之后调用——
    /// 读已回收的信号会 panic，所以关闭文档时先摘下再延后回收（见 `AppState::remove_doc`）。
    pub fn dispose(&self) {
        self.scope.borrow_mut().dispose();
    }
}

/// 侧栏一行的模型（纯数据，文档列表 / 当前项变化时整体重建）。
#[derive(Clone)]
pub struct DocEntry {
    pub doc: Rc<Doc>,
    pub active: bool,
}

#[derive(Clone)]
pub struct AppState {
    // ---- 文档 ----
    /// 打开的全部文档，按打开顺序。
    pub docs: Signal<Vec<Rc<Doc>>>,
    /// 当前文档：0 或 1 个元素（给 `host_signal` 用，切换即重建内容区）。
    pub active: Signal<Vec<Rc<Doc>>>,
    /// 侧栏模型。
    pub entries: Signal<Vec<DocEntry>>,
    pub active_id: Rc<Cell<u64>>,
    /// 已关闭、等界面重建完再回收信号的文档。
    graveyard: Rc<RefCell<Vec<Rc<Doc>>>>,
    /// 解析代次计数器，见 [`Msg`]。
    pub generation: Rc<Cell<u64>>,

    // ---- 界面 ----
    pub show_rename: Signal<bool>,
    pub window_title: Signal<String>,
    pub title_width: Signal<i32>,
    pub sidebar_filter: Signal<String>,
    /// 当前文档换了就加一：侧栏据此把当前那一行滚进视口。
    pub reveal_active: Signal<u64>,
    /// 左侧文件列表占窗口宽度的比例（分隔条可拖，退出时写回配置）。
    pub sidebar_ratio: Signal<f32>,

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

pub const APP_TITLE: &str = "APK Info Plus";

impl AppState {
    pub fn new(config: Config, tx: Sender<Msg>, theme: ThemeHandle, locale: LocaleHandle) -> Self {
        let title_width = config.title_width;
        let sidebar_ratio = config.sidebar_ratio.clamp(0.12, 0.5);
        let enable_hash = config.enable_hash;
        let enable_signature = config.enable_signature;
        let install = crate::ui::install::InstallState::new(&config);
        Self {
            install,
            rename: crate::ui::rename::RenameState::new(),
            download: crate::ui::settings::DownloadState::new(),
            docs: signal(Vec::new()),
            active: signal(Vec::new()),
            entries: signal(Vec::new()),
            active_id: Rc::new(Cell::new(0)),
            graveyard: Rc::new(RefCell::new(Vec::new())),
            generation: Rc::new(Cell::new(0)),
            show_rename: signal(false),
            window_title: signal(String::from(APP_TITLE)),
            title_width: signal(title_width),
            sidebar_filter: signal(String::new()),
            reveal_active: signal(0),
            sidebar_ratio: signal(sidebar_ratio),
            enable_hash: signal(enable_hash),
            enable_signature: signal(enable_signature),
            config: Rc::new(RefCell::new(config)),
            tx,
            theme,
            locale,
        }
    }

    // ---- 文档管理 ----

    pub fn active_doc(&self) -> Option<Rc<Doc>> {
        self.active.with(|v| v.first().cloned())
    }

    pub fn doc_by_id(&self, id: u64) -> Option<Rc<Doc>> {
        self.docs.with(|v| v.iter().find(|d| d.id == id).cloned())
    }

    /// 按解析代次找文档：文档关闭或重新解析后旧代次找不到，迟到的结果就此丢弃。
    pub fn doc_by_gen(&self, gen: u64) -> Option<Rc<Doc>> {
        self.docs
            .with(|v| v.iter().find(|d| d.gen.get() == gen).cloned())
    }

    pub fn doc_by_path(&self, path: &std::path::Path) -> Option<Rc<Doc>> {
        self.docs
            .with(|v| v.iter().find(|d| d.path.with(|p| p == path)).cloned())
    }

    /// 加入列表并设为当前。
    pub fn add_doc(&self, doc: Rc<Doc>) {
        self.reap();
        self.docs.update(|v| v.push(doc.clone()));
        self.set_active(doc.id);
    }

    pub fn set_active(&self, id: u64) {
        let Some(doc) = self.doc_by_id(id) else {
            return;
        };
        if self.active_id.get() != id || self.active.with(|v| v.is_empty()) {
            self.active_id.set(id);
            self.active.set(vec![doc]);
            self.reveal_active.update(|n| *n += 1);
        }
        self.refresh_entries();
    }

    /// 关闭文档：从列表摘下、切到邻近文档、释放大块内存；信号延后到下一次增删时回收。
    pub fn remove_doc(&self, id: u64) {
        let Some(index) = self.docs.with(|v| v.iter().position(|d| d.id == id)) else {
            return;
        };
        self.reap();
        let doc = self.docs.with(|v| v[index].clone());
        self.docs.update(|v| {
            v.remove(index);
        });
        if self.active_id.get() == id {
            let next = self
                .docs
                .with(|v| v.get(index).or_else(|| v.last()).map(|d| d.id));
            match next {
                Some(n) => {
                    self.active_id.set(0);
                    self.set_active(n);
                }
                None => {
                    self.active_id.set(0);
                    self.active.set(Vec::new());
                    self.refresh_entries();
                }
            }
        } else {
            self.refresh_entries();
        }
        doc.release();
        self.graveyard.borrow_mut().push(doc);
    }

    /// 回收此前关闭的文档的信号。调用时机是用户的下一次增删操作，距上次关闭至少隔了
    /// 一帧，引用旧信号的界面节点早已随 `host_signal` 重建被拆掉。
    fn reap(&self) {
        for d in self.graveyard.borrow_mut().drain(..) {
            d.dispose();
        }
    }

    /// 重算侧栏模型与窗口标题。文档的标题 / 副标题变了（解析完成、改名）也要调一次。
    pub fn refresh_entries(&self) {
        let active = self.active_id.get();
        let entries: Vec<DocEntry> = self.docs.with(|v| {
            v.iter()
                .map(|d| DocEntry {
                    doc: d.clone(),
                    active: d.id == active,
                })
                .collect()
        });
        self.entries.set(entries);
        let title = match self.active_doc() {
            Some(d) => format!("{} - {}", d.file_name(), APP_TITLE),
            None => APP_TITLE.to_string(),
        };
        self.window_title.set(title);
    }

    /// 当前文档的路径等「非信号派生」的展示要刷新时（改名后），重新发布一次 `active`。
    pub fn touch_active(&self) {
        let v = self.active.get();
        self.active.set(v);
        self.refresh_entries();
    }

    /// 当前文档的解析结果（没有文档或尚未解析完为 `None`）。
    pub fn current_info(&self) -> Option<Rc<ApkInfo>> {
        self.active_doc().and_then(|d| d.current_info())
    }

    // ---- 配置 ----

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

    /// 开启新一代（打开 / 重新解析时调用），返回新代次。
    pub fn next_generation(&self) -> u64 {
        let g = self.generation.get() + 1;
        self.generation.set(g);
        g
    }
}
