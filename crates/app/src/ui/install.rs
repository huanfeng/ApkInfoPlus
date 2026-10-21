//! 安装窗口：左栏是已打开的安装包（多选），右栏上方是设备（多选），下方是分包与高级选项。
//!
//! 独立子窗（`Window::single`）而非主窗内的模态：可以拖到主窗之外、与主窗并排看日志，
//! 也不会把主窗整个压暗锁死。多个安装包在后台线程里逐个安装，每个包装到所有选中的设备，
//! 进度经 `Msg::Install` 回到 UI 线程更新各行的状态信号；子窗没有自己的消息通道，读的是同一批 `Signal`。

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use apkinfo_core::adb::{AdbDevice, InstallOptions};
use apkinfo_core::icon::RgbaImage;
use apkinfo_core::info::{ApkInfo, SplitType};
use apkinfo_core::installer::{InstallEvent, InstallPhase, InstallRequest};
use windui::core::EventCtx;
use windui::prelude::*;

use crate::state::{AppState, Msg};
use crate::theme::{CARD_BLUE, CARD_ORANGE, CARD_PURPLE};

/// 一台设备在对话框里的一行。
#[derive(Clone)]
pub struct DeviceRow {
    pub device: AdbDevice,
    pub selected: Signal<bool>,
    pub phase: Signal<Option<InstallPhase>>,
    pub message: Signal<String>,
}

/// 左栏一个安装包。打开窗口时从已解析的文档拍快照：安装途中主窗关掉了这个文档，
/// 这里也不会去读已回收的文档信号。
#[derive(Clone)]
pub struct FileRow {
    pub doc_id: u64,
    pub info: Rc<ApkInfo>,
    pub title: String,
    pub subtitle: String,
    pub icon: Option<Rc<RgbaImage>>,
    pub selected: Signal<bool>,
    pub status: Signal<Option<FileStatus>>,
}

/// 一个安装包在本轮安装里的状态。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileStatus {
    Waiting,
    Installing,
    Success,
    /// 有的设备成功、有的失败。
    Partial,
    Failed,
    Stopped,
}

/// 安装线程发回 UI 线程的消息：第几个安装包开始了、它的安装事件、全部结束。
pub enum InstallMsg {
    FileStarted(usize),
    Event(usize, InstallEvent),
    AllDone,
}

#[derive(Clone)]
pub struct SplitRow {
    pub path: String,
    pub kind: SplitType,
    pub size: u64,
    pub selected: Signal<bool>,
}

/// 安装对话框的全部状态，随 `AppState` 一起创建。
#[derive(Clone)]
pub struct InstallState {
    pub files: Signal<Vec<FileRow>>,
    pub devices: Signal<Vec<DeviceRow>>,
    pub loading_devices: Signal<bool>,
    pub device_error: Signal<String>,
    pub installing: Signal<bool>,
    pub show_advanced: Signal<bool>,
    pub show_splits: Signal<bool>,
    pub allow_test: Signal<bool>,
    pub allow_downgrade: Signal<bool>,
    pub replace: Signal<bool>,
    pub grant: Signal<bool>,
    pub splits: Signal<Vec<SplitRow>>,
    /// 底栏与分包标题上的计数：勾选信号在行里，派生信号看不到它们变，故在勾选处手动刷新。
    pub file_count: Signal<usize>,
    pub split_count: Signal<String>,
    /// 分包行属于哪个安装包（`doc_id`）；0 = 当前没有分包行。
    pub splits_doc: Rc<Cell<u64>>,
    pub confirm_stop: Signal<bool>,
    pub cancel: Rc<RefCell<Option<Arc<AtomicBool>>>>,
    pub log: Signal<String>,
    /// 本轮安装里每台设备的（成功, 失败）个数，全部结束时汇总到设备行。
    pub tally: Rc<RefCell<HashMap<String, (u32, u32)>>>,
    /// 本轮安装的安装包（`files` 里的下标），与安装线程里的顺序一致。
    pub batch: Rc<RefCell<Vec<usize>>>,
    /// 安装包 / 分包 / 设备行的信号各归一个作用域：每次重建整批回收，否则每开一次就漏一批。
    /// `ManuallyDrop`：进程退出时信号 arena（线程局部）可能先于 AppState 销毁，
    /// 那时再 dispose 会撞 TLS 已析构而 abort；这几个作用域只在重建时手动回收。
    pub file_scope: Rc<RefCell<std::mem::ManuallyDrop<windui::signal::SignalScope>>>,
    pub split_scope: Rc<RefCell<std::mem::ManuallyDrop<windui::signal::SignalScope>>>,
    /// 设备行的信号同理（刷新时只给新设备建信号，但旧设备消失后要能回收）。
    pub device_scope: Rc<RefCell<std::mem::ManuallyDrop<windui::signal::SignalScope>>>,
}

fn new_scope() -> Rc<RefCell<std::mem::ManuallyDrop<windui::signal::SignalScope>>> {
    Rc::new(RefCell::new(std::mem::ManuallyDrop::new(
        windui::signal::SignalScope::new(),
    )))
}

impl InstallState {
    pub fn new(cfg: &crate::config::Config) -> Self {
        Self {
            files: signal(Vec::new()),
            devices: signal(Vec::new()),
            loading_devices: signal(false),
            device_error: signal(String::new()),
            installing: signal(false),
            show_advanced: signal(false),
            show_splits: signal(false),
            allow_test: signal(cfg.install_allow_test),
            allow_downgrade: signal(cfg.install_allow_downgrade),
            replace: signal(cfg.install_replace),
            grant: signal(cfg.install_grant),
            splits: signal(Vec::new()),
            file_count: signal(0),
            split_count: signal(String::new()),
            splits_doc: Default::default(),
            confirm_stop: signal(false),
            cancel: Rc::new(RefCell::new(None)),
            log: signal(String::new()),
            tally: Default::default(),
            batch: Default::default(),
            file_scope: new_scope(),
            split_scope: new_scope(),
            device_scope: new_scope(),
        }
    }

    /// 这个文档是否在本轮安装的安装包里。
    pub fn is_installing_doc(&self, doc_id: u64) -> bool {
        let batch = self.batch.borrow();
        self.files.with(|v| {
            batch
                .iter()
                .any(|&i| v.get(i).is_some_and(|f| f.doc_id == doc_id))
        })
    }

    /// 刷新底栏的已选安装包数与分包标题上的「已选 / 全部」。
    fn refresh_counts(&self) {
        self.file_count.set(self.selected_files().len());
        let text = self.splits.with(|v| {
            if v.is_empty() {
                String::new()
            } else {
                let n = v.iter().filter(|s| s.selected.get()).count();
                format!("{} / {}", n, v.len())
            }
        });
        self.split_count.set(text);
    }

    /// 选中的安装包下标。
    fn selected_files(&self) -> Vec<usize> {
        self.files.with(|v| {
            v.iter()
                .enumerate()
                .filter(|(_, f)| f.selected.get())
                .map(|(i, _)| i)
                .collect()
        })
    }
}

/// 打开安装窗口：列出已解析的安装包（默认只选中当前文档）、刷新设备。
/// 窗口已开着时只把它拉到前台（`single` 去重）。
pub fn open(state: &AppState, ctx: &mut EventCtx) {
    let Some(active) = state.active_doc() else {
        return;
    };
    if active.current_info().is_none() {
        return;
    }
    let ins = &state.install;
    // 安装进行中只把窗口重新拉到前台，不能重置设备状态与日志。
    if ins.installing.get() {
        ctx.open_window(window(state));
        return;
    }
    // 先把旧行清掉再回收旧信号：host_signal 下一帧按新数据重建，不会再读到已回收的句柄。
    ins.files.set(Vec::new());
    let mut scope = ins.file_scope.borrow_mut();
    scope.dispose();
    let docs = state.docs.get();
    let rows: Vec<FileRow> = scope.collect(|| {
        docs.iter()
            .filter_map(|d| {
                let info = d.current_info()?;
                let label = d.display_label();
                let file_name = d.file_name();
                let version = info.meta.version_name.clone().unwrap_or_default();
                let (title, subtitle) = if label.trim().is_empty() {
                    (file_name, version)
                } else if version.is_empty() {
                    (label, file_name)
                } else {
                    (label, format!("{} · {}", version, file_name))
                };
                Some(FileRow {
                    doc_id: d.id,
                    info,
                    title,
                    subtitle,
                    icon: d.icon.with(|v| v.first().cloned()),
                    selected: signal(d.id == active.id),
                    status: signal(None),
                })
            })
            .collect()
    });
    drop(scope);
    ins.files.set(rows);
    ins.splits_doc.set(0);
    sync_splits(state);
    ins.log.set(String::new());
    ins.confirm_stop.set(false);
    for d in ins.devices.get() {
        d.phase.set(None);
        d.message.set(String::new());
    }
    ctx.open_window(window(state));
    refresh_devices(state, ctx);
}

/// 只选中一个归档包时，分包区列出它的分包；选了多个或选的不是归档包时清空
/// （多个安装包一起装时各自装全部分包，不提供逐个勾选）。
fn sync_splits(state: &AppState) {
    let ins = &state.install;
    let selected = ins.selected_files();
    let target = match selected.as_slice() {
        [i] => ins
            .files
            .with(|v| v.get(*i).cloned())
            .filter(|f| f.info.is_archive()),
        _ => None,
    };
    let target_id = target.as_ref().map(|f| f.doc_id).unwrap_or(0);
    if target_id == ins.splits_doc.get() {
        ins.refresh_counts();
        return;
    }
    ins.splits_doc.set(target_id);
    ins.splits.set(Vec::new());
    let mut scope = ins.split_scope.borrow_mut();
    scope.dispose();
    let rows: Vec<SplitRow> = match &target {
        Some(f) => scope.collect(|| {
            f.info
                .splits
                .iter()
                .map(|s| SplitRow {
                    path: s.path.clone(),
                    kind: s.kind,
                    size: s.size,
                    selected: signal(true),
                })
                .collect()
        }),
        None => Vec::new(),
    };
    drop(scope);
    ins.splits.set(rows);
    ins.refresh_counts();
}

fn window(state: &AppState) -> WindowRequest {
    let st = state.clone();
    let installing = state.install.installing;
    let confirm = state.install.confirm_stop;
    Window::new(tr!("install.title"), 920, 640)
        .single("install")
        .centered(true)
        .min_size(760, 500)
        .frameless(!cfg!(target_os = "macos"))
        .title(t!("install.title"))
        // 安装中关窗：不直接关，弹「确认停止」；停下来之后窗口留着看结果，用户再关。
        .on_close_request(move |_ctx| {
            if installing.get() {
                confirm.set(true);
                false
            } else {
                true
            }
        })
        .content(move || build_window(&st))
}

pub fn refresh_devices(state: &AppState, ctx: &mut EventCtx) {
    let ins = &state.install;
    let Some(adb) = crate::tools::resolve_adb(state) else {
        ins.device_error.set(tr!("install.no_adb"));
        ins.devices.set(Vec::new());
        return;
    };
    let _ = ctx;
    ins.device_error.set(String::new());
    ins.loading_devices.set(true);
    let tx = state.tx.clone();
    thread::spawn(move || {
        let result = apkinfo_core::adb::list_devices(&adb);
        let _ = tx.send(Msg::Devices(result));
    });
}

pub fn on_devices(state: &AppState, _ctx: &mut EventCtx, result: Result<Vec<AdbDevice>, String>) {
    let ins = &state.install;
    ins.loading_devices.set(false);
    match result {
        Ok(list) => {
            // 保留已有行的选中状态与结果（刷新不应清掉刚装完的结果）。
            // 旧行的状态要保留（刚装完的结果不能被刷新清掉），所以先把值拷出来，
            // 再在新的作用域里重建全部信号——这样消失的设备不会留下永不回收的槽位。
            let old: Vec<(String, bool, Option<InstallPhase>, String)> = ins.devices.with(|v| {
                v.iter()
                    .map(|r| {
                        (
                            r.device.id.clone(),
                            r.selected.get(),
                            r.phase.get(),
                            r.message.get(),
                        )
                    })
                    .collect()
            });
            ins.devices.set(Vec::new());
            let mut scope = ins.device_scope.borrow_mut();
            scope.dispose();
            let rows = scope.collect(|| {
                list.into_iter()
                    .map(|d| {
                        let prev = old.iter().find(|(id, ..)| *id == d.id);
                        let default_sel = !d.is_offline() && !d.is_unauthorized();
                        DeviceRow {
                            selected: signal(prev.map(|p| p.1).unwrap_or(default_sel)),
                            phase: signal(prev.and_then(|p| p.2)),
                            message: signal(prev.map(|p| p.3.clone()).unwrap_or_default()),
                            device: d,
                        }
                    })
                    .collect::<Vec<_>>()
            });
            drop(scope);
            ins.devices.set(rows);
            ins.device_error.set(String::new());
        }
        Err(e) => {
            ins.devices.set(Vec::new());
            ins.device_error.set(e);
        }
    }
}

pub fn on_msg(state: &AppState, ctx: &mut EventCtx, msg: InstallMsg) {
    let ins = &state.install;
    let file = |i: usize| -> Option<FileRow> {
        let idx = *ins.batch.borrow().get(i)?;
        ins.files.with(|v| v.get(idx).cloned())
    };
    let multi = ins.batch.borrow().len() > 1;
    match msg {
        InstallMsg::FileStarted(i) => {
            if let Some(f) = file(i) {
                f.status.set(Some(FileStatus::Installing));
                if multi {
                    ins.log
                        .update(|l| l.push_str(&format!("== {} ==\n", f.title)));
                }
            }
        }
        InstallMsg::Event(i, ev) => match ev {
            InstallEvent::Device {
                device,
                phase,
                message,
            } => {
                if let Some(row) = ins
                    .devices
                    .with(|v| v.iter().find(|r| r.device.id == device).cloned())
                {
                    row.phase.set(Some(phase));
                    row.message.set(message.clone());
                }
                match phase {
                    InstallPhase::Success => {
                        ins.tally.borrow_mut().entry(device.clone()).or_default().0 += 1
                    }
                    InstallPhase::Failed => {
                        ins.tally.borrow_mut().entry(device.clone()).or_default().1 += 1
                    }
                    _ => {}
                }
                if !message.is_empty() {
                    ins.log
                        .update(|l| l.push_str(&format!("[{}] {}\n", device, message)));
                }
            }
            InstallEvent::Log(line) => {
                ins.log.update(|l| {
                    l.push_str(&line);
                    l.push('\n');
                });
            }
            InstallEvent::Finished { success, failed } => {
                if let Some(f) = file(i) {
                    f.status.set(Some(match (success, failed) {
                        (0, 0) => FileStatus::Stopped,
                        (_, 0) => FileStatus::Success,
                        (0, _) => FileStatus::Failed,
                        _ => FileStatus::Partial,
                    }));
                }
            }
        },
        InstallMsg::AllDone => {
            ins.installing.set(false);
            *ins.cancel.borrow_mut() = None;
            // 没轮到的（中途停止）记为已停止。
            for idx in ins.batch.borrow().iter() {
                if let Some(f) = ins.files.with(|v| v.get(*idx).cloned()) {
                    if matches!(
                        f.status.get(),
                        Some(FileStatus::Waiting | FileStatus::Installing)
                    ) {
                        f.status.set(Some(FileStatus::Stopped));
                    }
                }
            }
            let tally = ins.tally.borrow().clone();
            let (mut ok, mut bad) = (0, 0);
            for row in ins.devices.get() {
                let Some(&(s, f)) = tally.get(&row.device.id) else {
                    continue;
                };
                ok += s;
                bad += f;
                // 多个安装包：设备行最后一条事件只代表最后一个包，换成汇总。
                if multi {
                    row.phase.set(Some(if f == 0 {
                        InstallPhase::Success
                    } else {
                        InstallPhase::Failed
                    }));
                    row.message
                        .set(tr!("install.summary", ok = s as i64, failed = f as i64));
                }
                // 全部装好的设备自动取消勾选，防止再点一次重复安装。
                if f == 0 && s > 0 {
                    row.selected.set(false);
                }
            }
            if bad == 0 && ok > 0 {
                ctx.toast_ok(tr!("install.success"));
            } else if bad > 0 {
                ctx.toast_err(tr!("install.failed"));
            }
        }
    }
}

fn start_install(state: &AppState, ctx: &mut EventCtx) {
    let ins = &state.install;
    let batch = ins.selected_files();
    if batch.is_empty() {
        ctx.toast_err(tr!("install.no_file_selection"));
        return;
    }
    let Some(adb) = crate::tools::resolve_adb(state) else {
        ctx.toast_err(tr!("install.no_adb"));
        return;
    };
    let devices: Vec<String> = ins.devices.with(|v| {
        v.iter()
            .filter(|r| r.selected.get() && !r.device.is_offline())
            .map(|r| r.device.id.clone())
            .collect()
    });
    if devices.is_empty() {
        ctx.toast_err(tr!("install.no_selection"));
        return;
    }
    let options = InstallOptions {
        allow_test: ins.allow_test.get(),
        allow_downgrade: ins.allow_downgrade.get(),
        replace: ins.replace.get(),
        grant_permissions: ins.grant.get(),
    };
    state.update_config(|c| {
        c.install_allow_test = options.allow_test;
        c.install_allow_downgrade = options.allow_downgrade;
        c.install_replace = options.replace;
        c.install_grant = options.grant_permissions;
    });
    let files: Vec<FileRow> = ins
        .files
        .with(|v| batch.iter().map(|&i| v[i].clone()).collect());
    let requests: Vec<InstallRequest> = files
        .iter()
        .map(|f| {
            // 分包只在单选一个归档包时可勾；其余情况装全部分包。
            let selected_splits =
                (f.info.is_archive() && ins.splits_doc.get() == f.doc_id).then(|| {
                    ins.splits.with(|v| {
                        v.iter()
                            .filter(|s| s.selected.get())
                            .map(|s| s.path.clone())
                            .collect()
                    })
                });
            InstallRequest {
                adb: adb.clone(),
                file: f.info.path.clone(),
                kind: f.info.kind,
                devices: devices.clone(),
                options,
                selected_splits,
                package_name: f.info.meta.package_name.clone(),
            }
        })
        .collect();
    for f in ins.files.get() {
        f.status.set(None);
    }
    for f in &files {
        f.status.set(Some(FileStatus::Waiting));
    }
    for id in &devices {
        if let Some(row) = ins
            .devices
            .with(|v| v.iter().find(|r| &r.device.id == id).cloned())
        {
            row.phase.set(None);
            row.message.set(String::new());
        }
    }
    *ins.batch.borrow_mut() = batch;
    ins.tally.borrow_mut().clear();
    ins.log.set(String::new());
    ins.installing.set(true);
    let cancel = Arc::new(AtomicBool::new(false));
    *ins.cancel.borrow_mut() = Some(cancel.clone());

    let tx = state.tx.clone();
    thread::spawn(move || {
        for (i, req) in requests.into_iter().enumerate() {
            if cancel.load(Ordering::SeqCst) {
                break;
            }
            let _ = tx.send(Msg::Install(InstallMsg::FileStarted(i)));
            apkinfo_core::installer::run(req, cancel.clone(), |ev| {
                let _ = tx.send(Msg::Install(InstallMsg::Event(i, ev)));
            });
        }
        let _ = tx.send(Msg::Install(InstallMsg::AllDone));
    });
}

fn stop_install(state: &AppState) {
    let ins = &state.install;
    if let Some(c) = ins.cancel.borrow().as_ref() {
        c.store(true, Ordering::SeqCst);
    }
    ins.confirm_stop.set(false);
}

// ---------------------------------------------------------------------------
// 界面
// ---------------------------------------------------------------------------

fn build_window(state: &AppState) -> Element {
    let st = state.clone();
    let ins = state.install.clone();
    let installing = ins.installing;

    // 右栏：设备在上，分包与高级选项在下，日志垫底。右侧多留滚动条的位置。
    let right = Element::scroll().fill().child(
        Element::col()
            .width_match()
            .spacing(super::CARD_GAP)
            .padding_edges(Insets::new(0, 0, super::SCROLL_GUTTER, 0))
            .child(device_section(state))
            .child(splits_section(state))
            .child(options_section(state))
            .child(log_section(state)),
    );
    let body = Element::row()
        .fill()
        .spacing(super::CARD_GAP)
        .padding_edges(Insets::new(
            super::PAGE_PAD,
            super::PAGE_PAD,
            super::PAGE_PAD - 4,
            super::PAGE_PAD,
        ))
        .child(files_section(state).width(300).height_match())
        .child(right.weight(1.0));

    let files = ins.files;
    let devices = ins.devices;
    let summary = Element::label(Signal::map(ins.file_count, |n| {
        tr!("install.selected_files", count = *n as i64)
    }))
    .font_size(12.5)
    .fg_role(Role::TextMuted);

    let footer = Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(10)
        .padding_edges(Insets::new(super::PAGE_PAD, 10, super::PAGE_PAD, 12))
        .bg_role(Role::Surface)
        .child(summary)
        .child(Element::flex_spacer())
        .child(
            Element::button(t!("action.close"))
                .neutral()
                .outline_soft()
                .min_width(96)
                .visible_when(move || !installing.get())
                .on_click(move |ctx| ctx.request_close()),
        )
        .child(
            Element::button(t!("install.stop"))
                .danger()
                .outline_soft()
                .min_width(96)
                .visible_when(move || installing.get())
                .on_click({
                    let confirm = ins.confirm_stop;
                    move |_| confirm.set(true)
                }),
        )
        .child(
            Element::button(t!("action.install"))
                .icon_content(crate::icons::content("play", crate::icons::ICON_WHITE))
                .min_width(140)
                .enabled_when({
                    let splits = ins.splits;
                    move || {
                        !installing.get()
                            && files.with(|v| v.iter().any(|f| f.selected.get()))
                            && devices.with(|v| {
                                v.iter().any(|r| r.selected.get() && !r.device.is_offline())
                            })
                            // 单选归档包：一个分包都没勾就没东西可装
                            && splits.with(|v| v.is_empty() || v.iter().any(|s| s.selected.get()))
                    }
                })
                .on_click({
                    let st = st.clone();
                    move |ctx| start_install(&st, ctx)
                }),
        );

    let page = Element::col()
        .fill()
        .bg_role(Role::Bg)
        .child(body.weight(1.0))
        .child(Element::divider())
        .child(footer);

    // 停止确认：叠在安装窗口之上的小对话框。
    let confirm = ins.confirm_stop;
    let confirm_dialog = Element::dialog(
        confirm,
        Element::col()
            .width(360)
            .padding(20)
            .spacing(14)
            .bg_role(Role::Surface)
            .corner(12.0)
            .child(
                Element::label(t!("install.stop_confirm_title"))
                    .font_size(16.0)
                    .font_weight(600)
                    .fg_role(Role::Text),
            )
            .child(
                Element::label(t!("install.stop_confirm_message"))
                    .fg_role(Role::TextMuted)
                    .width_match(),
            )
            .child(
                Element::row()
                    .width_match()
                    .spacing(8)
                    .child(Element::flex_spacer())
                    .child(
                        Element::button(t!("action.cancel"))
                            .small()
                            .neutral()
                            .outline_soft()
                            .on_click(move |_| confirm.set(false)),
                    )
                    .child(
                        Element::button(t!("install.stop"))
                            .small()
                            .danger()
                            .on_click({
                                let st = st.clone();
                                move |_| stop_install(&st)
                            }),
                    ),
            ),
    );

    Element::stack()
        .fill()
        .child(super::window_chrome(tr!("install.title"), page))
        .child(confirm_dialog)
}

/// 左栏：已打开的安装包，多选；默认只选中发起安装的那个。
fn files_section(state: &AppState) -> Element {
    let st = state.clone();
    let ins = state.install.clone();
    let files = ins.files;
    let installing = ins.installing;
    let list = Element::host_signal(files, move |row: FileRow| file_row(&st, row))
        .width_match()
        .spacing(2);
    let count = Element::label(Signal::map(files, |v| v.len().to_string()))
        .font_size(12.0)
        .fg_role(Role::TextMuted);
    let st = state.clone();
    let head = Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(6)
        .padding_edges(Insets::new(14, 12, 12, 6))
        .child(
            Element::label(t!("install.files"))
                .font_size(13.5)
                .font_weight(600)
                .fg_role(Role::Text),
        )
        .child(count)
        .child(Element::flex_spacer())
        .child(
            Element::link(t!("install.select_all"))
                .font_size(12.5)
                .enabled_when(move || !installing.get())
                .on_click(move |_| {
                    for f in files.get() {
                        f.selected.set(true);
                    }
                    sync_splits(&st);
                }),
        );
    Element::col()
        .bg_role(Role::Surface)
        .corner(super::CARD_CORNER)
        .border_role(Role::Border, 1)
        .child(head)
        .child(
            Element::scroll()
                .width_match()
                .weight(1.0)
                .child(list.padding_edges(Insets::new(6, 0, 6, 6))),
        )
}

fn file_row(state: &AppState, row: FileRow) -> Element {
    let st = state.clone();
    let selected = row.selected;
    let installing = state.install.installing;
    let icon = match &row.icon {
        Some(img) => Element::image_rgba(img.width, img.height, &img.data)
            .fit(Fit::Contain)
            .size(32, 32)
            .corner(8.0),
        None => Element::stack()
            .size(32, 32)
            .corner(8.0)
            .bg_role(Role::SurfaceAlt)
            .child(
                Element::image_content(crate::icons::gray("package"))
                    .size(18, 18)
                    .align_xy(Align::Center, Align::Center),
            ),
    };
    let status = row.status;
    let status_text = Signal::map(status, |s| match s {
        None => String::new(),
        Some(FileStatus::Waiting) => tr!("install.waiting"),
        Some(FileStatus::Installing) => tr!("install.installing"),
        Some(FileStatus::Success) => tr!("install.success"),
        Some(FileStatus::Partial) => tr!("install.partial"),
        Some(FileStatus::Failed) => tr!("install.failed"),
        Some(FileStatus::Stopped) => tr!("install.stopped"),
    });
    let status_role = Signal::map(status, |s| match s {
        Some(FileStatus::Success) => Role::Success,
        Some(FileStatus::Failed | FileStatus::Partial) => Role::Danger,
        Some(FileStatus::Stopped) => Role::Warning,
        Some(FileStatus::Installing) => Role::Accent,
        _ => Role::TextMuted,
    });
    Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(8)
        .padding_xy(6, 6)
        .corner(8.0)
        .tooltip(row.info.path.display().to_string())
        .child(
            Element::checkbox("", selected)
                .enabled_when(move || !installing.get())
                .on_toggle(move |_| {
                    selected.set(!selected.get());
                    sync_splits(&st);
                }),
        )
        .child(icon)
        .child(
            Element::col()
                .weight(1.0)
                .spacing(2)
                .child(
                    Element::label(row.title.clone())
                        .font_size(13.0)
                        .font_weight(600)
                        .fg_role(Role::Text)
                        .max_lines(1)
                        .truncate(Truncate::End)
                        .width_match(),
                )
                .child(
                    Element::label(row.subtitle.clone())
                        .font_size(11.5)
                        .fg_role(Role::TextMuted)
                        .max_lines(1)
                        .truncate(Truncate::Middle)
                        .width_match(),
                )
                .child(
                    Element::label(status_text)
                        .font_size(11.5)
                        .fg_role_signal(status_role)
                        .visible_when(move || status.with(|s| s.is_some())),
                ),
        )
        .child(super::kind_badge_small(row.info.kind))
}

fn device_section(state: &AppState) -> Element {
    let st = state.clone();
    let ins = state.install.clone();
    let loading = ins.loading_devices;
    let error = ins.device_error;
    let devices = ins.devices;
    let installing = ins.installing;

    let list = Element::host_signal(devices, |row: DeviceRow| device_row(row)).width_match();

    // 标题行右侧：设备数 + 刷新。
    let action = Element::row()
        .cross(Align::Center)
        .spacing(6)
        .child(
            Element::label(Signal::map(devices, |v| {
                tr!("install.device_count", count = v.len() as i64)
            }))
            .font_size(12.0)
            .fg_role(Role::TextMuted),
        )
        .child(
            crate::icons::button("refresh", 26, tr!("install.refresh_devices"))
                .enabled_when(move || !installing.get() && !loading.get())
                .on_click(move |ctx| refresh_devices(&st, ctx)),
        );
    card(
        "smartphone",
        CARD_BLUE,
        tr!("install.select_device"),
        Some(action),
        Element::col()
            .width_match()
            .child(
                Element::label(t!("install.loading_devices"))
                    .fg_role(Role::TextMuted)
                    .padding(8)
                    .visible_when(move || loading.get()),
            )
            .child(
                Element::label(error)
                    .fg_role(Role::Danger)
                    .padding(8)
                    .width_match()
                    .visible_when(move || !loading.get() && error.with(|e| !e.is_empty())),
            )
            .child(
                Element::label(t!("install.no_devices"))
                    .fg_role(Role::TextMuted)
                    .padding(8)
                    .visible_when(move || {
                        !loading.get()
                            && error.with(|e| e.is_empty())
                            && devices.with(|v| v.is_empty())
                    }),
            )
            .child(list),
    )
}

fn device_row(row: DeviceRow) -> Element {
    let d = &row.device;
    let offline = d.is_offline();
    let unauthorized = d.is_unauthorized();
    let status_text = if offline {
        tr!("install.offline")
    } else if unauthorized {
        tr!("install.unauthorized")
    } else {
        tr!("install.online")
    };
    let status_badge = if offline || unauthorized {
        Element::badge_intent(status_text, Intent::Neutral)
    } else {
        Element::badge_intent(status_text, Intent::Success)
    };
    let phase = row.phase;
    let message = row.message;
    let result_text = Signal::map(phase, move |p| match p {
        None => String::new(),
        Some(InstallPhase::Installing) => tr!("install.installing"),
        Some(InstallPhase::PushingObb) => tr!("install.pushing_obb"),
        Some(InstallPhase::Success) => {
            let m = message.get();
            if m.is_empty() {
                tr!("install.success")
            } else {
                m
            }
        }
        Some(InstallPhase::Failed) => {
            let m = message.get();
            if m.is_empty() {
                tr!("install.failed")
            } else {
                format!("{}: {}", tr!("install.failed"), crate::ui::shorten(&m, 80))
            }
        }
        Some(InstallPhase::Stopped) => tr!("install.stopped"),
    });
    let result_role = Signal::map(phase, |p| match p {
        Some(InstallPhase::Success) => Role::Success,
        Some(InstallPhase::Failed) => Role::Danger,
        Some(InstallPhase::Stopped) => Role::Warning,
        _ => Role::TextMuted,
    });
    // 标题是型号（无线设备也显示型号），第二行是序列号 / 地址与设备代号。
    let title = d
        .model
        .as_ref()
        .map(|m| m.replace('_', " "))
        .unwrap_or_else(|| d.id.clone());
    let mut details = Vec::new();
    if d.model.is_some() {
        details.push(d.id.clone());
    }
    if let Some(code) = d.device.as_ref().or(d.product.as_ref()) {
        details.push(code.clone());
    }
    let subtitle = details.join(" · ");
    Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(10)
        .padding_xy(6, 5)
        .child(Element::checkbox("", row.selected).enabled(!offline))
        .child(
            Element::col()
                .weight(1.0)
                .spacing(2)
                .child(
                    Element::row()
                        .cross(Align::Center)
                        .spacing(8)
                        .child(
                            Element::label(title)
                                .font_size(13.5)
                                .font_weight(600)
                                .fg_role(if offline {
                                    Role::TextDisabled
                                } else {
                                    Role::Text
                                }),
                        )
                        .child(status_badge),
                )
                .child(
                    Element::label(subtitle.clone())
                        .font_size(11.5)
                        .fg_role(Role::TextSubtle)
                        .font_family(super::mono_family())
                        .visible(!subtitle.is_empty()),
                ),
        )
        .child(
            Element::label(result_text)
                .font_size(12.5)
                .fg_role_signal(result_role)
                .max_lines(2)
                .width(200),
        )
}

fn options_section(state: &AppState) -> Element {
    let ins = state.install.clone();
    fold_card(
        "settings",
        CARD_PURPLE,
        tr!("install.advanced_options"),
        None,
        ins.show_advanced,
        Element::col()
            .width_match()
            .spacing(4)
            .child(Element::checkbox(t!("install.force_install"), ins.replace))
            .child(Element::checkbox(t!("install.allow_test"), ins.allow_test))
            .child(Element::checkbox(
                t!("install.allow_downgrade"),
                ins.allow_downgrade,
            ))
            .child(Element::checkbox(
                t!("install.grant_permissions"),
                ins.grant,
            )),
    )
}

/// 分包选择：只在选中的安装包里有归档包时出现，默认折叠。只选中一个归档包时可逐个勾选；
/// 选了多个时各包安装全部分包，这里只给提示。
fn splits_section(state: &AppState) -> Element {
    let ins = state.install.clone();
    let splits = ins.splits;
    let files = ins.files;

    let counts = ins.clone();
    let rows = Element::host_signal(splits, move |row: SplitRow| {
        let counts = counts.clone();
        let selected = row.selected;
        let is_base = row.kind == SplitType::Base;
        let kind_key = format!("install.split_type.{}", row.kind.key());
        Element::row()
            .width_match()
            .cross(Align::Center)
            .spacing(8)
            .padding_xy(4, 1)
            .child(
                Element::checkbox(row.path.clone(), selected)
                    .enabled(!is_base)
                    .on_toggle(move |_| {
                        selected.set(!selected.get());
                        counts.refresh_counts();
                    })
                    .weight(1.0),
            )
            .child(
                Element::label(apkinfo_core::info::format_file_size(row.size))
                    .font_size(12.0)
                    .fg_role(Role::TextMuted)
                    .width(72)
                    .text_align(Align::End),
            )
            .child(Element::badge_intent(tr!(kind_key), Intent::Neutral))
    })
    .width_match();

    let preset = |label: &'static str, pick: fn(SplitType) -> bool| {
        let counts = ins.clone();
        Element::button(t!(label))
            .small()
            .neutral()
            .outline_soft()
            // 先把行拷出来再写：`with` 期间信号仓持共享借用，闭包里再 set 会撞 RefCell 借用而 panic
            .on_click(move |_| {
                for s in splits.get() {
                    s.selected.set(pick(s.kind) || s.kind == SplitType::Base);
                }
                counts.refresh_counts();
            })
    };
    let toolbar = Element::row()
        .width_match()
        .spacing(8)
        .padding_xy(4, 2)
        .child(preset("install.select_all", |_| true))
        .child(preset("install.base_only", |k| k == SplitType::Base))
        .child(preset("install.base_and_language", |k| {
            matches!(k, SplitType::Base | SplitType::Language)
        }));

    let editable = Element::col()
        .width_match()
        .spacing(4)
        .child(toolbar)
        .child(rows)
        .visible_when(move || splits.with(|v| !v.is_empty()));
    let multi_hint = Element::label(t!("install.splits_multi_hint"))
        .font_size(12.5)
        .fg_role(Role::TextMuted)
        .width_match()
        .padding_xy(4, 2)
        .visible_when(move || splits.with(|v| v.is_empty()));

    let summary = Element::label(ins.split_count)
        .font_size(12.0)
        .fg_role(Role::TextMuted);

    fold_card(
        "layers",
        CARD_ORANGE,
        tr!("install.split_options"),
        Some(summary),
        ins.show_splits,
        Element::col()
            .width_match()
            .child(editable)
            .child(multi_hint),
    )
    .visible_when(move || files.with(|v| v.iter().any(|f| f.selected.get() && f.info.is_archive())))
}

/// 卡片：图标 + 标题（+ 右侧动作）+ 内容。
fn card(
    icon: &str,
    color: Color,
    title: String,
    action: Option<Element>,
    body: Element,
) -> Element {
    let mut head = Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(10)
        .child(super::card_icon(icon, color))
        .child(
            Element::label(title)
                .font_size(14.0)
                .font_weight(600)
                .fg_role(Role::Text)
                .weight(1.0),
        );
    if let Some(a) = action {
        head = head.child(a);
    }
    Element::col()
        .width_match()
        .bg_role(Role::Surface)
        .corner(super::CARD_CORNER)
        .border_role(Role::Border, 1)
        .padding(super::CARD_PAD)
        .spacing(8)
        .child(head)
        .child(body)
}

/// 可折叠卡片：点标题行展开 / 收起，右端箭头指示状态。
fn fold_card(
    icon: &str,
    color: Color,
    title: String,
    summary: Option<Element>,
    show: Signal<bool>,
    body: Element,
) -> Element {
    let mut head = Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(10)
        .clickable()
        .on_click(move |_| show.set(!show.get()))
        .child(super::card_icon(icon, color))
        .child(
            Element::label(title)
                .font_size(14.0)
                .font_weight(600)
                .fg_role(Role::Text)
                .weight(1.0),
        );
    if let Some(s) = summary {
        head = head.child(s);
    }
    head = head
        .child(
            Element::image_content(crate::icons::gray("chevron_down"))
                .size(16, 16)
                .visible_when(move || show.get()),
        )
        .child(
            Element::image_content(crate::icons::gray("chevron_right"))
                .size(16, 16)
                .visible_when(move || !show.get()),
        );
    Element::col()
        .width_match()
        .bg_role(Role::Surface)
        .corner(super::CARD_CORNER)
        .border_role(Role::Border, 1)
        .padding(super::CARD_PAD)
        .spacing(8)
        .child(head)
        .child(body.visible_when(move || show.get()))
}

fn log_section(state: &AppState) -> Element {
    let log = state.install.log;
    Element::col()
        .width_match()
        .spacing(4)
        .child(
            Element::label(t!("install.log"))
                .font_size(12.5)
                .font_weight(600)
                .fg_role(Role::TextMuted),
        )
        .child(crate::ui::mono_block(log, 110))
        .visible_when(move || log.with(|l| !l.is_empty()))
}
