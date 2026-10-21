//! icon 模块的端到端测试：文本 XML → SVG 片段断言、resvg 回读、内存 APK 渲染。

use std::collections::BTreeMap;

use super::resolve::tests::{res, MemSource};
use super::svg::{self, LiteralResolver};
use super::xml::parse_root;
use super::*;

fn vector_svg(src: &str) -> String {
    let root = parse_root(src.as_bytes()).expect("parse vector");
    svg::vector_document(&mut LiteralResolver, &root).0
}

/// resvg 能解析并渲染（不报错、尺寸正确）。
fn assert_svg_renders(svg_text: &str) -> tiny_skia::Pixmap {
    let tree = resvg::usvg::Tree::from_str(svg_text, &resvg::usvg::Options::default())
        .unwrap_or_else(|e| panic!("usvg failed: {e}\n{svg_text}"));
    let size = tree.size();
    let mut pm = tiny_skia::Pixmap::new(size.width().ceil() as u32, size.height().ceil() as u32)
        .expect("pixmap");
    resvg::render(&tree, tiny_skia::Transform::identity(), &mut pm.as_mut());
    pm
}

fn px(pm: &tiny_skia::Pixmap, x: u32, y: u32) -> [u8; 4] {
    let c = pm.pixel(x, y).expect("pixel").demultiply();
    [c.red(), c.green(), c.blue(), c.alpha()]
}

fn img_px(img: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * img.width + x) * 4) as usize;
    [
        img.data[i],
        img.data[i + 1],
        img.data[i + 2],
        img.data[i + 3],
    ]
}

const NS: &str = r#"xmlns:android="http://schemas.android.com/apk/res/android" xmlns:aapt="http://schemas.android.com/aapt""#;

#[test]
fn vector_path_and_group_transform() {
    let svg_text = vector_svg(&format!(
        r##"<vector {NS} android:width="24dp" android:height="24dp" android:viewportWidth="24" android:viewportHeight="24" android:alpha="0.5">
  <group android:translateX="1" android:pivotX="12" android:pivotY="12" android:rotation="90" android:scaleX="2">
    <path android:pathData="M0,0h4v4h-4z" android:fillColor="#80ff0000" android:fillAlpha="0.5" android:fillType="evenOdd"
          android:strokeColor="#00ff00" android:strokeWidth="2" android:strokeLineCap="1" android:strokeLineJoin="bevel"/>
  </group>
  <path android:pathData="M1,1h1"/>
</vector>"##
    ));
    assert!(svg_text.contains(r##"viewBox="0 0 24 24""##), "{svg_text}");
    assert!(svg_text.contains(r##"<g opacity="0.5">"##));
    assert!(svg_text.contains(r##"transform="translate(1 0) translate(12 12) rotate(90) scale(2 1) translate(-12 -12)""##), "{svg_text}");
    // 颜色 alpha 0x80 × fillAlpha 0.5
    assert!(
        svg_text.contains(r##"fill="#ff0000" fill-opacity="0.251""##),
        "{svg_text}"
    );
    assert!(svg_text.contains(r##"fill-rule="evenodd""##));
    assert!(
        svg_text.contains(
            r##"stroke="#00ff00" stroke-width="2" stroke-linecap="round" stroke-linejoin="bevel""##
        ),
        "{svg_text}"
    );
    // 无 fillColor 的路径不产生任何图元（Android 默认透明）
    assert!(!svg_text.contains(r##"d="M1,1h1""##));
    assert_svg_renders(&svg_text);
}

#[test]
fn clip_path_applies_to_following_siblings_only() {
    let svg_text = vector_svg(&format!(
        r##"<vector {NS} android:viewportWidth="20" android:viewportHeight="20" android:width="20dp" android:height="20dp">
  <path android:pathData="M0,0h20v2h-20z" android:fillColor="#0000ff"/>
  <group>
    <clip-path android:pathData="M0,0h10v20h-10z"/>
    <path android:pathData="M0,0h20v20h-20z" android:fillColor="#ff0000"/>
  </group>
  <path android:pathData="M0,18h20v2h-20z" android:fillColor="#00ff00"/>
</vector>"##
    ));
    // clip 前的兄弟不在裁剪组内；clip 后的兄弟在
    let clip_g = svg_text
        .find(r#"<g clip-path="url(#clip_0)">"#)
        .expect("clip group");
    let blue = svg_text.find("#0000ff").expect("blue");
    let red = svg_text.find("#ff0000").expect("red");
    let green = svg_text.find("#00ff00").expect("green");
    assert!(blue < clip_g && clip_g < red);
    let pm = assert_svg_renders(&svg_text);
    // 红色只在左半边
    assert_eq!(px(&pm, 5, 10), [255, 0, 0, 255]);
    assert_eq!(px(&pm, 15, 10)[3], 0, "right half must be clipped");
    // group 结束后 clip 失效：底部绿色横贯
    assert_eq!(px(&pm, 15, 19), [0, 255, 0, 255]);
    assert!(green > red);
}

#[test]
fn nested_clip_paths_intersect() {
    let svg_text = vector_svg(&format!(
        r##"<vector {NS} android:viewportWidth="20" android:viewportHeight="20" android:width="20dp" android:height="20dp">
  <clip-path android:pathData="M0,0h10v20h-10z"/>
  <clip-path android:pathData="M0,0h20v10h-20z"/>
  <path android:pathData="M0,0h20v20h-20z" android:fillColor="#ff0000"/>
</vector>"##
    ));
    let pm = assert_svg_renders(&svg_text);
    assert_eq!(px(&pm, 5, 5)[3], 255);
    assert_eq!(px(&pm, 15, 5)[3], 0);
    assert_eq!(px(&pm, 5, 15)[3], 0);
}

#[test]
fn inline_linear_and_radial_gradients() {
    let svg_text = vector_svg(&format!(
        r##"<vector {NS} android:viewportWidth="10" android:viewportHeight="10" android:width="10dp" android:height="10dp">
  <path android:pathData="M0,0h10v10h-10z">
    <aapt:attr name="android:fillColor">
      <gradient android:type="linear" android:startX="0" android:startY="0" android:endX="10" android:endY="0" android:tileMode="mirror">
        <item android:color="#ffff0000" android:offset="0"/>
        <item android:color="#800000ff" android:offset="1"/>
      </gradient>
    </aapt:attr>
  </path>
  <path android:pathData="M0,0h10v10h-10z">
    <aapt:attr name="android:fillColor">
      <gradient android:type="radial" android:startColor="#ffffff" android:endColor="#000000"/>
    </aapt:attr>
  </path>
</vector>"##
    ));
    assert!(svg_text.contains(r##"<linearGradient id="gradient_0" x1="0" y1="0" x2="10" y2="0" gradientUnits="userSpaceOnUse" spreadMethod="reflect">"##), "{svg_text}");
    assert!(
        svg_text.contains(r##"<stop offset="1" stop-color="#0000ff" stop-opacity="0.502"/>"##),
        "{svg_text}"
    );
    // radial 缺省中心 / 半径取路径包围盒
    assert!(
        svg_text.contains(r##"<radialGradient id="gradient_1" cx="5" cy="5" r="5""##),
        "{svg_text}"
    );
    assert!(svg_text.contains(r##"fill="url(#gradient_0)""##));
    assert_svg_renders(&svg_text);
}

#[test]
fn sweep_gradient_is_approximated() {
    let svg_text = vector_svg(&format!(
        r##"<vector {NS} android:viewportWidth="20" android:viewportHeight="20" android:width="20dp" android:height="20dp">
  <path android:pathData="M0,0h20v20h-20z">
    <aapt:attr name="android:fillColor">
      <gradient android:type="sweep" android:centerX="10" android:centerY="10">
        <item android:color="#ffff0000" android:offset="0"/>
        <item android:color="#ff0000ff" android:offset="1"/>
      </gradient>
    </aapt:attr>
  </path>
</vector>"##
    ));
    assert!(svg_text.contains(r##"<mask id="sweep_0">"##), "{svg_text}");
    let pm = assert_svg_renders(&svg_text);
    // 角度 0（右侧）偏红，接近 360°（右上方）偏蓝
    let right = px(&pm, 19, 10);
    let above_right = px(&pm, 19, 9);
    assert!(right[0] > 200 && right[2] < 60, "{right:?}");
    assert!(above_right[2] > 200, "{above_right:?}");
    // 角度 180（左侧）中间色
    let left = px(&pm, 0, 10);
    assert!(left[0] > 90 && left[2] > 90, "{left:?}");
}

#[test]
fn trim_path_emitted_as_geometry() {
    let svg_text = vector_svg(&format!(
        r##"<vector {NS} android:viewportWidth="10" android:viewportHeight="10" android:width="10dp" android:height="10dp">
  <path android:pathData="M0,5 L10,5" android:strokeColor="#000000" android:strokeWidth="1" android:trimPathEnd="0.5"/>
</vector>"##
    ));
    assert!(svg_text.contains(r##"d="M0 5L5 5""##), "{svg_text}");
    let pm = assert_svg_renders(&svg_text);
    assert!(px(&pm, 2, 5)[3] > 0);
    assert_eq!(px(&pm, 8, 5)[3], 0);
}

fn memory_apk() -> (MemSource, BTreeMap<u32, ResourceInfo>) {
    let adaptive = format!(
        r##"<adaptive-icon {NS}><background android:drawable="@res/0x7f050000"/><foreground android:drawable="@res/0x7f080001"/></adaptive-icon>"##
    );
    // 前景：视口中心 4×4 黑块；放大 1.28 后仍在中央
    let fg = format!(
        r##"<vector {NS} android:width="108dp" android:height="108dp" android:viewportWidth="108" android:viewportHeight="108"><path android:pathData="M44,44h20v20h-20z" android:fillColor="@res/0x7f050001"/></vector>"##
    );
    let layer = format!(
        r##"<layer-list {NS}><item><shape android:shape="oval"><solid android:color="#ff00ff00"/></shape></item><item android:left="100dp" android:top="100dp" android:right="100dp" android:bottom="100dp"><shape><solid android:color="#ffff0000"/><corners android:radius="8dp"/></shape></item></layer-list>"##
    );
    let mut png = Vec::new();
    {
        let img = image::RgbaImage::from_pixel(8, 4, image::Rgba([0, 0, 255, 255]));
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("png");
    }
    let src = MemSource::with(&[
        ("res/mipmap-anydpi-v26/ic_launcher.xml", adaptive.as_bytes()),
        ("res/drawable/ic_fg.xml", fg.as_bytes()),
        ("res/drawable/layers.xml", layer.as_bytes()),
        ("res/mipmap-xxhdpi/ic_launcher.png", &png),
    ]);
    let mut map = BTreeMap::new();
    map.insert(
        0x7f050000,
        res(0x7f050000, "color", "bg", &[], &["#ff336699"], &[]),
    );
    map.insert(
        0x7f050001,
        res(0x7f050001, "color", "fg", &[], &["#ff000000"], &[]),
    );
    map.insert(
        0x7f080001,
        res(
            0x7f080001,
            "drawable",
            "ic_fg",
            &["res/drawable/ic_fg.xml"],
            &[],
            &[],
        ),
    );
    map.insert(
        0x7f0d0000,
        res(
            0x7f0d0000,
            "mipmap",
            "ic_launcher",
            &[
                "res/mipmap-anydpi-v26/ic_launcher.xml",
                "res/mipmap-xxhdpi/ic_launcher.png",
            ],
            &[],
            &[],
        ),
    );
    (src, map)
}

#[test]
fn renders_adaptive_icon_with_mask() {
    let (mut src, map) = memory_apk();
    let mut r = IconRenderer::new(&mut src, &map);
    let img = r
        .render_path("@mipmap/ic_launcher", 432, true)
        .expect("render");
    assert_eq!((img.width, img.height), (432, 432));
    // 背景色
    assert_eq!(img_px(&img, 60, 216), [0x33, 0x66, 0x99, 255]);
    // 前景黑块在中心
    assert_eq!(img_px(&img, 216, 216), [0, 0, 0, 255]);
    // 圆角外透明
    assert_eq!(img_px(&img, 1, 1)[3], 0);
    // 非透明模式下四角是白底
    let img = r
        .render_path("res/mipmap-anydpi-v26/ic_launcher.xml", 108, false)
        .expect("render");
    assert_eq!(img_px(&img, 0, 0), [255, 255, 255, 255]);
}

#[test]
fn renders_layer_list_and_shapes() {
    let (mut src, map) = memory_apk();
    let mut r = IconRenderer::new(&mut src, &map);
    let img = r
        .render_path("res/drawable/layers.xml", 432, true)
        .expect("render");
    assert_eq!(img_px(&img, 216, 50)[1], 255, "oval");
    assert_eq!(img_px(&img, 2, 2)[3], 0, "outside oval");
    assert_eq!(img_px(&img, 216, 216), [255, 0, 0, 255], "inset rect");
    assert_eq!(img_px(&img, 100, 100)[0], 0, "rounded corner of inset rect");
}

#[test]
fn bitmap_candidate_native_resolution_and_contain() {
    let (mut src, map) = memory_apk();
    let mut r = IconRenderer::new(&mut src, &map);
    let c = IconCandidate::from_path("res/mipmap-xxhdpi/ic_launcher.png", None);
    let img = r.render_candidate(&c, 432, false).expect("bitmap");
    assert_eq!((img.width, img.height), (8, 4));
    // 作为 drawable 渲染时等比居中
    let img = r
        .render_path("res/mipmap-xxhdpi/ic_launcher.png", 64, true)
        .expect("render");
    assert_eq!(img_px(&img, 32, 32), [0, 0, 255, 255]);
    assert_eq!(img_px(&img, 32, 4)[3], 0);
    assert_eq!(r.raw_bytes(&c).map(|b| b.len() > 8), Some(true));
}

#[test]
fn export_adaptive_svg() {
    let (mut src, map) = memory_apk();
    let mut r = IconRenderer::new(&mut src, &map);
    let c = IconCandidate::from_path("res/mipmap-anydpi-v26/ic_launcher.xml", None);
    let svg_text = r.export_svg(&c).expect("svg");
    assert!(svg_text.contains(r##"<clipPath id="adaptive_clip"><rect width="108" height="108" rx="23.76" ry="23.76"/></clipPath>"##), "{svg_text}");
    assert!(
        svg_text.contains(r##"<rect width="108" height="108" fill="#336699"/>"##),
        "{svg_text}"
    );
    assert!(
        svg_text.contains(r##"translate(54 54) scale(1.28) translate(-54 -54)"##),
        "{svg_text}"
    );
    assert!(svg_text.contains(r##"fill="#000000""##));
    assert_svg_renders(&svg_text);
    // 位图候选不支持 SVG
    let b = IconCandidate::from_path("res/mipmap-xxhdpi/ic_launcher.png", None);
    assert!(matches!(r.export_svg(&b), Err(IconError::Unsupported(_))));
}

#[test]
fn nothing_drawn_is_error() {
    let (mut src, map) = memory_apk();
    let mut r = IconRenderer::new(&mut src, &map);
    assert!(matches!(
        r.render_path("res/missing.xml", 64, false),
        Err(IconError::NothingDrawn(_))
    ));
    assert!(r.render_path("@res/0x7f777777", 64, false).is_err());
}

#[test]
fn png_roundtrip() {
    let img = RgbaImage {
        width: 2,
        height: 1,
        data: vec![255, 0, 0, 255, 0, 0, 0, 0],
    };
    let png = encode_png(&img);
    assert_eq!(decode_bitmap(&png), Some(img));
    assert!(encode_png(&RgbaImage {
        width: 3,
        height: 3,
        data: vec![]
    })
    .is_empty());
    assert!(decode_bitmap(b"nope").is_none());
}

/// 真实 APK 冒烟测试：`APKINFO_ICON_APKS` 为以 `;` 分隔的 APK 路径，
/// `APKINFO_ICON_OUT` 为输出目录。每个候选渲染成 PNG，XML 候选另导出 SVG 并回读校验。
/// 运行：`cargo test -p apkinfo-core real_apk_icons -- --ignored --nocapture`
#[test]
#[ignore]
fn real_apk_icons() {
    struct ReaderSource<'r>(&'r crate::apk::ApkReader);
    impl IconSource for ReaderSource<'_> {
        fn read(&mut self, path: &str) -> Option<Vec<u8>> {
            self.0.read_file(path)
        }
        fn file_names(&mut self) -> Vec<String> {
            self.0.list_files()
        }
    }

    let (Ok(apks), Ok(out)) = (
        std::env::var("APKINFO_ICON_APKS"),
        std::env::var("APKINFO_ICON_OUT"),
    ) else {
        eprintln!("APKINFO_ICON_APKS / APKINFO_ICON_OUT not set, skipped");
        return;
    };
    let out = std::path::PathBuf::from(out);
    std::fs::create_dir_all(&out).expect("create out dir");
    for apk in apks.split(';').filter(|s| !s.is_empty()) {
        let stem = std::path::Path::new(apk)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("apk")
            .to_string();
        let mut reader = crate::apk::ApkReader::open(apk).expect("open apk");
        let meta = reader.parse().expect("parse manifest");
        let resources = reader.resources();
        println!(
            "== {stem}: icon={:?} icon_paths={:?} resources={}",
            meta.application_icon,
            meta.icon_paths,
            resources.len()
        );
        let mut src = ReaderSource(&reader);
        let candidates = collect_candidates(&mut src, &resources, None, &meta.icon_paths);
        let mut r = IconRenderer::new(&mut src, &resources);
        for (i, c) in candidates.iter().enumerate() {
            match r.render_candidate(c, DEFAULT_ICON_SIZE, false) {
                Ok(img) => {
                    let file = out.join(format!("{stem}_{i}.png"));
                    std::fs::write(&file, encode_png(&img)).expect("write png");
                    println!(
                        "  [{i}] {} -> {}x{} {}",
                        c.display_label,
                        img.width,
                        img.height,
                        file.display()
                    );
                }
                Err(e) => println!("  [{i}] {} -> render error: {e}", c.display_label),
            }
            if c.kind == IconKind::XmlVector {
                match r.export_svg(c) {
                    Ok(svg_text) => {
                        let file = out.join(format!("{stem}_{i}.svg"));
                        std::fs::write(&file, &svg_text).expect("write svg");
                        let pm = assert_svg_renders(&svg_text);
                        let img = RgbaImage {
                            width: pm.width(),
                            height: pm.height(),
                            data: pm
                                .pixels()
                                .iter()
                                .flat_map(|p| {
                                    let c = p.demultiply();
                                    [c.red(), c.green(), c.blue(), c.alpha()]
                                })
                                .collect(),
                        };
                        std::fs::write(out.join(format!("{stem}_{i}_svg.png")), encode_png(&img))
                            .expect("write svg png");
                        println!("      svg ok ({} bytes)", svg_text.len());
                    }
                    Err(e) => println!("      svg: {e}"),
                }
                if let Ok(img) = r.render_candidate(c, 1024, true) {
                    std::fs::write(out.join(format!("{stem}_{i}_1024t.png")), encode_png(&img))
                        .expect("write png");
                }
            }
        }
        if let Some(icon) = &meta.application_icon {
            match r.render_path(icon, DEFAULT_ICON_SIZE, false) {
                Ok(img) => {
                    std::fs::write(out.join(format!("{stem}_ref.png")), encode_png(&img))
                        .expect("write png");
                    println!("  ref {icon} ok");
                }
                Err(e) => println!("  ref {icon}: {e}"),
            }
        }
    }
}
