//! VectorDrawable pathData 几何工具：包围盒（渐变缺省坐标需要）与 trimPath。
//!
//! SVG 没有 trimPath 的等价物（`stroke-dasharray` 只能裁描边、不能裁填充），
//! 所以这里把路径展平成折线后按长度截取，生成新的 pathData 交给 SVG。

use svgtypes::{SimplePathSegment, SimplifyingPathParser};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Bounds {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Bounds {
    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }
    pub fn cx(&self) -> f32 {
        (self.x0 + self.x1) / 2.0
    }
    pub fn cy(&self) -> f32 {
        (self.y0 + self.y1) / 2.0
    }
    pub fn is_empty(&self) -> bool {
        self.width() <= 0.0 || self.height() <= 0.0
    }
}

/// 一条子路径展平后的折线。
struct Contour {
    pts: Vec<(f64, f64)>,
}

/// 解析并展平。遇到语法错误时保留已解析的部分（与 Android 的宽松行为一致）。
fn flatten(d: &str) -> Vec<Contour> {
    let mut contours: Vec<Contour> = Vec::new();
    let mut cur: Vec<(f64, f64)> = Vec::new();
    let mut last = (0.0, 0.0);
    let mut start = (0.0, 0.0);
    for seg in SimplifyingPathParser::from(d) {
        let Ok(seg) = seg else { break };
        match seg {
            SimplePathSegment::MoveTo { x, y } => {
                if cur.len() > 1 {
                    contours.push(Contour {
                        pts: std::mem::take(&mut cur),
                    });
                }
                cur.clear();
                cur.push((x, y));
                last = (x, y);
                start = (x, y);
            }
            SimplePathSegment::LineTo { x, y } => {
                if cur.is_empty() {
                    cur.push(last);
                }
                cur.push((x, y));
                last = (x, y);
            }
            SimplePathSegment::CurveTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                if cur.is_empty() {
                    cur.push(last);
                }
                let n = 24;
                for i in 1..=n {
                    let t = i as f64 / n as f64;
                    let mt = 1.0 - t;
                    let px = mt * mt * mt * last.0
                        + 3.0 * mt * mt * t * x1
                        + 3.0 * mt * t * t * x2
                        + t * t * t * x;
                    let py = mt * mt * mt * last.1
                        + 3.0 * mt * mt * t * y1
                        + 3.0 * mt * t * t * y2
                        + t * t * t * y;
                    cur.push((px, py));
                }
                last = (x, y);
            }
            SimplePathSegment::Quadratic { x1, y1, x, y } => {
                if cur.is_empty() {
                    cur.push(last);
                }
                let n = 16;
                for i in 1..=n {
                    let t = i as f64 / n as f64;
                    let mt = 1.0 - t;
                    let px = mt * mt * last.0 + 2.0 * mt * t * x1 + t * t * x;
                    let py = mt * mt * last.1 + 2.0 * mt * t * y1 + t * t * y;
                    cur.push((px, py));
                }
                last = (x, y);
            }
            SimplePathSegment::ClosePath => {
                if !cur.is_empty() {
                    cur.push(start);
                    contours.push(Contour {
                        pts: std::mem::take(&mut cur),
                    });
                }
                last = start;
            }
        }
    }
    if cur.len() > 1 {
        contours.push(Contour { pts: cur });
    }
    contours
}

/// 路径包围盒；无可用几何时返回 `None`。
pub(crate) fn bounds(d: &str) -> Option<Bounds> {
    let mut b: Option<Bounds> = None;
    for c in flatten(d) {
        for &(x, y) in &c.pts {
            let (x, y) = (x as f32, y as f32);
            b = Some(match b {
                None => Bounds {
                    x0: x,
                    y0: y,
                    x1: x,
                    y1: y,
                },
                Some(o) => Bounds {
                    x0: o.x0.min(x),
                    y0: o.y0.min(y),
                    x1: o.x1.max(x),
                    y1: o.y1.max(y),
                },
            });
        }
    }
    b
}

/// 是否需要 trim（与 Dart 一致的阈值判断）。
pub(crate) fn needs_trim(start: f32, end: f32, offset: f32) -> bool {
    start.abs() > 1e-6 || (end - 1.0).abs() > 1e-6 || offset.abs() > 1e-6
}

/// 按 Dart 版 `_applyTrimPath` 的语义截取：start/end 加 offset 后取小数部分，
/// 每个子路径独立按比例截取；start > end 时取两段（环绕）。返回新的 pathData。
pub(crate) fn trim(d: &str, trim_start: f32, trim_end: f32, trim_offset: f32) -> String {
    // 跨度 ≥ 1 表示整条路径可见（例如只设 offset 的动画初始帧）。Dart 版在这种情况下
    // start/end 取小数后相等而整条路径消失，这里按 Android 语义保留原路径。
    let span = (trim_end - trim_start) as f64;
    if span >= 1.0 - 1e-6 {
        return d.to_string();
    }
    let mut out = String::new();
    if span <= 1e-6 {
        return out;
    }
    let frac = |v: f64| v - v.floor();
    let start = frac((trim_start + trim_offset) as f64);
    let end = frac((trim_end + trim_offset) as f64);
    for c in flatten(d) {
        if start < end {
            extract(&c, start, end, &mut out);
        } else {
            extract(&c, start, 1.0, &mut out);
            extract(&c, 0.0, end, &mut out);
        }
    }
    out
}

fn extract(c: &Contour, from: f64, to: f64, out: &mut String) {
    let seg_len: Vec<f64> = c
        .pts
        .windows(2)
        .map(|w| ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt())
        .collect();
    let total: f64 = seg_len.iter().sum();
    if total <= 0.0 {
        return;
    }
    let a = (from * total).clamp(0.0, total);
    let b = (to * total).clamp(0.0, total);
    if b <= a {
        return;
    }
    let mut acc = 0.0;
    let mut started = false;
    let lerp =
        |p: (f64, f64), q: (f64, f64), t: f64| (p.0 + (q.0 - p.0) * t, p.1 + (q.1 - p.1) * t);
    for (i, &len) in seg_len.iter().enumerate() {
        let (p, q) = (c.pts[i], c.pts[i + 1]);
        let s0 = acc;
        let s1 = acc + len;
        acc = s1;
        if s1 < a || len <= 0.0 {
            continue;
        }
        if s0 > b {
            break;
        }
        if !started {
            let t = ((a - s0) / len).clamp(0.0, 1.0);
            let pt = lerp(p, q, t);
            out.push_str(&format!("M{} {}", fmt(pt.0), fmt(pt.1)));
            started = true;
        }
        let t = ((b - s0) / len).clamp(0.0, 1.0);
        let pt = if s1 <= b { q } else { lerp(p, q, t) };
        out.push_str(&format!("L{} {}", fmt(pt.0), fmt(pt.1)));
        if s1 >= b {
            break;
        }
    }
}

/// 紧凑地格式化浮点（去掉多余的 0）。
pub(crate) fn fmt(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_of_rect() {
        let b = bounds("M2,3 h10 v5 h-10 z").expect("bounds");
        assert_eq!((b.x0, b.y0, b.x1, b.y1), (2.0, 3.0, 12.0, 8.0));
        assert!(bounds("").is_none());
    }

    #[test]
    fn trim_half_line() {
        let d = trim("M0,0 L10,0", 0.0, 0.5, 0.0);
        assert_eq!(d, "M0 0L5 0");
        // 环绕：offset 让区间跨过终点
        let d = trim("M0,0 L10,0", 0.0, 0.5, 0.75);
        assert_eq!(d, "M7.5 0L10 0M0 0L2.5 0");
        assert!(trim("M0,0 L10,0", 0.3, 0.3, 0.0).is_empty());
        // 只设 offset：整条路径保持可见
        assert_eq!(trim("M0,0 L10,0", 0.0, 1.0, 0.25), "M0,0 L10,0");
        // end=1 时取到结尾
        assert_eq!(trim("M0,0 L10,0", 0.2, 1.0, 0.0), "M2 0L10 0");
    }

    #[test]
    fn fmt_compact() {
        assert_eq!(fmt(1.0), "1");
        assert_eq!(fmt(1.25), "1.25");
        assert_eq!(fmt(-0.00001), "0");
    }
}
