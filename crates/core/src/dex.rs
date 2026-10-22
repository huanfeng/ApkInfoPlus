//! 最小 DEX 读取：只取 class_defs 里定义的类描述符与 string_ids 里的个别标记，
//! 供库识别（DEX 规则、技术栈标记、加固旁证）用。不解析代码与成员。
//!
//! 格式见 <https://source.android.com/docs/core/runtime/dex-format>。全部偏移来自用户文件，
//! 每次读取都做边界检查；越界或结构不对只是少收一部分，不 panic。

use std::collections::BTreeMap;

/// 一个 APK 内全部 `classes*.dex` 的汇总。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DexInfo {
    pub dex_count: usize,
    pub class_count: usize,
    /// 包名（点分）→ 该包直接定义的类数。默认包（无包名的类）不收。
    pub packages: BTreeMap<String, u32>,
    pub markers: DexMarkers,
}

/// 从类描述符 / 字符串表里顺带看出的标记。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DexMarkers {
    /// 字符串表里有 `Lkotlin/Metadata;`（Kotlin 编译器给每个类加的注解），或定义了 `kotlin.*` 类。
    pub kotlin: bool,
    /// 定义了 `androidx.compose.*` 类。
    pub compose: bool,
    /// 360 加固壳类（`com.qihoo.util` / `com.tianyu.util`）。
    pub qihoo: bool,
    /// 梆梆加固壳类（`com.secneo.apkwrapper`）。
    pub secneo: bool,
    /// `io.flutter.FlutterInjector`：Flutter 引擎被改名打包时的旁证。
    pub flutter_injector: bool,
}

/// `classes.dex`、`classes2.dex` …（根目录、数字后缀）。
pub fn is_dex_entry(name: &str) -> bool {
    name.strip_prefix("classes")
        .and_then(|r| r.strip_suffix(".dex"))
        .is_some_and(|d| d.bytes().all(|b| b.is_ascii_digit()))
}

impl DexInfo {
    /// 合并一个 dex 文件的内容；不是合法 dex 时返回 `false`（不计数）。
    pub fn add_dex(&mut self, data: &[u8]) -> bool {
        let Some(h) = Header::parse(data) else {
            return false;
        };
        self.dex_count += 1;
        let m = &mut self.markers;
        for i in 0..h.class_defs_size {
            let Some(desc) = h.class_descriptor(data, i) else {
                continue;
            };
            self.class_count += 1;
            m.kotlin |= desc.starts_with(b"Lkotlin/");
            m.compose |= desc.starts_with(b"Landroidx/compose/");
            m.qihoo |=
                desc.starts_with(b"Lcom/qihoo/util/") || desc.starts_with(b"Lcom/tianyu/util/");
            m.secneo |= desc.starts_with(b"Lcom/secneo/apkwrapper/");
            m.flutter_injector |= desc == b"Lio/flutter/FlutterInjector;";
            if let Some(pkg) = package_of(desc) {
                *self.packages.entry(pkg).or_default() += 1;
            }
        }
        if !self.markers.kotlin {
            self.markers.kotlin = h.has_string(data, b"Lkotlin/Metadata;");
        }
        true
    }
}

/// `Lcom/foo/Bar;` → `com.foo`；默认包、非类类型或非 UTF-8 返回 `None`。
fn package_of(desc: &[u8]) -> Option<String> {
    let inner = desc.strip_prefix(b"L")?.strip_suffix(b";")?;
    let slash = inner.iter().rposition(|&b| b == b'/')?;
    let pkg = std::str::from_utf8(&inner[..slash]).ok()?;
    Some(pkg.replace('/', "."))
}

struct Header {
    string_ids_size: usize,
    string_ids_off: usize,
    type_ids_size: usize,
    type_ids_off: usize,
    class_defs_size: usize,
    class_defs_off: usize,
}

fn u32_at(data: &[u8], off: usize) -> Option<u32> {
    let b = data.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

impl Header {
    fn parse(data: &[u8]) -> Option<Self> {
        // "dex\n" + 3 位版本 + "\0"
        if data.len() < 0x70 || &data[..4] != b"dex\n" || data[7] != 0 {
            return None;
        }
        let at = |off| u32_at(data, off).map(|v| v as usize);
        Some(Self {
            string_ids_size: at(0x38)?,
            string_ids_off: at(0x3c)?,
            type_ids_size: at(0x40)?,
            type_ids_off: at(0x44)?,
            class_defs_size: at(0x60)?,
            class_defs_off: at(0x64)?,
        })
    }

    /// 第 `i` 个字符串的 MUTF-8 字节（不含结尾 NUL）。
    fn string<'a>(&self, data: &'a [u8], i: usize) -> Option<&'a [u8]> {
        if i >= self.string_ids_size {
            return None;
        }
        let off = u32_at(data, self.string_ids_off.checked_add(i.checked_mul(4)?)?)? as usize;
        // string_data_item：uleb128 的 UTF-16 长度，然后是 NUL 结尾的 MUTF-8
        let mut p = off;
        loop {
            let b = *data.get(p)?;
            p += 1;
            if b & 0x80 == 0 {
                break;
            }
            if p - off > 5 {
                return None;
            }
        }
        let rest = data.get(p..)?;
        let end = rest.iter().position(|&b| b == 0)?;
        Some(&rest[..end])
    }

    fn class_descriptor<'a>(&self, data: &'a [u8], i: usize) -> Option<&'a [u8]> {
        // class_def_item 32 字节，首字段 class_idx → type_ids[class_idx] = descriptor_idx
        let class_idx =
            u32_at(data, self.class_defs_off.checked_add(i.checked_mul(32)?)?)? as usize;
        if class_idx >= self.type_ids_size {
            return None;
        }
        let desc_idx = u32_at(
            data,
            self.type_ids_off.checked_add(class_idx.checked_mul(4)?)?,
        )? as usize;
        self.string(data, desc_idx)
    }

    /// 字符串表按内容排序，二分查找。
    fn has_string(&self, data: &[u8], needle: &[u8]) -> bool {
        let (mut lo, mut hi) = (0usize, self.string_ids_size);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let Some(s) = self.string(data, mid) else {
                return false;
            };
            match s.cmp(needle) {
                std::cmp::Ordering::Equal => return true,
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
            }
        }
        false
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 造一个只有字符串表、类型表与 class_defs 的最小 dex。`strings` 须已按字节序排好。
    pub(crate) fn build_dex(strings: &[&str], classes: &[usize]) -> Vec<u8> {
        let header = 0x70;
        let string_ids_off = header;
        let type_ids_off = string_ids_off + strings.len() * 4;
        let class_defs_off = type_ids_off + strings.len() * 4;
        let data_off = class_defs_off + classes.len() * 32;
        let mut out = vec![0u8; data_off];
        out[..8].copy_from_slice(b"dex\n035\0");
        let put = |out: &mut Vec<u8>, off: usize, v: usize| {
            out[off..off + 4].copy_from_slice(&(v as u32).to_le_bytes())
        };
        put(&mut out, 0x38, strings.len());
        put(&mut out, 0x3c, string_ids_off);
        // 每个字符串都当一个类型
        put(&mut out, 0x40, strings.len());
        put(&mut out, 0x44, type_ids_off);
        put(&mut out, 0x60, classes.len());
        put(&mut out, 0x64, class_defs_off);
        for (i, s) in strings.iter().enumerate() {
            let off = out.len();
            put(&mut out, string_ids_off + i * 4, off);
            put(&mut out, type_ids_off + i * 4, i);
            out.push(s.len() as u8); // 测试串都短于 128
            out.extend_from_slice(s.as_bytes());
            out.push(0);
        }
        for (i, &t) in classes.iter().enumerate() {
            put(&mut out, class_defs_off + i * 32, t);
        }
        out
    }

    #[test]
    fn dex_entry_names() {
        assert!(is_dex_entry("classes.dex"));
        assert!(is_dex_entry("classes12.dex"));
        assert!(!is_dex_entry("classesx.dex"));
        assert!(!is_dex_entry("assets/classes.dex"));
        assert!(!is_dex_entry("classes.dex.bak"));
    }

    #[test]
    fn collects_packages_and_markers() {
        let strings = [
            "LBare;",
            "Landroidx/compose/ui/Modifier;",
            "Lcom/foo/A;",
            "Lcom/foo/B;",
            "Lcom/foo/sub/C;",
            "Lcom/qihoo/util/StubApp;",
            "Lio/flutter/FlutterInjector;",
            "Lkotlin/Metadata;",
        ];
        // kotlin/Metadata 只出现在字符串表（作为注解类型），不是定义的类
        let dex = build_dex(&strings, &[0, 1, 2, 3, 4, 5, 6]);
        let mut info = DexInfo::default();
        assert!(info.add_dex(&dex));
        assert_eq!(info.dex_count, 1);
        assert_eq!(info.class_count, 7);
        let pkgs: Vec<_> = info
            .packages
            .iter()
            .map(|(k, v)| (k.as_str(), *v))
            .collect();
        assert_eq!(
            pkgs,
            vec![
                ("androidx.compose.ui", 1),
                ("com.foo", 2),
                ("com.foo.sub", 1),
                ("com.qihoo.util", 1),
                ("io.flutter", 1),
            ]
        );
        let m = info.markers;
        assert!(m.kotlin && m.compose && m.qihoo && m.flutter_injector && !m.secneo);
    }

    #[test]
    fn garbage_and_truncated_input_are_rejected_or_partial() {
        let mut info = DexInfo::default();
        assert!(!info.add_dex(b"not a dex"));
        let mut dex = build_dex(&["La/B;"], &[0]);
        // class_defs 指向越界的类型下标：跳过该类，不 panic
        let off = u32_at(&dex, 0x64).unwrap() as usize;
        dex[off..off + 4].copy_from_slice(&999u32.to_le_bytes());
        assert!(info.add_dex(&dex));
        assert_eq!(info.class_count, 0);
        // 截断：表在文件之外
        let short = &build_dex(&["La/B;"], &[0])[..0x80];
        assert!(info.add_dex(short));
        assert_eq!(info.class_count, 0);
    }
}
