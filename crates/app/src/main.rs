//! APK Info Plus 桌面应用入口。
use windui::prelude::*;

fn main() {
    let ui = Element::col()
        .fill()
        .padding(24)
        .spacing(12)
        .bg_role(Role::Bg)
        .child(Element::label("APK Info Plus").font_size(22.0).fg_role(Role::Text))
        .child(Element::label("脚手架：窗口可运行。").fg_role(Role::TextMuted));

    App::new("APK Info Plus", 960, 640)
        .min_size(720, 480)
        .centered()
        .screenshot_from_args()
        .content(ui)
        .run();
}
