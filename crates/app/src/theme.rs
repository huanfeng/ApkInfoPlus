//! 主题：亮 / 暗 / 跟随系统 + 用户强调色。
//!
//! 调色板按设计稿定：浅灰蓝的窗口底、纯白卡片、饱和一点的蓝作强调色、
//! 文字用偏冷的深灰；暗色按同一套语义镜像。

use windui::prelude::*;

use crate::config::{Config, ThemeMode};

/// 卡片标题图标用的四种语义色（亮暗主题共用，底色取 12% 透明度）。
pub const CARD_BLUE: Color = Color::rgba(0x2F, 0x7B, 0xF5, 0xFF);
pub const CARD_GREEN: Color = Color::rgba(0x22, 0xC5, 0x5E, 0xFF);
pub const CARD_PURPLE: Color = Color::rgba(0x7C, 0x3A, 0xED, 0xFF);
pub const CARD_ORANGE: Color = Color::rgba(0xF5, 0x9E, 0x0B, 0xFF);

fn light_palette() -> Palette {
    Palette {
        accent: Color::hex(0x2F7BF5),
        accent_hover: Color::hex(0x4A8EF7),
        accent_active: Color::hex(0x2566D6),
        bg: Color::hex(0xF4F6FA),
        surface: Color::hex(0xFFFFFF),
        surface_alt: Color::hex(0xF6F8FC),
        text: Color::hex(0x1F2937),
        text_muted: Color::hex(0x6B7280),
        text_subtle: Color::hex(0x9CA3AF),
        text_disabled: Color::hex(0xC0C6CF),
        border: Color::hex(0xE3E8EF),
        track: Color::hex(0xE3E8EF),
        divider: Color::hex(0xEEF1F5),
        placeholder: Color::hex(0xA3AAB5),
        success: Color::hex(0x16A34A),
        warning: Color::hex(0xF59E0B),
        danger: Color::hex(0xEF4444),
        ..Palette::default()
    }
}

fn dark_palette() -> Palette {
    Palette {
        accent: Color::hex(0x3B82F6),
        accent_hover: Color::hex(0x5B95F7),
        accent_active: Color::hex(0x2F6FDB),
        bg: Color::hex(0x0F172A),
        surface: Color::hex(0x1B2437),
        surface_alt: Color::hex(0x232D42),
        text: Color::hex(0xE5E7EB),
        text_muted: Color::hex(0x9AA3B2),
        text_subtle: Color::hex(0x76808F),
        text_disabled: Color::hex(0x56607A),
        border: Color::hex(0x2B3650),
        track: Color::hex(0x2B3650),
        divider: Color::hex(0x263047),
        placeholder: Color::hex(0x6B7589),
        success: Color::hex(0x22C55E),
        warning: Color::hex(0xF59E0B),
        danger: Color::hex(0xF87171),
        ..Palette::dark()
    }
}

/// 按配置与系统偏好算出主题。
pub fn make_theme(cfg: &Config, system_dark: bool) -> Theme {
    let dark = is_dark(cfg, system_dark);
    let mut theme = if dark {
        Theme::dark()
    } else {
        Theme::default()
    };
    theme.palette = if dark {
        dark_palette()
    } else {
        light_palette()
    };
    if let Some(rgb) = cfg.accent_color {
        apply_accent(&mut theme, Color::hex(rgb));
    }
    // 圆角统一偏大一点：卡片 12、控件 8。
    theme.metrics.corner_md = 8.0;
    theme.metrics.corner_lg = 12.0;
    // 设置页表单行：标题略大一点，与主窗字段行统一。
    theme.form.label_size = Some(13.5);
    theme.form.desc_size = Some(12.0);
    theme
}

pub fn is_dark(cfg: &Config, system_dark: bool) -> bool {
    match cfg.theme_mode {
        ThemeMode::Light => false,
        ThemeMode::Dark => true,
        ThemeMode::System => system_dark,
    }
}

/// 强调色及其派生（悬停 / 按下）一起换，否则按钮悬停时会跳回默认蓝。
pub fn apply_accent(theme: &mut Theme, accent: Color) {
    theme.palette.accent = accent;
    theme.palette.accent_hover = lighten(accent, 0.08);
    theme.palette.accent_active = lighten(accent, -0.10);
}

fn lighten(c: Color, amount: f32) -> Color {
    let f = |v: u8| -> u8 {
        let x = v as f32 / 255.0;
        let y = if amount >= 0.0 {
            x + (1.0 - x) * amount
        } else {
            x * (1.0 + amount)
        };
        (y.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    Color::rgba(f(c.r), f(c.g), f(c.b), c.a)
}
