//! 设置窗口：单例子窗，左侧 pill 导航（外观 / 功能 / 工具 / 关于），右侧内容。
//!
//! windui 的下拉 / 开关 / 取色器没有变更回调，设置项的「改了就生效」靠窗口自己的
//! `on_interval` 比对信号版本号：只在设置窗开着时每 120ms 看一眼，什么都没变就不出帧。

use std::path::PathBuf;
use std::thread;

use apkinfo_core::tools::DownloadEvent;
use windui::core::EventCtx;
use windui::prelude::*;

use crate::actions;
use crate::config::{RendererMode, ThemeMode, ToolSource};
use crate::state::{AppState, Msg};

/// 下载 platform-tools 的进度状态，挂在 AppState 上（下载可能比设置窗活得久）。
#[derive(Clone)]
pub struct DownloadState {
    pub running: Signal<bool>,
    pub percent: Signal<f32>,
    pub stage: Signal<String>,
    /// 下载失败的原始错误（空串 = 无错误）；界面用 `t!("settings.download_failed")` 套句子。
    pub message: Signal<String>,
    pub cancel:
        std::rc::Rc<std::cell::RefCell<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>>>,
    /// 下载完成后 +1，让设置页重算 adb 路径显示。
    pub changed: Signal<u64>,
}

impl DownloadState {
    pub fn new() -> Self {
        Self {
            running: signal(false),
            percent: signal(0.0),
            stage: signal(String::new()),
            message: signal(String::new()),
            cancel: Default::default(),
            changed: signal(0),
        }
    }
}

pub fn on_download_event(state: &AppState, ctx: &mut EventCtx, ev: DownloadEvent) {
    let dl = &state.download;
    match ev {
        DownloadEvent::Progress { stage, percent } => {
            dl.stage.set(stage);
            if let Some(p) = percent {
                dl.percent.set(p);
            }
        }
        DownloadEvent::Done(path) => {
            dl.running.set(false);
            dl.percent.set(1.0);
            dl.message.set(String::new());
            dl.changed.update(|n| *n += 1);
            ctx.toast_ok(tr!(
                "settings.download_success",
                path = path.display().to_string()
            ));
        }
        DownloadEvent::Failed(e) => {
            dl.running.set(false);
            dl.message.set(e.clone());
            ctx.toast_err(tr!("settings.download_failed", error = e));
        }
    }
}

pub fn open(state: &AppState, ctx: &mut EventCtx) {
    let st = state.clone();
    ctx.open_window(
        Window::new(tr!("settings.title"), 760, 560)
            .single("settings")
            .centered(true)
            .min_size(640, 460)
            .frameless(!cfg!(target_os = "macos"))
            .title(t!("settings.title"))
            .on_interval(std::time::Duration::from_millis(120), {
                let st = st.clone();
                let mut watcher: Option<Watcher> = None;
                move |ctx| {
                    if watcher.is_none() {
                        watcher = WATCHER.with(|w| w.borrow_mut().take());
                    }
                    if let Some(w) = watcher.as_mut() {
                        w.poll(&st, ctx);
                    }
                }
            })
            .content(move || super::window_chrome(t!("settings.title"), build(&st))),
    );
}

thread_local! {
    /// 设置页构建时把它的信号交到这里，窗口的 `on_interval` 取走。
    /// 两者都在 UI 线程，且 `content` 闭包在定时器首次触发之前执行。
    static WATCHER: std::cell::RefCell<Option<Watcher>> = const { std::cell::RefCell::new(None) };
}

/// 设置页上会被「静默修改」的信号（没有回调的控件），附带上次看到的版本号。
struct Watcher {
    language: (Signal<usize>, u64),
    theme_mode: (Signal<usize>, u64),
    renderer: (Signal<usize>, u64),
    accent: (Signal<Color>, u64),
    title_width: (Signal<f64>, u64),
    adb_source: (Signal<usize>, u64),
    adb_path: (Signal<String>, u64),
    download_source: (Signal<usize>, u64),
    /// 上次看到的 `download.changed` 版本号。
    dl_changed: u64,
    locales: Vec<String>,
    /// 解析出的 adb 路径；空串表示没找到（界面换成 `settings.path_not_found`）。
    resolved: Signal<String>,
    /// 关于页「基于 windui … · 渲染后端 …」里的后端名，只有拿到 ctx 才问得到。
    renderer_name: Signal<String>,
}

impl Watcher {
    fn poll(&mut self, state: &AppState, ctx: &mut EventCtx) {
        if self.renderer_name.with(|f| f.is_empty()) {
            self.renderer_name
                .set(ctx.active_renderer().name().to_string());
        }
        if changed(&mut self.renderer) {
            let mode = RendererMode::ALL[self.renderer.0.get().min(2)];
            if state.cfg(|c| c.renderer) != mode {
                state.update_config(|c| c.renderer = mode);
                ctx.toast(tr!("settings.restart_required"));
            }
        }
        if changed(&mut self.language) {
            let idx = self.language.0.get();
            let lang = if idx == 0 {
                "auto".to_string()
            } else {
                self.locales.get(idx - 1).cloned().unwrap_or_default()
            };
            apply_language(state, &lang);
        }
        if changed(&mut self.theme_mode) {
            let mode = ThemeMode::ALL[self.theme_mode.0.get().min(2)];
            state.update_config(|c| c.theme_mode = mode);
            apply_theme(state);
        }
        if changed(&mut self.accent) {
            let c = self.accent.0.get();
            let rgb = ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32;
            state.update_config(|cfg| cfg.accent_color = Some(rgb));
            apply_theme(state);
        }
        if changed(&mut self.title_width) {
            let w = self.title_width.0.get().round() as i32;
            state.update_config(|c| c.title_width = w);
            state.title_width.set(w);
            // 字段行在构建期读宽度，重建一次当前文档的内容区即可。
            state.touch_active();
        }
        let mut adb_changed = false;
        if changed(&mut self.adb_source) {
            let src = ToolSource::ALL[self.adb_source.0.get().min(2)];
            state.update_config(|c| c.adb_source = src);
            adb_changed = true;
        }
        if changed(&mut self.adb_path) {
            let p = self.adb_path.0.get();
            state.update_config(|c| c.adb_path = p);
            adb_changed = true;
        }
        if changed(&mut self.download_source) {
            // 只在点下载时读取，这里无需动作；保留比对以免反复触发。
        }
        let dl_changed = state.download.changed.version();
        if adb_changed || dl_changed != self.dl_changed {
            self.dl_changed = dl_changed;
            self.resolved.set(resolved_adb_text(state));
        }
    }
}

fn changed<T: 'static>(slot: &mut (Signal<T>, u64)) -> bool {
    let v = slot.0.version();
    if v != slot.1 {
        slot.1 = v;
        true
    } else {
        false
    }
}

fn apply_language(state: &AppState, lang: &str) {
    state.update_config(|c| c.language = lang.to_string());
    if lang == "auto" {
        let sys = windui::platform::system_locales();
        let avail = state.locale.available();
        let pick = sys
            .iter()
            .find_map(|s| {
                avail
                    .iter()
                    .find(|a| a.id.eq_ignore_ascii_case(s) || s.starts_with(&a.id))
            })
            .map(|a| a.id.clone())
            .unwrap_or_else(|| "en".into());
        state.locale.set(&pick);
    } else {
        state.locale.set(lang);
    }
}

pub fn apply_theme(state: &AppState) {
    let theme = state.cfg(|c| crate::theme::make_theme(c, windui::event::system_prefers_dark()));
    state.theme.set(theme);
}

/// 解析出的 adb 路径；找不到时返回空串，由界面显示「未找到」（跟随语言）。
fn resolved_adb_text(state: &AppState) -> String {
    crate::tools::resolve_adb(state)
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// 构建
// ---------------------------------------------------------------------------

fn build(state: &AppState) -> Element {
    let st = state.clone();
    let nav = signal(0usize);

    // ---- 外观 ----
    let locales = state.locale.available();
    let locale_ids: Vec<String> = locales.iter().map(|l| l.id.clone()).collect();
    let mut lang_options: Vec<TextContent> = vec![t!("settings.language_auto").into()];
    lang_options.extend(locales.iter().map(|l| l.name.clone().into()));
    let cfg_lang = state.cfg(|c| c.language.clone());
    let lang_idx = if cfg_lang == "auto" {
        0
    } else {
        locale_ids
            .iter()
            .position(|id| id.eq_ignore_ascii_case(&cfg_lang))
            .map(|i| i + 1)
            .unwrap_or(0)
    };
    let language = signal(lang_idx);
    let theme_mode = signal(state.cfg(|c| c.theme_mode.index()));
    let renderer = signal(state.cfg(|c| c.renderer.index()));
    let renderer_name = signal(String::new());
    let accent_initial = state
        .cfg(|c| c.accent_color)
        .map(Color::hex)
        .unwrap_or(Theme::default().palette.accent);
    let accent = signal(accent_initial);
    let title_width = signal(state.cfg(|c| c.title_width) as f64);

    let appearance = Element::col()
        .width_match()
        .spacing(12)
        .child(Element::card(
            t!("settings.appearance"),
            Element::col()
                .width_match()
                .child(Element::setting_row(
                    t!("settings.language"),
                    Element::dropdown(lang_options, language).width(220),
                ))
                .child(Element::setting_row(
                    t!("settings.theme_mode"),
                    Element::segmented(
                        vec![
                            t!("settings.theme_light"),
                            t!("settings.theme_dark"),
                            t!("settings.theme_system"),
                        ],
                        theme_mode,
                    ),
                ))
                .child(Element::setting_row_desc(
                    t!("settings.renderer"),
                    t!("settings.renderer_hint"),
                    Element::segmented(
                        vec![
                            t!("settings.renderer_auto"),
                            t!("settings.renderer_software"),
                            t!("settings.renderer_gpu"),
                        ],
                        renderer,
                    ),
                ))
                .child(Element::setting_row(
                    t!("settings.accent_color"),
                    Element::row()
                        .spacing(8)
                        .cross(Align::Center)
                        .child(Element::color_picker_opts(
                            accent,
                            ColorPickerOpts::default().alpha(false),
                        ))
                        .child(
                            Element::button(t!("action.reset"))
                                .small()
                                .neutral()
                                .outline_soft()
                                .on_click({
                                    let st = st.clone();
                                    move |_| {
                                        st.update_config(|c| c.accent_color = None);
                                        accent.set(Theme::default().palette.accent);
                                        apply_theme(&st);
                                    }
                                }),
                        ),
                ))
                .child(Element::setting_row(
                    t!("settings.title_width"),
                    Element::stepper(title_width, 60.0, 240.0, 10.0).width(140),
                )),
        ));

    // ---- 功能 ----
    let enable_sig = state.enable_signature;
    let enable_hash = state.enable_hash;
    let enable_debug = signal(state.cfg(|c| c.enable_debug));
    let features = Element::col()
        .width_match()
        .spacing(12)
        .child(Element::card(
            t!("settings.features"),
            Element::col()
                .width_match()
                .child(Element::setting_row_desc(
                    t!("settings.enable_signature"),
                    t!("settings.enable_signature_desc"),
                    Element::checkbox("", enable_sig).on_toggle({
                        let st = st.clone();
                        move |_| {
                            let v = !enable_sig.get();
                            enable_sig.set(v);
                            st.update_config(|c| c.enable_signature = v);
                        }
                    }),
                ))
                .child(Element::setting_row_desc(
                    t!("settings.enable_hash"),
                    t!("settings.enable_hash_desc"),
                    Element::checkbox("", enable_hash).on_toggle({
                        let st = st.clone();
                        move |_| {
                            let v = !enable_hash.get();
                            enable_hash.set(v);
                            st.update_config(|c| c.enable_hash = v);
                        }
                    }),
                ))
                .child(
                    Element::label(t!("settings.restart_hint"))
                        .font_size(12.0)
                        .fg_role(Role::TextSubtle),
                ),
        ))
        .child(file_association_card())
        .child(Element::card(
            t!("settings.enable_debug"),
            Element::col()
                .width_match()
                .spacing(8)
                .child(Element::setting_row_desc(
                    t!("settings.enable_debug"),
                    t!("settings.enable_debug_desc"),
                    Element::checkbox("", enable_debug).on_toggle({
                        let st = st.clone();
                        move |_| {
                            let v = !enable_debug.get();
                            enable_debug.set(v);
                            st.update_config(|c| c.enable_debug = v);
                            crate::logging::set_debug(v);
                        }
                    }),
                ))
                .child(
                    Element::row()
                        .spacing(8)
                        .child(
                            Element::button(t!("settings.open_debug_log"))
                                .small()
                                .neutral()
                                .outline_soft()
                                .on_click(|ctx| {
                                    let p = crate::config::log_path();
                                    if let Err(e) = actions::reveal_in_file_manager(&p) {
                                        ctx.toast_err(e);
                                    }
                                }),
                        )
                        .child(
                            Element::button(t!("settings.open_data_dir"))
                                .small()
                                .neutral()
                                .outline_soft()
                                .on_click(|ctx| {
                                    let p = crate::config::data_dir();
                                    let _ = std::fs::create_dir_all(&p);
                                    if let Err(e) = actions::open_directory(&p) {
                                        ctx.toast_err(e);
                                    }
                                }),
                        ),
                ),
        ));

    // ---- 工具 ----
    let adb_source = signal(state.cfg(|c| c.adb_source.index()));
    let adb_path = signal(state.cfg(|c| c.adb_path.clone()));
    let resolved = signal(resolved_adb_text(state));
    let download_sources = apkinfo_core::tools::default_download_sources();
    let mut source_options: Vec<TextContent> = vec![t!("settings.download_source_auto").into()];
    source_options.extend(download_sources.iter().map(|s| short_url(s).into()));
    let download_source = signal(0usize);
    let dl = state.download.clone();
    let download_dir_text = signal(state.cfg(|c| c.resolved_download_dir().display().to_string()));

    let tools = Element::col()
        .width_match()
        .spacing(12)
        .child(Element::card(
            "adb",
            Element::col()
                .width_match()
                .child(Element::setting_row(
                    t!("settings.adb_source"),
                    Element::dropdown(
                        vec![
                            t!("settings.tool_source_builtin"),
                            t!("settings.tool_source_system"),
                            t!("settings.tool_source_custom"),
                        ],
                        adb_source,
                    )
                    .width(220),
                ))
                .child(Element::setting_row(
                    t!("settings.adb_path"),
                    Element::row()
                        .spacing(8)
                        .cross(Align::Center)
                        .child(Element::text_input(adb_path, t!("settings.select_exe")).width(300))
                        .child(
                            Element::button(t!("action.browse"))
                                .small()
                                .neutral()
                                .outline_soft()
                                .on_click({
                                    move |ctx| {
                                        let dlg =
                                            PickDialog::new().title(tr!("settings.select_exe"));
                                        ctx.request_pick_file(dlg, move |p| {
                                            if let Some(p) = p {
                                                adb_path.set(p.display().to_string());
                                                adb_source.set(ToolSource::Custom.index());
                                            }
                                        });
                                    }
                                }),
                        ),
                ))
                .child(Element::setting_row(
                    t!("settings.resolved_path"),
                    Element::row()
                        .spacing(8)
                        .cross(Align::Center)
                        .child(
                            Element::label(resolved)
                                .font_size(12.5)
                                .fg_role(Role::TextMuted)
                                .max_lines(1)
                                .truncate(Truncate::Middle)
                                .width(300)
                                .visible_when(move || resolved.with(|s| !s.is_empty())),
                        )
                        .child(
                            Element::label(t!("settings.path_not_found"))
                                .font_size(12.5)
                                .fg_role(Role::TextMuted)
                                .width(300)
                                .visible_when(move || resolved.with(|s| s.is_empty())),
                        )
                        .child(
                            Element::button(t!("settings.auto_detect"))
                                .small()
                                .neutral()
                                .outline_soft()
                                .on_click({
                                    let st = st.clone();
                                    move |_| {
                                        let src = st.cfg(|c| {
                                            crate::tools::auto_detect_source(
                                                &c.adb_path,
                                                &c.resolved_download_dir(),
                                            )
                                        });
                                        adb_source.set(src.index());
                                    }
                                }),
                        ),
                )),
        ))
        .child(Element::card(
            t!("settings.download_platform_tools"),
            Element::col()
                .width_match()
                .spacing(6)
                .child(Element::setting_row(
                    t!("settings.download_source"),
                    Element::dropdown(source_options, download_source).width(300),
                ))
                .child(Element::setting_row(
                    t!("settings.download_dir"),
                    Element::row()
                        .spacing(8)
                        .cross(Align::Center)
                        .child(
                            Element::label(download_dir_text)
                                .font_size(12.5)
                                .fg_role(Role::TextMuted)
                                .max_lines(1)
                                .truncate(Truncate::Middle)
                                .width(260),
                        )
                        .child(
                            Element::button(t!("action.browse"))
                                .small()
                                .neutral()
                                .outline_soft()
                                .on_click({
                                    let st = st.clone();
                                    move |ctx| {
                                        let dlg =
                                            PickDialog::new().title(tr!("settings.download_dir"));
                                        let st = st.clone();
                                        ctx.request_pick_folder(dlg, move |p| {
                                            if let Some(p) = p {
                                                st.update_config(|c| {
                                                    c.download_dir = p.display().to_string()
                                                });
                                                download_dir_text.set(p.display().to_string());
                                                st.download.changed.update(|n| *n += 1);
                                            }
                                        });
                                    }
                                }),
                        )
                        .child(
                            Element::button(t!("action.reset"))
                                .small()
                                .neutral()
                                .outline_soft()
                                .on_click({
                                    let st = st.clone();
                                    move |_| {
                                        st.update_config(|c| c.download_dir.clear());
                                        download_dir_text.set(st.cfg(|c| {
                                            c.resolved_download_dir().display().to_string()
                                        }));
                                        st.download.changed.update(|n| *n += 1);
                                    }
                                }),
                        )
                        .child(
                            Element::button(t!("settings.open_download_dir"))
                                .small()
                                .neutral()
                                .outline_soft()
                                .on_click({
                                    let st = st.clone();
                                    move |ctx| {
                                        let p = st.cfg(|c| c.resolved_download_dir());
                                        let _ = std::fs::create_dir_all(&p);
                                        if let Err(e) = actions::open_directory(&p) {
                                            ctx.toast_err(e);
                                        }
                                    }
                                }),
                        ),
                ))
                .child(
                    Element::row()
                        .width_match()
                        .spacing(10)
                        .cross(Align::Center)
                        .child(
                            Element::button(t!("settings.download_start"))
                                .small()
                                .enabled_when({
                                    let running = dl.running;
                                    move || !running.get()
                                })
                                .on_click({
                                    let st = st.clone();
                                    let sources = download_sources.clone();
                                    move |_| start_download(&st, download_source.get(), &sources)
                                }),
                        )
                        .child(
                            Element::button(t!("action.stop"))
                                .small()
                                .danger()
                                .outline_soft()
                                .visible_when({
                                    let running = dl.running;
                                    move || running.get()
                                })
                                .on_click({
                                    let dl = dl.clone();
                                    move |_| {
                                        if let Some(c) = dl.cancel.borrow().as_ref() {
                                            c.store(true, std::sync::atomic::Ordering::SeqCst);
                                        }
                                    }
                                }),
                        )
                        .child(
                            Element::button(t!("settings.delete_downloads"))
                                .small()
                                .danger()
                                .outline_soft()
                                .enabled_when({
                                    let running = dl.running;
                                    move || !running.get()
                                })
                                .on_click({
                                    let st = st.clone();
                                    move |ctx| {
                                        let dir = st.cfg(|c| c.resolved_download_dir());
                                        match apkinfo_core::tools::delete_platform_tools(&dir) {
                                            Ok(()) => {
                                                ctx.toast_ok(tr!("settings.deleted"));
                                                st.download.changed.update(|n| *n += 1);
                                            }
                                            Err(e) => ctx.toast_err(e),
                                        }
                                    }
                                }),
                        ),
                )
                .child(
                    Element::col()
                        .width_match()
                        .spacing(4)
                        .child(Element::progress(dl.percent).width_match())
                        .child(
                            Element::label(Signal::map(dl.stage, {
                                let percent = dl.percent;
                                move |s| format!("{}  {:.0}%", s, percent.get() * 100.0)
                            }))
                            .font_size(12.0)
                            .fg_role(Role::TextMuted),
                        )
                        .visible_when({
                            let running = dl.running;
                            move || running.get()
                        }),
                )
                .child(
                    Element::label(t!("settings.download_failed", error = dl.message))
                        .font_size(12.0)
                        .fg_role(Role::Danger)
                        .width_match()
                        .visible_when({
                            let m = dl.message;
                            move || m.with(|s| !s.is_empty())
                        }),
                ),
        ))
        .child(super::libs::rules_card(state));

    // ---- 关于 ----
    let about = Element::col()
        .width_match()
        .spacing(12)
        .child(Element::card(
            t!("settings.about"),
            Element::col()
                .width_match()
                .spacing(8)
                .child(
                    Element::label("APK Info Plus")
                        .font_size(20.0)
                        .font_weight(700)
                        .fg_role(Role::Text),
                )
                .child(
                    Element::label(t!("settings.version", version = env!("CARGO_PKG_VERSION")))
                        .fg_role(Role::TextMuted),
                )
                .child(
                    Element::label(t!("about.description"))
                        .fg_role(Role::Text)
                        .width_match(),
                )
                .child(Element::label(t!("about.author")).fg_role(Role::TextMuted))
                .child(Element::label(t!("about.license")).fg_role(Role::TextMuted))
                .child(Element::link(t!("settings.homepage")).url(env!("CARGO_PKG_REPOSITORY")))
                .child(
                    Element::label(t!(
                        "settings.framework",
                        version = windui::VERSION,
                        renderer = renderer_name
                    ))
                    .font_size(12.0)
                    .fg_role(Role::TextSubtle)
                    .visible_when(move || renderer_name.with(|s| !s.is_empty())),
                )
                .children(crate::libs::rules().map(|r| {
                    Element::row()
                        .spacing(4)
                        .cross(Align::Center)
                        .child(
                            Element::label(t!(
                                "about.rules",
                                version = r.meta().data_version as i64
                            ))
                            .font_size(12.0)
                            .fg_role(Role::TextSubtle),
                        )
                        .child(
                            Element::link("LibChecker-Rules")
                                .url(super::libs::RULES_HOME)
                                .font_size(12.0),
                        )
                        .child(
                            Element::label("(Apache-2.0)")
                                .font_size(12.0)
                                .fg_role(Role::TextSubtle),
                        )
                }))
                .child(
                    Element::row()
                        .spacing(6)
                        .cross(Align::Center)
                        .child(
                            Element::label(t!("settings.config_file"))
                                .font_size(12.0)
                                .fg_role(Role::TextSubtle),
                        )
                        .child(
                            Element::label(crate::config::config_path().display().to_string())
                                .font_size(12.0)
                                .fg_role(Role::TextSubtle)
                                .max_lines(1)
                                .truncate(Truncate::Middle)
                                .weight(1.0),
                        ),
                ),
        ));

    WATCHER.with(|w| {
        *w.borrow_mut() = Some(Watcher {
            language: (language, language.version()),
            theme_mode: (theme_mode, theme_mode.version()),
            renderer: (renderer, renderer.version()),
            accent: (accent, accent.version()),
            title_width: (title_width, title_width.version()),
            adb_source: (adb_source, adb_source.version()),
            adb_path: (adb_path, adb_path.version()),
            download_source: (download_source, download_source.version()),
            dl_changed: state.download.changed.version(),
            locales: locale_ids,
            resolved,
            renderer_name,
        })
    });

    let sidebar = Element::col()
        .width(170)
        .height_match()
        .padding(10)
        .bg_role(Role::Surface)
        .child(
            Element::list_pill(
                vec![
                    t!("settings.appearance"),
                    t!("settings.features"),
                    t!("settings.tools"),
                    t!("settings.about"),
                ],
                nav,
            )
            .width_match(),
        );

    let pages = |i: usize, e: Element| e.visible_when(move || nav.get() == i);
    let content = Element::scroll().fill().child(
        Element::col()
            .width_match()
            .padding_edges(Insets::new(12, 12, 12 + super::SCROLL_GUTTER, 12))
            .child(pages(0, appearance))
            .child(pages(1, features))
            .child(pages(2, tools))
            .child(pages(3, about)),
    );

    Element::row()
        .fill()
        .bg_role(Role::Bg)
        .child(sidebar)
        .child(Element::divider())
        .child(content.weight(1.0))
}

/// 「文件关联」卡片：注册 / 取消，以及跳到系统「默认应用」设置（Win10+ 用户选过别的程序时只能去那里改）。
fn file_association_card() -> Element {
    let registered = signal(crate::assoc::is_registered());
    let exts = crate::assoc::EXTENSIONS
        .iter()
        .map(|e| format!(".{}", e))
        .collect::<Vec<_>>()
        .join(" / ");
    let mut buttons = Element::row()
        .spacing(8)
        .child(
            Element::button(t!("settings.assoc_register"))
                .small()
                .on_click(move |ctx| match crate::assoc::register() {
                    Ok(()) => {
                        registered.set(crate::assoc::is_registered());
                        ctx.toast_ok(tr!("settings.assoc_done"));
                    }
                    Err(e) => ctx.toast_err(e),
                }),
        )
        .child(
            Element::button(t!("settings.assoc_unregister"))
                .small()
                .neutral()
                .outline_soft()
                .on_click(move |ctx| match crate::assoc::unregister() {
                    Ok(()) => registered.set(crate::assoc::is_registered()),
                    Err(e) => ctx.toast_err(e),
                }),
        );
    if cfg!(target_os = "windows") {
        buttons = buttons.child(
            Element::button(t!("settings.assoc_open_system"))
                .small()
                .neutral()
                .outline_soft()
                .on_click(|ctx| {
                    if let Err(e) = crate::assoc::open_system_settings() {
                        ctx.toast_err(e);
                    }
                }),
        );
    }
    let body = if crate::assoc::is_supported() {
        Element::col()
            .width_match()
            .spacing(8)
            .child(Element::setting_row_desc(
                t!("settings.assoc_types"),
                t!("settings.assoc_desc"),
                Element::label(exts)
                    .font_size(13.0)
                    .fg_role(Role::TextMuted),
            ))
            .child(Element::setting_row(
                t!("settings.assoc_status"),
                Element::row()
                    .child(
                        Element::label(t!("settings.assoc_status_on"))
                            .font_size(13.0)
                            .fg_role(Role::TextMuted)
                            .visible_when(move || registered.get()),
                    )
                    .child(
                        Element::label(t!("settings.assoc_status_off"))
                            .font_size(13.0)
                            .fg_role(Role::TextMuted)
                            .visible_when(move || !registered.get()),
                    ),
            ))
            .child(buttons)
    } else {
        Element::col().width_match().child(
            Element::label(t!("settings.assoc_unsupported"))
                .font_size(12.5)
                .fg_role(Role::TextMuted),
        )
    };
    Element::card(t!("settings.file_association"), body)
}

fn short_url(url: &str) -> String {
    let trimmed = url
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let mut parts: Vec<&str> = trimmed.split('/').collect();
    if parts.len() > 3 {
        let host = parts.remove(0);
        let tail: Vec<&str> = parts.iter().rev().take(2).rev().cloned().collect();
        format!("{}/…/{}", host, tail.join("/"))
    } else {
        trimmed.to_string()
    }
}

fn start_download(state: &AppState, source_index: usize, sources: &[String]) {
    let dl = &state.download;
    if dl.running.get() {
        return;
    }
    let dir: PathBuf = state.cfg(|c| c.resolved_download_dir());
    let chosen: Vec<String> = if source_index == 0 {
        sources.to_vec()
    } else {
        sources.get(source_index - 1).cloned().into_iter().collect()
    };
    dl.running.set(true);
    dl.percent.set(0.0);
    dl.stage.set(String::new());
    dl.message.set(String::new());
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    *dl.cancel.borrow_mut() = Some(cancel.clone());
    let tx = state.tx.clone();
    thread::spawn(move || {
        apkinfo_core::tools::download_platform_tools(&dir, &chosen, cancel, |ev| {
            let _ = tx.send(Msg::Download(ev));
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use windui::geometry::{Rect, Size};
    use windui::render::{Canvas, Fit, Image, Paint};

    /// 只记下画过哪些字的画布。
    #[derive(Default)]
    struct TextCanvas {
        texts: RefCell<Vec<String>>,
    }

    impl Canvas for TextCanvas {
        fn dpi_scale(&self) -> f32 {
            1.0
        }
        fn fill_rect(&mut self, _: f32, _: f32, _: f32, _: f32, _: &Paint) {}
        fn fill_round_rect(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: &Paint) {}
        fn stroke_round_rect(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32, _: &Paint) {
        }
        fn draw_line(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: &Paint) {}
        fn fill_circle(&mut self, _: f32, _: f32, _: f32, _: &Paint) {}
        fn draw_shadow(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32, _: Color) {}
        fn draw_image(&mut self, _: &Image, _: Rect, _: Fit, _: f32, _: f32) {}
        fn draw_text(
            &mut self,
            text: &str,
            _: Rect,
            _: Color,
            _: Align,
            _: &windui::text::TextStyle,
        ) {
            self.texts.borrow_mut().push(text.to_string());
        }
        fn measure_text(&mut self, _: &str, _: &windui::text::TextStyle) -> Size {
            Size::ZERO
        }
        fn push_layer(&mut self, _: f32) {}
        fn pop_layer(&mut self) {}
        fn save(&mut self) {}
        fn restore(&mut self) {}
        fn clip_rect(&mut self, _: Rect) {}
    }

    /// 设置窗的标题栏 + 「文件关联」卡片（卡片标题、设置项、状态文案、按钮）：**同一棵树**
    /// 换语言后再画一帧，画出的全是新语言，旧语言一个字不剩。构建期用 `tr!` 定格的文案
    /// 过不了这一关（重建树才能变）。整页 `build` 要 `AppState`，而后者的 `Sender` 在测试里
    /// 造不出来，所以挑不依赖状态的这一块做代表。
    #[test]
    fn settings_follow_language_switch() {
        windui::i18n::install(
            Locales::builder()
                .embed(include_str!("../../i18n/zh-CN.toml"))
                .embed(include_str!("../../i18n/en.toml"))
                .fallback("en")
                .initial(Initial::Fixed("zh-CN".into()))
                .build(),
        );
        let locale = LocaleHandle::new();
        let mut tree = windui::core::Tree::new();
        let ui = super::super::window_chrome(t!("settings.title"), file_association_card());
        let root = ui.build(&mut tree);
        tree.root = Some(root);
        let paint = |tree: &mut windui::core::Tree| {
            tree.layout_root(Size::new(760, 600), &mut windui::text::NullTextEngine);
            let mut cv = TextCanvas::default();
            tree.paint(&mut cv);
            cv.texts.into_inner()
        };
        let has = |texts: &[String], s: &str| texts.iter().any(|t| t == s);

        // macOS 用系统标题栏（不画自绘标题），也不支持在这里注册文件关联：只断言该平台真会画的字
        let mac = cfg!(target_os = "macos");
        let assoc = crate::assoc::is_supported();
        let expect = |all: &[&'static str], titled: &'static str, assoc_only: &[&'static str]| {
            let mut v: Vec<&'static str> = all.to_vec();
            if !mac {
                v.push(titled);
            }
            if assoc {
                v.extend_from_slice(assoc_only);
            }
            v
        };

        let zh = paint(&mut tree);
        for want in expect(
            &["文件关联"],
            "设置",
            &["文件类型", "状态", "关联到本程序", "取消关联"],
        ) {
            assert!(has(&zh, want), "中文界面缺「{want}」: {zh:?}");
        }
        if assoc {
            assert!(has(&zh, "已关联到本程序") || has(&zh, "未关联"), "{zh:?}");
        }

        assert!(locale.set("en"));
        let en = paint(&mut tree);
        for want in expect(&["File association"], "Settings", &["File types", "Status"]) {
            assert!(has(&en, want), "英文界面缺「{want}」: {en:?}");
        }
        if assoc {
            assert!(
                has(&en, "Associated with this app") || has(&en, "Not associated"),
                "{en:?}"
            );
        }
        for stale in zh
            .iter()
            .filter(|t| t.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)))
        {
            assert!(!has(&en, stale), "换语言后仍残留「{stale}」: {en:?}");
        }
    }
}
