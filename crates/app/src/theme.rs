//! 主题：亮 / 暗 / 跟随系统 + 用户强调色。

use windui::prelude::*;

use crate::config::{Config, ThemeMode};

/// 按配置与系统偏好算出主题。
pub fn make_theme(cfg: &Config, system_dark: bool) -> Theme {
    let dark = match cfg.theme_mode {
        ThemeMode::Light => false,
        ThemeMode::Dark => true,
        ThemeMode::System => system_dark,
    };
    let mut theme = if dark {
        Theme::dark()
    } else {
        Theme::default()
    };
    if let Some(rgb) = cfg.accent_color {
        apply_accent(&mut theme, Color::hex(rgb));
    }
    // 设置页表单行：标题略大一点，与主窗字段行统一。
    theme.form.label_size = Some(13.5);
    theme.form.desc_size = Some(12.0);
    theme
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
