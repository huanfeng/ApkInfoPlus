//! 界面组装。每个子模块导出一个 `build_*` 函数，返回 `Element`。
//!
//! 共用的小部件（字段行、分区卡片、复制按钮）放在这里，保证各页视觉一致。

pub mod header;
pub mod install;
pub mod main_window;
pub mod pages;
pub mod rename;
pub mod settings;

use windui::prelude::*;

use crate::state::AppState;

/// 内容区左右留白。
pub const PAGE_PAD: i32 = 20;
/// 滚动容器右侧额外留白：windui 的滚动条压在内容之上（约 10px），不让位就会盖住行尾。
pub const SCROLL_GUTTER: i32 = 10;

/// 「标题 | 值 | 复制」一行。值为空时显示占位。
pub fn field_row(
    state: &AppState,
    title: impl Into<TextContent>,
    value: impl Into<String>,
) -> Element {
    let value: String = value.into();
    field_row_with(state, title, value, true)
}

pub fn field_row_with(
    state: &AppState,
    title: impl Into<TextContent>,
    value: String,
    copyable: bool,
) -> Element {
    let width = state.title_width.get();
    let display = if value.is_empty() {
        tr!("field.none")
    } else {
        value.clone()
    };
    let mut row =
        Element::row()
            .width_match()
            .cross(Align::Start)
            .spacing(12)
            .padding_xy(0, 5)
            .child(
                Element::label(title)
                    .width(width)
                    .font_size(13.0)
                    .fg_role(Role::TextMuted),
            )
            .child(Element::label(display).weight(1.0).font_size(13.0).fg_role(
                if value.is_empty() {
                    Role::TextSubtle
                } else {
                    Role::Text
                },
            ));
    if copyable && !value.is_empty() {
        row = row.child(copy_button(value));
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

/// 分区卡片：标题 + 分隔线 + 内容。
pub fn section(title: impl Into<String>, body: Element) -> Element {
    Element::card(title, body).width_match()
}

/// 分区卡片，标题旁可放一个操作（如「复制全部」）。
pub fn section_with_action(
    title: impl Into<TextContent>,
    action: Element,
    body: Element,
) -> Element {
    Element::col()
        .width_match()
        .bg_role(Role::Surface)
        .corner(10.0)
        .border_role(Role::Border, 1)
        .padding(16)
        .spacing(10)
        .child(
            Element::row()
                .width_match()
                .cross(Align::Center)
                .child(
                    Element::label(title)
                        .font_size(14.0)
                        .font_weight(600)
                        .fg_role(Role::Text)
                        .weight(1.0),
                )
                .child(action),
        )
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
                14,
                PAGE_PAD + SCROLL_GUTTER,
                PAGE_PAD,
            ))
            .spacing(14)
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
