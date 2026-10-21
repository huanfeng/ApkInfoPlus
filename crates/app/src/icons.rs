//! 内嵌的单色 SVG 图标。矢量 + 模板着色，任何 DPI 下都锐利，也不依赖系统字体里有没有某个符号。

use windui::prelude::*;

/// 工具图标的统一灰度（亮暗主题下都可读；要跟随主题换色的地方请用 `tint_role`）。
pub const ICON_GRAY: Color = Color::rgba(0x7A, 0x80, 0x89, 0xFF);

pub fn svg(name: &str) -> &'static [u8] {
    match name {
        "copy" => include_bytes!("../assets/icons/copy.svg"),
        "folder" => include_bytes!("../assets/icons/folder.svg"),
        "globe" => include_bytes!("../assets/icons/globe.svg"),
        "chevron_left" => include_bytes!("../assets/icons/chevron_left.svg"),
        "chevron_right" => include_bytes!("../assets/icons/chevron_right.svg"),
        "package" => include_bytes!("../assets/icons/package.svg"),
        "android" => include_bytes!("../assets/icons/android.svg"),
        _ => include_bytes!("../assets/icons/package.svg"),
    }
}

pub fn content(name: &str, tint: Color) -> ImageContent {
    ImageContent::from_svg_bytes(svg(name), None).tint(tint)
}

/// 方形图标按钮：`px` 是按钮边长，图标约占 60%。
pub fn button(name: &str, px: i32, tooltip: impl Into<String>) -> Element {
    Element::icon_button_content(content(name, ICON_GRAY))
        .size(px, px)
        .tooltip(tooltip)
}
