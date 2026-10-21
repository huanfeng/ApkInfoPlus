//! 界面组装。每个子模块导出一个 `build_*` 函数，返回 `Element`。
//!
//! 共用的小部件（字段行、带图标的分区卡片、可选中文本、复制按钮）放在这里，保证各页视觉一致。

pub mod header;
pub mod install;
pub mod main_window;
pub mod pages;
pub mod rename;
pub mod settings;

use windui::prelude::*;
use windui::ui::inputs::TextInput;

use crate::state::AppState;

/// 内容区四周留白。
pub const PAGE_PAD: i32 = 12;
/// 卡片之间的间距。
pub const CARD_GAP: i32 = 12;
/// 卡片内边距。
pub const CARD_PAD: i32 = 14;
/// 滚动容器右侧额外留白：windui 的滚动条压在内容之上（约 10px），不让位就会盖住行尾。
pub const SCROLL_GUTTER: i32 = 10;
/// 卡片圆角。
pub const CARD_CORNER: f32 = 12.0;

/// 可选中、可复制的单行文本：无框只读输入框。过长时随光标横向滚动，不撑破布局；
/// 右键有内建的复制菜单。
pub fn selectable(text: impl Into<String>) -> Element {
    selectable_signal(signal(text.into()))
}

/// 单行字段值的高度：与纯 label 行对齐（label 约 18px + 上下各 3px）。
pub const VALUE_H: i32 = 24;

pub fn selectable_signal(text: Signal<String>) -> Element {
    Element::text_input(text, "")
        .read_only()
        .frameless()
        // 光标放在开头：超长内容默认露出头部而不是尾部。
        .select_range(0, 0)
        .font_size(13.0)
        .fg_role(Role::Text)
        .width_match()
        .height(VALUE_H)
}

/// 文件格式徽章：APK / XAPK / APKM / APKS 各一种固定颜色，侧栏与概览头共用。
pub fn kind_badge(kind: apkinfo_core::info::ArchiveKind) -> Element {
    use apkinfo_core::info::ArchiveKind;
    let color = match kind {
        ArchiveKind::Apk => crate::theme::CARD_BLUE,
        ArchiveKind::Xapk => crate::theme::CARD_PURPLE,
        ArchiveKind::Apkm => crate::theme::CARD_ORANGE,
        ArchiveKind::Apks => crate::theme::CARD_GREEN,
        _ => crate::icons::ICON_GRAY,
    };
    Element::badge_intent(kind.display_name(), Intent::Custom(color))
}

/// 子窗口的自绘标题栏（与主窗同一套）：图标 + 标题 + 关闭。macOS 用系统标题栏，原样返回。
pub fn window_chrome(title: impl Into<TextContent>, body: Element) -> Element {
    if cfg!(target_os = "macos") {
        return body;
    }
    let bar = Element::row()
        .width_match()
        .height(40)
        .cross(Align::Stretch)
        .bg_role(Role::Surface)
        .window_drag()
        .child(
            Element::row()
                .cross(Align::Center)
                .spacing(8)
                .padding_edges(Insets::new(14, 0, 0, 0))
                .child(
                    Element::image_content(ImageContent::from_bytes(include_bytes!(
                        "../../assets/icon.png"
                    )))
                    .size(18, 18),
                )
                .child(
                    Element::label(title)
                        .font_size(14.0)
                        .font_weight(600)
                        .fg_role(Role::Text),
                )
                .weight(1.0),
        )
        .child(Element::window_button(WindowButtonKind::Close).fg_role(Role::TextMuted));
    Element::col()
        .fill()
        .child(bar)
        .child(Element::divider())
        .child(body.weight(1.0))
}

/// 多行版本：`lines` 为显示行数（内容更多时可滚动）。
pub fn selectable_lines(text: impl Into<String>, lines: usize) -> Element {
    let mut ti = TextInput::new(signal(text.into()), "");
    {
        let c = ti.config_mut();
        c.read_only = true;
        c.frameless = true;
        c.multiline = true;
        c.wrap = true;
    }
    // 每行约 20px，外加输入框自己的上下内边距。
    Element::leaf()
        .widget(ti)
        .font_size(13.0)
        .fg_role(Role::Text)
        .width_match()
        .height((lines.max(1) as i32) * 20 + 14)
}

/// 非输入框的字段值（占位、徽章行、纯 label）：补上与无框输入框内边距等宽的左边距，
/// 让各行的值从同一条竖线起笔。
pub fn plain_value(el: Element) -> Element {
    Element::row()
        .cross(Align::Center)
        .padding_edges(Insets::new(VALUE_PAD_X, 0, 0, 0))
        .child(el)
}

/// 无框输入框的文字相对控件左缘的内边距。
pub const VALUE_PAD_X: i32 = 10;

/// 给任意元素挂一个「复制」右键菜单（标题、徽章这类不是输入框的文本）。
pub fn with_copy_menu(el: Element, text: String) -> Element {
    el.on_context_menu(move || {
        let t = text.clone();
        vec![windui::event::MenuItem::run(
            tr!("action.copy"),
            move |ctx| {
                ctx.clipboard_set(&t);
                ctx.toast_ok(tr!("action.copied", content = shorten(&t, 60)));
            },
            false,
        )]
    })
}

/// 「标题 | 值」一行，值可选中复制；空值显示占位。
pub fn field_row(
    state: &AppState,
    title: impl Into<TextContent>,
    value: impl Into<String>,
) -> Element {
    let value: String = value.into();
    if value.is_empty() {
        return field_row_custom(
            state,
            title,
            plain_value(
                Element::label(t!("field.none"))
                    .font_size(13.0)
                    .fg_role(Role::TextSubtle)
                    .height(VALUE_H),
            ),
            None,
        );
    }
    let lines = value.lines().count();
    let body = if lines > 1 {
        selectable_lines(value, lines.min(6))
    } else {
        selectable(value)
    };
    field_row_custom(state, title, body, None)
}

/// 同 [`field_row`]，行尾多一个复制按钮（包名、路径这类最常复制的字段）。
pub fn field_row_copy(
    state: &AppState,
    title: impl Into<TextContent>,
    value: impl Into<String>,
) -> Element {
    let value: String = value.into();
    if value.is_empty() {
        return field_row(state, title, value);
    }
    let lines = value.lines().count();
    let body = if lines > 1 {
        selectable_lines(value.clone(), lines.min(6))
    } else {
        selectable(value.clone())
    };
    field_row_custom(state, title, body, Some(value))
}

/// 「标题 | 任意元素 | （可选）复制」一行。
pub fn field_row_custom(
    state: &AppState,
    title: impl Into<TextContent>,
    value: Element,
    copy: Option<String>,
) -> Element {
    let width = state.title_width.get();
    let mut row = Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(10)
        .padding_xy(0, 3)
        .child(
            Element::label(title)
                .width(width)
                .font_size(13.0)
                .fg_role(Role::TextMuted),
        )
        .child(value.weight(1.0));
    if let Some(v) = copy.filter(|v| !v.is_empty()) {
        row = row.child(copy_button(v));
    }
    row
}

/// 复制到剪贴板的小图标按钮，带 toast 反馈。
pub fn copy_button(value: String) -> Element {
    crate::icons::button("copy", 24, tr!("action.copy")).on_click(move |ctx| {
        ctx.clipboard_set(&value);
        ctx.toast_ok(tr!("action.copied", content = shorten(&value, 60)));
    })
}

/// 卡片标题前的小图标：淡色圆角方块 + 语义色线条图标。
pub fn card_icon(name: &str, color: Color) -> Element {
    Element::stack()
        .size(28, 28)
        .corner(8.0)
        .bg(color.scale_alpha(0.12))
        .child(
            Element::image_content(crate::icons::content(name, color))
                .size(16, 16)
                .align_xy(Align::Center, Align::Center),
        )
}

/// 分区卡片：图标 + 标题 + 内容。
pub fn section(icon: &str, color: Color, title: impl Into<TextContent>, body: Element) -> Element {
    section_with_action(icon, color, title, None, body)
}

/// 分区卡片，标题行右侧可放一个动作（「复制全部」/「查看详情」）。
pub fn section_with_action(
    icon: &str,
    color: Color,
    title: impl Into<TextContent>,
    action: Option<Element>,
    body: Element,
) -> Element {
    let mut head = Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(10)
        .child(card_icon(icon, color))
        .child(
            Element::label(title)
                .font_size(15.0)
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
        .corner(CARD_CORNER)
        .border_role(Role::Border, 1)
        .padding(CARD_PAD)
        .spacing(8)
        .child(head)
        .child(Element::divider())
        .child(body)
}

/// 页面骨架：可滚动 + 统一内边距。
pub fn page(body: Element) -> Element {
    Element::scroll().fill().child(
        Element::col()
            .width_match()
            .padding_edges(Insets::new(
                PAGE_PAD,
                CARD_GAP,
                PAGE_PAD + SCROLL_GUTTER,
                PAGE_PAD,
            ))
            .spacing(CARD_GAP)
            .child(body),
    )
}

/// 等宽、可选中复制的多行文本块（哈希、文本信息、签名指纹）。
pub fn mono_block(text: Signal<String>, min_height: i32) -> Element {
    Element::text_input(text, "")
        .multiline()
        .read_only()
        .wrap(false)
        .select_range(0, 0)
        .font_family(mono_family())
        .font_size(12.5)
        .width_match()
        .height(min_height)
}

/// 平台等宽字体名；缺失时 windui 会静默回退系统默认字体。
pub fn mono_family() -> &'static str {
    if cfg!(target_os = "windows") {
        "Consolas"
    } else if cfg!(target_os = "macos") {
        "Menlo"
    } else {
        "DejaVu Sans Mono"
    }
}

/// 过长的 toast 文案截断。
pub fn shorten(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars).collect();
        out.push('…');
        out
    }
}

/// 把多行列表拼成一段文本（权限 / 分包等「复制全部」用）。
pub fn join_lines<I: IntoIterator<Item = S>, S: AsRef<str>>(items: I) -> String {
    let mut out = String::new();
    for it in items {
        out.push_str(it.as_ref());
        out.push('\n');
    }
    out
}
