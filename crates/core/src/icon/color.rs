//! 颜色与尺寸字面量解析。移植自 `adaptive_icon_renderer.dart` 的 `_parseColor` /
//! `_parseAndroidColorInt` / `_parseDimension`。
//!
//! 颜色统一用 `u32` 表示 ARGB（0xAARRGGBB），与 Android 资源表的存储方式一致。

/// 是否为 `#RGB` / `#ARGB` / `#RRGGBB` / `#AARRGGBB` 形式的颜色字面量。
pub(crate) fn is_color_literal(value: &str) -> bool {
    let v = value.trim();
    let Some(hex) = v.strip_prefix('#') else {
        return false;
    };
    matches!(hex.len(), 3 | 4 | 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

/// 解析 `#...` 颜色字面量为 ARGB。
pub(crate) fn parse_color(value: &str) -> Option<u32> {
    let v = value.trim();
    if !is_color_literal(v) {
        return None;
    }
    let hex = &v[1..];
    let nib = |i: usize| -> u32 {
        let d = u32::from_str_radix(&hex[i..=i], 16).unwrap_or(0);
        d * 17
    };
    let byte = |i: usize| -> u32 { u32::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0) };
    let (a, r, g, b) = match hex.len() {
        3 => (255, nib(0), nib(1), nib(2)),
        4 => (nib(0), nib(1), nib(2), nib(3)),
        6 => (255, byte(0), byte(2), byte(4)),
        _ => (byte(0), byte(2), byte(4), byte(6)),
    };
    Some((a << 24) | (r << 16) | (g << 8) | b)
}

/// 是否为 Android 整型字面量（`0x...`、`0x-...`、`-0x...`、十进制可带负号）。
/// 二进制 XML 中 `TYPE_INT_COLOR` 以外的整型颜色（比如混淆后被写成 int）会以这种形式出现。
pub(crate) fn is_android_int_literal(value: &str) -> bool {
    parse_android_int_literal(value).is_some()
}

fn parse_android_int_literal(raw: &str) -> Option<i64> {
    let v = raw.trim().to_ascii_lowercase();
    let hex = |body: &str| -> Option<i64> {
        if body.is_empty() || !body.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        i64::from_str_radix(body, 16).ok()
    };
    if let Some(body) = v.strip_prefix("0x-").or_else(|| v.strip_prefix("-0x")) {
        return hex(body).map(|n| -n);
    }
    if let Some(body) = v.strip_prefix("0x") {
        return hex(body);
    }
    let digits = v.strip_prefix('-').unwrap_or(&v);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    v.parse::<i64>().ok()
}

/// 把 Android 整型颜色转成 ARGB。alpha 为 0 但 RGB 非零时补 `ff`：
/// 这类值通常是开发者写了 `0xRRGGBB` 省略 alpha；全 0 则保留透明。
pub(crate) fn parse_android_color_int(raw: &str) -> Option<u32> {
    let signed = parse_android_int_literal(raw)?;
    let mut color = (signed & 0xffff_ffff) as u32;
    if color & 0xff00_0000 == 0 && color > 0 && color <= 0x00ff_ffff {
        color |= 0xff00_0000;
    }
    Some(color)
}

/// 颜色字面量或 Android 整型颜色。
pub(crate) fn parse_any_color_literal(value: &str) -> Option<u32> {
    let v = value.trim();
    if is_color_literal(v) {
        return parse_color(v);
    }
    if is_android_int_literal(v) {
        return parse_android_color_int(v);
    }
    None
}

/// 格式化为 `#aarrggbb`（资源表与 Dart 版的统一格式）。
pub(crate) fn format_argb(color: u32) -> String {
    format!("#{color:08x}")
}

/// SVG 颜色：`#rrggbb` + 不透明度（0..1）。
pub(crate) fn svg_color(color: u32) -> (String, f32) {
    let a = (color >> 24) & 0xff;
    (format!("#{:06x}", color & 0x00ff_ffff), a as f32 / 255.0)
}

/// 解析尺寸 / 浮点：先整体按数字解析，失败则取前导数字（兼容 `108.0dp`、`12px`、`50%`）。
pub(crate) fn parse_dimension(raw: Option<&str>, fallback: f32) -> f32 {
    let Some(raw) = raw else {
        return fallback;
    };
    let t = raw.trim();
    if t.is_empty() {
        return fallback;
    }
    if let Ok(v) = t.parse::<f32>() {
        if v.is_finite() {
            return v;
        }
    }
    match leading_number(t).and_then(|s| s.parse::<f32>().ok()) {
        Some(v) if v.is_finite() => v,
        _ => fallback,
    }
}

/// 对应 Dart 的 `_kLeadingNumberPattern`：`^[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?`
fn leading_number(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let has_int = i > int_start;
    if i < b.len() && b[i] == b'.' {
        let dot = i;
        i += 1;
        let frac_start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if !has_int && i == frac_start {
            let _ = dot;
            return None;
        }
    } else if !has_int {
        return None;
    }
    // 指数部分必须完整才吞掉
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let exp_start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > exp_start {
            i = j;
        }
    }
    Some(&s[..i])
}

/// 尺寸是否为百分比（Android inset 自 API 28 起支持 fraction）。
pub(crate) fn is_fraction(raw: Option<&str>) -> bool {
    raw.is_some_and(|s| s.trim().ends_with('%'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_colors() {
        assert_eq!(parse_color("#f00"), Some(0xffff0000));
        assert_eq!(parse_color("#8f00"), Some(0x88ff0000));
        assert_eq!(parse_color("#00ff00"), Some(0xff00ff00));
        assert_eq!(parse_color("#80112233"), Some(0x80112233));
        assert_eq!(parse_color(" #FFAABBCC "), Some(0xffaabbcc));
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(parse_color("#ggg"), None);
        assert_eq!(parse_color("red"), None);
    }

    #[test]
    fn android_int_colors() {
        // alpha 为 0 且 RGB 非零 → 补 ff
        assert_eq!(parse_android_color_int("0x00ff00"), Some(0xff00ff00));
        assert_eq!(parse_android_color_int("0"), Some(0));
        assert_eq!(parse_android_color_int("-1"), Some(0xffffffff));
        assert_eq!(parse_android_color_int("-16777216"), Some(0xff000000));
        assert_eq!(parse_android_color_int("0x-1"), Some(0xffffffff));
        assert_eq!(parse_android_color_int("-0x1000000"), Some(0xff000000));
        assert_eq!(parse_android_color_int("0x80ff0000"), Some(0x80ff0000));
        assert_eq!(parse_android_color_int("12ab"), None);
        assert!(is_android_int_literal("0x7f010000"));
        assert!(!is_android_int_literal("@res/0x7f010000"));
    }

    #[test]
    fn dimensions() {
        assert_eq!(parse_dimension(Some("108.0dp"), 0.0), 108.0);
        assert_eq!(parse_dimension(Some("1.5"), 0.0), 1.5);
        assert_eq!(parse_dimension(Some("-2e1px"), 0.0), -20.0);
        assert_eq!(parse_dimension(Some(".5"), 0.0), 0.5);
        assert_eq!(parse_dimension(Some("abc"), 7.0), 7.0);
        assert_eq!(parse_dimension(None, 3.0), 3.0);
        assert_eq!(parse_dimension(Some("12e"), 0.0), 12.0);
    }

    #[test]
    fn alpha_and_format() {
        assert_eq!(format_argb(0x01020304), "#01020304");
        assert_eq!(
            svg_color(0x80ff0000),
            ("#ff0000".to_string(), 128.0 / 255.0)
        );
    }
}
