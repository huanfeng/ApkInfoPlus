//! 把 PNG 应用图标打成多尺寸 .ico（Windows 资源嵌入用）。
//!
//! 用法：`cargo run -p apkinfo-core --example make_ico -- icon.png app.ico`

use image::codecs::ico::{IcoEncoder, IcoFrame};
use image::imageops::FilterType;
use image::ExtendedColorType;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(src), Some(dst)) = (args.next(), args.next()) else {
        eprintln!("usage: make_ico <in.png> <out.ico>");
        std::process::exit(2);
    };
    let img = image::open(&src).expect("open png").to_rgba8();
    let mut frames = Vec::new();
    for size in [256u32, 128, 64, 48, 32, 24, 16] {
        let scaled = image::imageops::resize(&img, size, size, FilterType::Lanczos3);
        frames.push(
            IcoFrame::as_png(scaled.as_raw(), size, size, ExtendedColorType::Rgba8)
                .expect("encode frame"),
        );
    }
    let out = std::fs::File::create(&dst).expect("create ico");
    IcoEncoder::new(out)
        .encode_images(&frames)
        .expect("write ico");
    println!("wrote {} ({} frames)", dst, frames.len());
}
