//! 语言列表对话框：概览「语言列表」点开，逐条列出代码、当前界面语言下的名称与自称。

use windui::prelude::*;

use apkinfo_core::android::DEFAULT_LOCALE;

use crate::android_text::{locale_autonym, locale_name};
use crate::state::AppState;

/// 对话框状态（应用级，主窗口根部挂一份）。
#[derive(Clone)]
pub struct LocalesState {
    pub show: Signal<bool>,
    pub codes: Signal<Vec<String>>,
    pub filter: Signal<String>,
}

impl LocalesState {
    pub fn new() -> Self {
        Self {
            show: signal(false),
            codes: signal(Vec::new()),
            filter: signal(String::new()),
        }
    }

    pub fn open(&self, mut codes: Vec<String>) {
        // 资源表里是出现顺序，按代码排好才找得到；「默认」占位排最前
        codes.sort_by_key(|c| (c != DEFAULT_LOCALE, c.to_ascii_lowercase()));
        self.codes.set(codes);
        // 过滤结果是 filter 的派生：重置它顺带让列表按新数据重建（`set` 不比较新旧值、
        // 版本号必增，同为空串也会触发）
        self.filter.set(String::new());
        self.show.set(true);
    }
}

/// 过滤：代码、当前界面语言下的名称、自称，任一包含关键字即命中。
fn matches(code: &str, needle: &str) -> bool {
    needle.is_empty()
        || code.to_lowercase().contains(needle)
        || locale_name(code).to_lowercase().contains(needle)
        || locale_autonym(code).is_some_and(|a| a.to_lowercase().contains(needle))
}

const CODE_W: i32 = 120;
const AUTONYM_W: i32 = 200;
const LIST_H: i32 = 420;

pub fn build_dialog(state: &AppState) -> Element {
    let ls = state.locales_dialog.clone();
    let show = ls.show;
    let codes = ls.codes;
    let filtered: Signal<Vec<String>> = Signal::map(ls.filter, move |f| {
        let needle = f.trim().to_lowercase();
        codes.with(|all| {
            all.iter()
                .filter(|c| matches(c, &needle))
                .cloned()
                .collect()
        })
    });

    let head_label = |text: windui::i18n::Message| {
        Element::label(text)
            .font_size(12.5)
            .font_weight(600)
            .fg_role(Role::TextMuted)
    };
    let header = Element::row()
        .width_match()
        .spacing(12)
        .padding_xy(10, 6)
        .bg_role(Role::SurfaceAlt)
        .corner(6.0)
        // 代码列的值是无框输入框，文字有内边距，表头跟着缩进
        .child(super::plain_value(head_label(t!("locale.col_code"))).width(CODE_W))
        .child(head_label(t!("locale.col_name")).weight(1.0))
        .child(head_label(t!("locale.col_autonym")).width(AUTONYM_W));

    let ui_lang = state.ui_lang;
    let list = Element::list_signal(
        filtered,
        |c: &String| c.clone(),
        move |code: String| {
            let name = {
                let code = code.clone();
                Signal::map(ui_lang, move |_| locale_name(&code))
            };
            Element::row()
                .width_match()
                .cross(Align::Center)
                .spacing(12)
                .padding_xy(10, 0)
                .child(
                    super::selectable(code.clone())
                        .font_family(super::mono_family())
                        .width(CODE_W),
                )
                .child(Element::label(name).font_size(13.0).weight(1.0))
                .child(
                    Element::label(locale_autonym(&code).unwrap_or_default())
                        .font_size(13.0)
                        .fg_role(Role::TextMuted)
                        .width(AUTONYM_W),
                )
        },
    )
    .width_match()
    .height(LIST_H);

    let top = Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(10)
        .child(Element::text_input(ls.filter, t!("action.filter")).width(240))
        .child(
            Element::label(t!(
                "field.count",
                count = Signal::map(filtered, |v| v.len() as i64)
            ))
            .font_size(12.5)
            .fg_role(Role::TextMuted)
            .weight(1.0),
        )
        .child({
            Element::button(t!("action.copy_all"))
                .small()
                .outline_soft()
                .neutral()
                .on_click(move |ctx| {
                    ctx.clipboard_set(&codes.with(|v| super::join_lines(v)));
                    ctx.toast_ok(tr!("action.copied", content = tr!("field.languages")));
                })
        });

    let body = Element::col()
        .width_match()
        .spacing(8)
        .child(top)
        .child(header)
        .child(list);
    let footer = Element::row()
        .width_match()
        .child(Element::flex_spacer())
        .child(
            Element::button(t!("action.close"))
                .small()
                .neutral()
                .outline_soft()
                .on_click(move |_| show.set(false)),
        );
    Element::dialog_panel(
        show,
        t!("locale.title"),
        640,
        move |_| show.set(false),
        body,
        footer,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_matches_code_and_autonym() {
        assert!(matches("ja", ""));
        assert!(matches("zh-HK", "hk"));
        assert!(matches("ja", "日本"));
        assert!(!matches("ja", "español"));
    }
}
