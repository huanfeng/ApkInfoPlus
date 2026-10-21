//! 内嵌的单色 SVG 图标。矢量 + 模板着色，任何 DPI 下都锐利，也不依赖系统字体里有没有某个符号。
//!
//! 线条风格统一取自 Lucide（24 格、2px 描边、圆角端点）。

use windui::prelude::*;

/// 工具图标的统一灰度（亮暗主题下都可读；要跟随主题换色的地方请用 `tint_role`）。
pub const ICON_GRAY: Color = Color::rgba(0x7A, 0x80, 0x89, 0xFF);
/// 强调色底上的白色图标（主按钮）。
pub const ICON_WHITE: Color = Color::rgba(0xFF, 0xFF, 0xFF, 0xFF);

pub fn svg(name: &str) -> &'static [u8] {
    match name {
        "copy" => include_bytes!("../assets/icons/copy.svg"),
        "folder" => include_bytes!("../assets/icons/folder.svg"),
        "folder_open" => include_bytes!("../assets/icons/folder_open.svg"),
        "globe" => include_bytes!("../assets/icons/globe.svg"),
        "chevron_left" => include_bytes!("../assets/icons/chevron_left.svg"),
        "chevron_right" => include_bytes!("../assets/icons/chevron_right.svg"),
        "chevron_down" => include_bytes!("../assets/icons/chevron_down.svg"),
        "package" => include_bytes!("../assets/icons/package.svg"),
        "android" => include_bytes!("../assets/icons/android.svg"),
        "refresh" => include_bytes!("../assets/icons/refresh.svg"),
        "smartphone" => include_bytes!("../assets/icons/smartphone.svg"),
        "pencil" => include_bytes!("../assets/icons/pencil.svg"),
        "image_down" => include_bytes!("../assets/icons/image_down.svg"),
        "x" => include_bytes!("../assets/icons/x.svg"),
        "settings" => include_bytes!("../assets/icons/settings.svg"),
        "search" => include_bytes!("../assets/icons/search.svg"),
        "info" => include_bytes!("../assets/icons/info.svg"),
        "key" => include_bytes!("../assets/icons/key.svg"),
        "layers" => include_bytes!("../assets/icons/layers.svg"),
        "shield" => include_bytes!("../assets/icons/shield.svg"),
        "hash" => include_bytes!("../assets/icons/hash.svg"),
        "file_text" => include_bytes!("../assets/icons/file_text.svg"),
        "play" => include_bytes!("../assets/icons/play.svg"),
        "check_circle" => include_bytes!("../assets/icons/check_circle.svg"),
        "cpu" => include_bytes!("../assets/icons/cpu.svg"),
        _ => include_bytes!("../assets/icons/package.svg"),
    }
}

pub fn content(name: &str, tint: Color) -> ImageContent {
    ImageContent::from_svg_bytes(svg(name), None).tint(tint)
}

/// 工具栏 / 标签页用的灰色图标。
pub fn gray(name: &str) -> ImageContent {
    content(name, ICON_GRAY)
}

/// 方形图标按钮：`px` 是按钮边长，图标约占 60%。
pub fn button(name: &str, px: i32, tooltip: impl Into<String>) -> Element {
    Element::icon_button_content(content(name, ICON_GRAY))
        .size(px, px)
        .tooltip(tooltip)
}
