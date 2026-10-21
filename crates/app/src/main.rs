//! APK Info Plus 桌面应用入口。
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod actions;
mod assoc;
mod config;
mod i18n_tests;
mod icons;
mod logging;
mod state;
mod theme;
mod tools;
mod ui;

use std::path::PathBuf;

use windui::event::Key;
use windui::prelude::*;

use crate::config::Config;
use crate::state::{AppState, Msg};

const APP_ID: &str = "to.feng.apkinfoplus";

fn main() {
    let cfg = Config::load();
    logging::init(cfg.enable_debug);
    log::info!("APK Info Plus {} 启动", env!("CARGO_PKG_VERSION"));
    // 上次强退可能留下几 GB 的 OBB 解压目录
    apkinfo_core::installer::cleanup_stale_temp_dirs();

    // 命令行里存在的受支持文件全部打开；其余参数（--screenshot 等）留给 windui。
    let args: Vec<String> = std::env::args().skip(1).collect();
    let initial_files: Vec<PathBuf> = supported_files(&args);
    let screenshot_mode = args.iter().any(|a| a == "--screenshot");
    // `--lang zh-CN`：调试 / 截图时强制语言，不写进配置。
    let lang_override = args
        .windows(2)
        .find(|w| w[0] == "--lang")
        .map(|w| w[1].clone());
    // `--theme light|dark`：同上，只影响本次运行。
    let theme_override = args
        .windows(2)
        .find(|w| w[0] == "--theme")
        .map(|w| w[1].clone());
    // `--size 1280x800`：同上，截图时指定窗口尺寸。
    let size_override = args
        .windows(2)
        .find(|w| w[0] == "--size")
        .and_then(|w| w[1].split_once('x'))
        .and_then(|(w, h)| Some((w.parse::<i32>().ok()?, h.parse::<i32>().ok()?)));
    let mut cfg = cfg;
    if let Some(l) = &lang_override {
        cfg.language = l.clone();
    }
    if let Some((w, h)) = size_override {
        cfg.window_width = w;
        cfg.window_height = h;
    }
    match theme_override.as_deref() {
        Some("dark") => cfg.theme_mode = config::ThemeMode::Dark,
        Some("light") => cfg.theme_mode = config::ThemeMode::Light,
        _ => {}
    }

    let locales = Locales::builder()
        .embed(include_str!("../i18n/zh-CN.toml"))
        .embed(include_str!("../i18n/en.toml"))
        .embed(include_str!("../i18n/zh-TW.toml"))
        .embed(include_str!("../i18n/zh-HK.toml"))
        .embed(include_str!("../i18n/ja.toml"))
        .embed(include_str!("../i18n/ko.toml"))
        .fallback("en")
        .initial(if cfg.language == "auto" {
            Initial::System
        } else {
            Initial::Fixed(cfg.language.clone())
        })
        .build();

    let theme = theme::make_theme(&cfg, windui::event::system_prefers_dark());

    let mut app = App::new("APK Info Plus", cfg.window_width, cfg.window_height)
        .min_size(900, 600)
        .centered()
        .theme(theme)
        .locales(locales)
        .icon(app_icon());
    // 自绘标题栏（Windows / Linux）。macOS 留系统标题栏：红绿灯与全屏手势都靠它。
    if !cfg!(target_os = "macos") {
        app = app.frameless();
    }
    app = app.renderer(match cfg.renderer {
        config::RendererMode::Auto => Renderer::Auto,
        config::RendererMode::Software => Renderer::Software,
        config::RendererMode::Gpu => Renderer::Gpu,
    });

    let theme_handle = app.theme_handle();
    let locale_handle = app.locale_handle();

    // 消息通道：先建状态占位，通道回调里再借用状态。
    let state_slot: std::rc::Rc<std::cell::RefCell<Option<AppState>>> = Default::default();
    let tx = {
        let slot = state_slot.clone();
        app.channel::<Msg>(move |ctx, msg| {
            let st = slot.borrow().clone();
            if let Some(st) = st {
                actions::handle_msg(&st, ctx, msg);
            }
        })
    };

    let state = AppState::new(cfg, tx.clone(), theme_handle, locale_handle);
    *state_slot.borrow_mut() = Some(state.clone());

    // 第二个实例：把它的参数里受支持的文件转给我们。
    {
        let tx = tx.clone();
        app = app.single_instance(APP_ID, move |argv| {
            let files = supported_files(argv.as_slice());
            if !files.is_empty() {
                let _ = tx.send(Msg::OpenPaths(files));
            }
        });
    }

    // 跟随系统明暗。
    {
        let st = state.clone();
        app = app.on_system_theme_changed(move |_ctx, _dark| {
            if st.cfg(|c| c.theme_mode == config::ThemeMode::System) {
                ui::settings::apply_theme(&st);
            }
        });
    }

    // 窗口级快捷键：没有 ctx，经通道转成消息再处理。
    {
        let st = state.clone();
        let tx = tx.clone();
        app = app.on_shortcut(move |sctx, ev| {
            if !ev.pressed {
                return false;
            }
            let ctrl = ev.ctrl || ev.meta;
            match ev.key {
                Key::Other(0x4F) if ctrl => {
                    let _ = tx.send(Msg::Shortcut(Shortcut::Open));
                    true
                }
                Key::Other(0x57) if ctrl => {
                    let _ = tx.send(Msg::Shortcut(Shortcut::CloseFile));
                    true
                }
                Key::Other(0x49) if ctrl => {
                    let _ = tx.send(Msg::Shortcut(Shortcut::Install));
                    true
                }
                Key::Other(0xBC) if ctrl => {
                    let _ = tx.send(Msg::Shortcut(Shortcut::Settings));
                    true
                }
                Key::F(5) => {
                    let _ = tx.send(Msg::Shortcut(Shortcut::Reparse));
                    true
                }
                Key::F(2) => {
                    let _ = tx.send(Msg::Shortcut(Shortcut::Rename));
                    true
                }
                Key::Tab if ctrl => {
                    let s = if ev.shift {
                        Shortcut::PrevDoc
                    } else {
                        Shortcut::NextDoc
                    };
                    let _ = tx.send(Msg::Shortcut(s));
                    true
                }
                Key::Escape => {
                    // 有对话框时先关对话框；否则交给默认行为（关窗，安装中由 on_close_request 拦）。
                    if st.show_rename.get() {
                        st.show_rename.set(false);
                        return true;
                    }
                    let _ = sctx;
                    false
                }
                _ => false,
            }
        });
    }

    // 安装进行中不允许关窗；放行时顺手把侧栏比例写回配置。
    {
        let st = state.clone();
        app = app.on_close_request(move |ctx| {
            if st.install.installing.get() {
                ctx.toast(tr!("install.installing"));
                return false;
            }
            let ratio = st.sidebar_ratio.get();
            if (st.cfg(|c| c.sidebar_ratio) - ratio).abs() > 0.001 {
                st.update_config(|c| c.sidebar_ratio = ratio);
            }
            true
        });
    }

    if !initial_files.is_empty() {
        if screenshot_mode {
            // 截图模式：同步加载，且与真实打开一样让第一个文件成为当前文档。
            let mut first = None;
            for p in initial_files {
                let id = actions::load_sync(&state, p);
                first.get_or_insert(id);
            }
            if let Some(id) = first {
                state.set_active(id);
            }
        } else {
            let _ = tx.send(Msg::OpenPaths(initial_files));
        }
    }

    let ui = ui::main_window::build(&state);
    app.title(state.window_title)
        .screenshot_from_args()
        .content(ui)
        .run();
}

/// 应用图标：随包分发的位图；解码失败时退回 windui 的品牌图标。
fn app_icon() -> windui::icon::IconSource {
    match Image::from_bytes(include_bytes!("../assets/icon.png")) {
        Ok(img) => match WindowIcon::from_image(&img) {
            Some(icon) => icon.into(),
            None => brand_icon(),
        },
        Err(_) => brand_icon(),
    }
}

/// 参数里存在且受支持的文件，保持顺序。
fn supported_files(args: &[String]) -> Vec<PathBuf> {
    args.iter()
        .map(PathBuf::from)
        .filter(|p| p.is_file() && actions::is_supported_file(p))
        .collect()
}

/// 快捷键意图，经消息通道到有 ctx 的地方执行。
#[derive(Debug, Clone, Copy)]
pub enum Shortcut {
    Open,
    CloseFile,
    Install,
    Rename,
    Reparse,
    Settings,
    NextDoc,
    PrevDoc,
}
