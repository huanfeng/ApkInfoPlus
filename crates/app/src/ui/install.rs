//! 安装窗口：设备列表（多选）+ 高级选项 + 分包勾选 + 进度与取消。
//!
//! 独立子窗（`Window::single`）而非主窗内的模态：可以拖到主窗之外、与主窗并排看日志，
//! 也不会把主窗整个压暗锁死。安装在后台线程串行执行，进度经 `Msg::Install` 回到 UI 线程
//! 更新各设备行的状态信号；子窗没有自己的消息通道，读的是同一批 `Signal`。

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use apkinfo_core::adb::{AdbDevice, InstallOptions};
use apkinfo_core::info::SplitType;
use apkinfo_core::installer::{InstallEvent, InstallPhase, InstallRequest};
use windui::core::EventCtx;
use windui::prelude::*;

use crate::state::{AppState, Msg};

/// 一台设备在对话框里的一行。
#[derive(Clone)]
pub struct DeviceRow {
    pub device: AdbDevice,
    pub selected: Signal<bool>,
    pub phase: Signal<Option<InstallPhase>>,
    pub message: Signal<String>,
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
    pub confirm_stop: Signal<bool>,
    pub cancel: Rc<std::cell::RefCell<Option<Arc<AtomicBool>>>>,
    pub log: Signal<String>,
    /// 窗口顶部显示的「正在为哪个文件安装」。
    pub file_name: Signal<String>,
    /// 装的是哪个文档（打开窗口后用户可能切到别的文档，装的仍是这一个）。
    pub doc_id: Rc<std::cell::Cell<u64>>,
    /// 分包行的信号归它所有：每次打开对话框整批回收再重建，否则每开一次就漏一批。
    /// `ManuallyDrop`：进程退出时信号 arena（线程局部）可能先于 AppState 销毁，
    /// 那时再 dispose 会撞 TLS 已析构而 abort；这两个作用域只在重建时手动回收。
    pub split_scope: Rc<std::cell::RefCell<std::mem::ManuallyDrop<windui::signal::SignalScope>>>,
    /// 设备行的信号同理（刷新时只给新设备建信号，但旧设备消失后要能回收）。
    pub device_scope: Rc<std::cell::RefCell<std::mem::ManuallyDrop<windui::signal::SignalScope>>>,
}

impl InstallState {
    pub fn new(cfg: &crate::config::Config) -> Self {
        Self {
            devices: signal(Vec::new()),
            loading_devices: signal(false),
            device_error: signal(String::new()),
            installing: signal(false),
            show_advanced: signal(false),
            show_splits: signal(true),
            allow_test: signal(cfg.install_allow_test),
            allow_downgrade: signal(cfg.install_allow_downgrade),
            replace: signal(cfg.install_replace),
            grant: signal(cfg.install_grant),
            splits: signal(Vec::new()),
            confirm_stop: signal(false),
            cancel: Rc::new(std::cell::RefCell::new(None)),
            log: signal(String::new()),
            file_name: signal(String::new()),
            doc_id: Default::default(),
            split_scope: Rc::new(std::cell::RefCell::new(std::mem::ManuallyDrop::new(
                windui::signal::SignalScope::new(),
            ))),
            device_scope: Rc::new(std::cell::RefCell::new(std::mem::ManuallyDrop::new(
                windui::signal::SignalScope::new(),
            ))),
        }
    }
}

/// 打开安装窗口：填充分包行、刷新设备。窗口已开着时只把它拉到前台（`single` 去重）。
pub fn open(state: &AppState, ctx: &mut EventCtx) {
    let Some(doc) = state.active_doc() else {
        return;
    };
    let Some(info) = doc.current_info() else {
        return;
    };
    let ins = &state.install;
    // 安装进行中只把窗口重新拉到前台，不能重置设备状态与日志。
    if ins.installing.get() {
        ctx.open_window(window(state));
        return;
    }
    ins.doc_id.set(doc.id);
    ins.file_name.set(
        info.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    // 先把旧行清掉再回收旧信号：host_signal 下一帧按新数据重建，不会再读到已回收的句柄。
    ins.splits.set(Vec::new());
    let mut scope = ins.split_scope.borrow_mut();
    scope.dispose();
    let rows: Vec<SplitRow> = scope.collect(|| {
        info.splits
            .iter()
            .map(|s| SplitRow {
                path: s.path.clone(),
                kind: s.kind,
                size: s.size,
                selected: signal(true),
            })
            .collect()
    });
    drop(scope);
    ins.splits.set(rows);
    ins.log.set(String::new());
    ins.confirm_stop.set(false);
    for d in ins.devices.get() {
        d.phase.set(None);
        d.message.set(String::new());
    }
    ctx.open_window(window(state));
    refresh_devices(state, ctx);
}

fn window(state: &AppState) -> WindowRequest {
    let st = state.clone();
    let installing = state.install.installing;
    let confirm = state.install.confirm_stop;
    Window::new(tr!("install.title"), 680, 640)
        .single("install")
        .centered(true)
        .min_size(560, 480)
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

pub fn on_event(state: &AppState, ctx: &mut EventCtx, ev: InstallEvent) {
    let ins = &state.install;
    match ev {
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
                if phase == InstallPhase::Success {
                    // 装完自动取消勾选，防止再点一次重复安装。
                    row.selected.set(false);
                }
            }
            if !message.is_empty() {
                ins.log.update(|l| {
                    l.push_str(&format!("[{}] {}\n", device, message));
                });
            }
        }
        InstallEvent::Log(line) => {
            ins.log.update(|l| {
                l.push_str(&line);
                l.push('\n');
            });
        }
        InstallEvent::Finished { success, failed } => {
            ins.installing.set(false);
            *ins.cancel.borrow_mut() = None;
            if failed == 0 && success > 0 {
                ctx.toast_ok(tr!("install.success"));
            } else if failed > 0 {
                ctx.toast_err(tr!("install.failed"));
            }
        }
    }
}

fn start_install(state: &AppState, ctx: &mut EventCtx) {
    let ins = &state.install;
    let Some(info) = state
        .doc_by_id(ins.doc_id.get())
        .and_then(|d| d.current_info())
    else {
        ctx.toast_err(tr!("install.doc_closed"));
        return;
    };
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
    let selected_splits: Option<Vec<String>> = if info.is_archive() {
        Some(ins.splits.with(|v| {
            v.iter()
                .filter(|s| s.selected.get())
                .map(|s| s.path.clone())
                .collect()
        }))
    } else {
        None
    };
    for id in &devices {
        if let Some(row) = ins
            .devices
            .with(|v| v.iter().find(|r| &r.device.id == id).cloned())
        {
            row.phase.set(None);
            row.message.set(String::new());
        }
    }
    ins.log.set(String::new());
    ins.installing.set(true);
    let cancel = Arc::new(AtomicBool::new(false));
    *ins.cancel.borrow_mut() = Some(cancel.clone());

    let req = InstallRequest {
        adb,
        file: info.path.clone(),
        kind: info.kind,
        devices,
        options,
        selected_splits,
        package_name: info.meta.package_name.clone(),
    };
    let tx = state.tx.clone();
    thread::spawn(move || {
        apkinfo_core::installer::run(req, cancel, |ev| {
            let _ = tx.send(Msg::Install(ev));
        });
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

    let header = Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(8)
        .padding_edges(Insets::new(super::PAGE_PAD, 14, super::PAGE_PAD, 6))
        .child(
            Element::image_content(crate::icons::content("package", crate::icons::ICON_GRAY))
                .size(18, 18),
        )
        .child(
            Element::label(ins.file_name)
                .font_size(14.0)
                .font_weight(600)
                .fg_role(Role::Text)
                .max_lines(1)
                .truncate(Truncate::Middle)
                .weight(1.0),
        );

    // 右侧多留滚动条的位置：滚动条压在内容之上，不留白就会盖住行尾的徽章。
    let body = Element::col()
        .width_match()
        .spacing(super::CARD_GAP)
        .padding_edges(Insets::new(
            super::PAGE_PAD,
            4,
            super::PAGE_PAD + super::SCROLL_GUTTER,
            super::PAGE_PAD,
        ))
        .child(device_section(state))
        .child(options_section(state))
        .child(splits_section(state))
        .child(log_section(state));

    let footer = Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(8)
        .padding_edges(Insets::new(super::PAGE_PAD, 10, super::PAGE_PAD, 12))
        .bg_role(Role::Surface)
        .child(
            Element::button(t!("action.refresh"))
                .small()
                .neutral()
                .outline()
                .enabled_when({
                    let loading = ins.loading_devices;
                    move || !installing.get() && !loading.get()
                })
                .on_click({
                    let st = st.clone();
                    move |ctx| refresh_devices(&st, ctx)
                }),
        )
        .child(Element::flex_spacer())
        .child(
            Element::button(t!("action.close"))
                .small()
                .neutral()
                .outline()
                .visible_when(move || !installing.get())
                .on_click(move |ctx| ctx.request_close()),
        )
        .child(
            Element::button(t!("install.stop"))
                .small()
                .danger()
                .outline()
                .visible_when(move || installing.get())
                .on_click({
                    let confirm = ins.confirm_stop;
                    move |_| confirm.set(true)
                }),
        )
        .child(
            Element::button(t!("action.install"))
                .small()
                .enabled_when({
                    let devices = ins.devices;
                    let splits = ins.splits;
                    move || {
                        !installing.get()
                            && devices.with(|v| {
                                v.iter().any(|r| r.selected.get() && !r.device.is_offline())
                            })
                            // 归档包：一个分包都没勾就没东西可装
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
        .child(header)
        .child(Element::scroll().width_match().weight(1.0).child(body))
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
                            .outline()
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

    Element::stack().fill().child(page).child(confirm_dialog)
}

fn device_section(state: &AppState) -> Element {
    let ins = state.install.clone();
    let loading = ins.loading_devices;
    let error = ins.device_error;
    let devices = ins.devices;

    let list = Element::host_signal(devices, |row: DeviceRow| device_row(row)).width_match();

    let count = Element::label(Signal::map(devices, |v| {
        tr!("install.device_count", count = v.len() as i64)
    }))
    .font_size(12.0)
    .fg_role(Role::TextMuted);
    step_card(
        1,
        tr!("install.select_device"),
        Some(count),
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
        Some(InstallPhase::Success) => tr!("install.success"),
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
    let subtitle = if d.is_wireless() {
        String::new()
    } else {
        d.id.clone()
    };
    Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(10)
        .padding_xy(6, 4)
        .child(Element::checkbox("", row.selected).enabled(!offline))
        .child(
            Element::col()
                .weight(1.0)
                .spacing(2)
                .child(
                    Element::row()
                        .cross(Align::Center)
                        .spacing(8)
                        .child(Element::label(d.display_name()).font_size(13.5).fg_role(
                            if offline {
                                Role::TextDisabled
                            } else {
                                Role::Text
                            },
                        ))
                        .child(status_badge),
                )
                .child(
                    Element::label(subtitle.clone())
                        .font_size(11.5)
                        .fg_role(Role::TextSubtle)
                        .visible(!subtitle.is_empty()),
                ),
        )
        .child(
            Element::label(result_text)
                .font_size(12.5)
                .fg_role_signal(result_role)
                .max_lines(2)
                .width(220),
        )
}

fn options_section(state: &AppState) -> Element {
    let ins = state.install.clone();
    let show = ins.show_advanced;
    Element::col()
        .width_match()
        .bg_role(Role::Surface)
        .corner(super::CARD_CORNER)
        .border_role(Role::Border, 1)
        .padding_xy(4, 0)
        .child(
            Element::collapsible(
                tr!("install.advanced_options"),
                show,
                Element::col()
                    .width_match()
                    .spacing(4)
                    .padding_xy(8, 4)
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
            .width_match(),
        )
}

fn splits_section(state: &AppState) -> Element {
    let ins = state.install.clone();
    let splits = ins.splits;
    let show = ins.show_splits;

    let groups = Element::host_signal(splits, move |row: SplitRow| {
        let is_base = row.kind == SplitType::Base;
        let kind_key = format!("install.split_type.{}", row.kind.key());
        Element::row()
            .width_match()
            .cross(Align::Center)
            .spacing(8)
            .padding_xy(4, 1)
            .child(Element::checkbox(row.path.clone(), row.selected).enabled(!is_base))
            .child(Element::flex_spacer())
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

    let toolbar = Element::row()
        .width_match()
        .spacing(8)
        .padding_xy(4, 2)
        .child(
            Element::button(t!("install.select_all"))
                .small()
                .neutral()
                .outline()
                // 先把行拷出来再写：`with` 期间信号仓持共享借用，闭包里再 set 会撞 RefCell 借用而 panic
                .on_click(move |_| {
                    for s in splits.get() {
                        s.selected.set(true);
                    }
                }),
        )
        .child(
            Element::button(t!("install.base_only"))
                .small()
                .neutral()
                .outline()
                .on_click(move |_| {
                    for s in splits.get() {
                        s.selected.set(s.kind == SplitType::Base);
                    }
                }),
        )
        .child(
            Element::button(t!("install.base_and_language"))
                .small()
                .neutral()
                .outline()
                .on_click(move |_| {
                    for s in splits.get() {
                        s.selected
                            .set(matches!(s.kind, SplitType::Base | SplitType::Language));
                    }
                }),
        );

    let _ = show;
    step_card(
        2,
        tr!("install.split_options"),
        None,
        Element::col()
            .width_match()
            .spacing(4)
            .child(toolbar)
            .child(groups),
    )
    .visible_when(move || splits.with(|v| !v.is_empty()))
}

/// 步骤卡片：圆形序号 + 标题（+ 右侧动作）+ 内容。
fn step_card(n: u32, title: String, action: Option<Element>, body: Element) -> Element {
    let mut head = Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(8)
        .child(
            Element::stack()
                .size(20, 20)
                .corner(10.0)
                .bg_role(Role::Accent)
                .child(
                    Element::label(n.to_string())
                        .font_size(11.0)
                        .font_weight(700)
                        .fg_role(Role::OnAccent)
                        .align_xy(Align::Center, Align::Center),
                ),
        )
        .child(
            Element::label(title)
                .font_size(13.5)
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
        .padding(12)
        .spacing(8)
        .child(head)
        .child(body)
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
