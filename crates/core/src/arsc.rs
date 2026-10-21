//! resources.arsc 资源表解析。移植自 Dart 版 `arsc_parser.dart` 与 `arsc/*.dart`。
//!
//! 资源 ID 结构 `0xPPTTEEEE`：PP 包 ID、TT 类型 ID（从 1 开始）、EEEE 条目下标。

use std::collections::BTreeMap;

use crate::axml::read_pool_string;
use crate::model::ResourceInfo;
use crate::reader::{ByteReader, Eof};

#[derive(Debug, thiserror::Error)]
pub enum ArscError {
    #[error("truncated data: {0}")]
    Eof(#[from] crate::reader::Eof),
    #[error("invalid resource table: {0}")]
    Invalid(String),
}

const RES_STRING_POOL_TYPE: u16 = 0x0001;
const RES_TABLE_TYPE: u16 = 0x0002;
const RES_TABLE_TYPE_TYPE: u16 = 0x0201;
const RES_TABLE_TYPE_SPEC_TYPE: u16 = 0x0202;

/// ResTable_type.flags：条目表为 (idx, offset/4) 稀疏对。
const TYPE_FLAG_SPARSE: u8 = 0x01;
/// ResTable_type.flags：条目偏移为 u16（实际偏移 = 值 × 4）。
const TYPE_FLAG_OFFSET16: u8 = 0x02;
const NO_ENTRY: u32 = 0xFFFF_FFFF;
const NO_ENTRY16: u16 = 0xFFFF;

const ENTRY_FLAG_COMPLEX: u16 = 0x0001;
/// ResTable_entry.flags：紧凑条目（key u16 + flags + data u32，类型在 flags 高字节）。
const ENTRY_FLAG_COMPACT: u16 = 0x0008;

// ---------------------------------------------------------------------------
// 字符串池

/// resources.arsc 中的字符串池（全局 / 类型名 / 键名）。
#[derive(Debug, Default, Clone)]
pub struct StringPool {
    strings: Vec<String>,
}

impl StringPool {
    pub fn get(&self, index: u32) -> Option<&str> {
        self.strings.get(index as usize).map(|s| s.as_str())
    }
    pub fn len(&self) -> usize {
        self.strings.len()
    }
    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }

    /// 从 chunk 起始（type 字段）解析；结束后游标位于 chunk 末尾。
    /// 计数损坏时返回空池而不是失败，让后续 package 仍有机会解析。
    pub fn parse(r: &mut ByteReader) -> Result<Self, Eof> {
        let header_start = r.pos();
        let _ty = r.u16()?;
        let _header_size = r.u16()?;
        let total_size = r.u32()? as usize;
        let string_count = r.u32()? as usize;
        let style_count = r.u32()? as usize;
        let flags = r.u32()?;
        let strings_start = r.u32()? as usize;
        let _styles_start = r.u32()?;
        let is_utf8 = flags & 0x100 != 0;
        let chunk_end = header_start.saturating_add(total_size);

        let needed = string_count
            .checked_add(style_count)
            .and_then(|n| n.checked_mul(4));
        if needed.is_none_or(|n| n > r.remain()) {
            r.seek(chunk_end);
            return Ok(Self::default());
        }
        let mut offsets = Vec::with_capacity(string_count);
        for _ in 0..string_count {
            offsets.push(r.u32()? as usize);
        }
        r.skip(style_count * 4)?;

        let data_start = header_start.saturating_add(strings_start);
        let mut strings = Vec::with_capacity(string_count);
        for off in offsets {
            let pos = data_start.saturating_add(off);
            if pos >= chunk_end {
                strings.push(String::new());
                continue;
            }
            r.seek(pos);
            strings.push(read_pool_string(r, is_utf8).unwrap_or_default());
        }
        r.seek(chunk_end);
        Ok(Self { strings })
    }
}

// ---------------------------------------------------------------------------
// 值 / 条目 / 配置

pub const TYPE_NULL: u8 = 0x00;
pub const TYPE_REFERENCE: u8 = 0x01;
pub const TYPE_STRING: u8 = 0x03;
pub const TYPE_INT_COLOR_ARGB8: u8 = 0x1c;
pub const TYPE_INT_COLOR_RGB8: u8 = 0x1d;
pub const TYPE_INT_COLOR_ARGB4: u8 = 0x1e;
pub const TYPE_INT_COLOR_RGB4: u8 = 0x1f;

/// Res_value。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceValue {
    pub size: u16,
    pub data_type: u8,
    pub data: u32,
}

impl ResourceValue {
    pub fn read(r: &mut ByteReader) -> Result<Self, Eof> {
        let size = r.u16()?;
        let _res0 = r.u8()?;
        let data_type = r.u8()?;
        let data = r.u32()?;
        Ok(Self {
            size,
            data_type,
            data,
        })
    }
    pub fn is_string(&self) -> bool {
        self.data_type == TYPE_STRING
    }
    pub fn is_reference(&self) -> bool {
        self.data_type == TYPE_REFERENCE
    }
    pub fn is_color(&self) -> bool {
        matches!(
            self.data_type,
            TYPE_INT_COLOR_ARGB8 | TYPE_INT_COLOR_RGB8 | TYPE_INT_COLOR_ARGB4 | TYPE_INT_COLOR_RGB4
        )
    }

    /// 颜色值转 `#aarrggbb`；4 位分量按 ×17 扩展到 8 位。
    pub fn color_hex(&self) -> Option<String> {
        let d = self.data;
        let n = |shift: u32| ((d >> shift) & 0xF) * 17;
        match self.data_type {
            TYPE_INT_COLOR_ARGB8 => Some(format!("#{d:08x}")),
            TYPE_INT_COLOR_RGB8 => Some(format!("#ff{:06x}", d & 0xFF_FFFF)),
            TYPE_INT_COLOR_ARGB4 => Some(format!(
                "#{:02x}{:02x}{:02x}{:02x}",
                n(12),
                n(8),
                n(4),
                n(0)
            )),
            TYPE_INT_COLOR_RGB4 => Some(format!("#ff{:02x}{:02x}{:02x}", n(8), n(4), n(0))),
            _ => None,
        }
    }
}

/// ResTable_entry（简单条目带 `value`，复杂/bag 条目带 `map_values`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceEntry {
    pub flags: u16,
    pub key_index: u32,
    pub value: Option<ResourceValue>,
    pub map_values: Vec<(u32, ResourceValue)>,
}

impl ResourceEntry {
    pub fn is_complex(&self) -> bool {
        self.flags & ENTRY_FLAG_COMPLEX != 0
    }
}

/// ResTable_config 中解析器关心的部分。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceConfig {
    pub size: u32,
    pub mcc: u16,
    pub mnc: u16,
    pub language: String,
    pub country: String,
    pub script: String,
    pub variant: String,
    pub density: u32,
}

impl ResourceConfig {
    /// `language[-Script][-COUNTRY]`；无语言时为空串（默认配置）。
    pub fn locale(&self) -> String {
        if self.language.is_empty() {
            return String::new();
        }
        let mut parts = vec![self.language.as_str()];
        if !self.script.is_empty() {
            parts.push(&self.script);
        }
        if !self.country.is_empty() {
            parts.push(&self.country);
        }
        parts.join("-")
    }

    /// 读取 config；结束后游标位于 config 末尾（起点 + size）。
    /// 混淆 APK 可能使用截短的 config（如 size=16），只读 size 覆盖到的字段。
    pub fn read(r: &mut ByteReader) -> Result<Self, Eof> {
        let start = r.pos();
        let size = r.u32()?;
        if size < 8 {
            if size > 4 {
                r.seek(start + size as usize);
            }
            return Ok(Self {
                size,
                ..Default::default()
            });
        }
        let mut c = Self {
            size,
            mcc: r.u16()?,
            mnc: r.u16()?,
            ..Default::default()
        };
        if size >= 12 {
            let lang = r.bytes(2)?;
            let country = r.bytes(2)?;
            c.language = decode_locale_chars([lang[0], lang[1]], b'a');
            c.country = decode_locale_chars([country[0], country[1]], b'0');
        }
        if size >= 16 {
            r.skip(2)?; // orientation + touchscreen
            c.density = r.u16()? as u32;
        }
        if size >= 40 {
            r.seek(start + 36);
            c.script = ascii_non_nul(r.bytes(4)?);
        }
        if size >= 48 {
            r.seek(start + 40);
            c.variant = ascii_non_nul(r.bytes(8)?);
        }
        r.seek(start + size as usize);
        Ok(c)
    }
}

fn ascii_non_nul(bytes: &[u8]) -> String {
    bytes
        .iter()
        .filter(|&&b| b != 0)
        .map(|&b| b as char)
        .collect()
}

/// 解码 2 字节语言/地区码，支持 BCP-47 packed 3 字母码。
///
/// packed 时 byte0 最高位置位，15 位里每 5 位一个字符。语言码 base 为 'a'；
/// 区域码按 Android `unpackRegion` base 为 '0'（3 位数字区域如 `419`）。
/// 位序按 Android `ResTable_config::unpackLanguageOrRegion`：第一个字符在 byte1 低 5 位。
/// Dart 版位序取反且区域 base 用了 'A'，会把 `ast` 解成 `tsa`、`es-419` 解成 `es-EBJ`，这里按 Android 源码修正。
fn decode_locale_chars(b: [u8; 2], base: u8) -> String {
    if b == [0, 0] {
        return String::new();
    }
    if b[0] & 0x80 != 0 {
        let first = b[1] & 0x1f;
        let second = ((b[1] & 0xe0) >> 5) | ((b[0] & 0x03) << 3);
        let third = (b[0] & 0x7c) >> 2;
        return [first, second, third]
            .iter()
            .map(|&x| (base + x) as char)
            .collect();
    }
    ascii_non_nul(&b)
}

/// 某个类型在某个配置下的全部条目。
#[derive(Debug, Clone, Default)]
pub struct TypeConfig {
    pub config: ResourceConfig,
    pub entries: BTreeMap<u32, ResourceEntry>,
}

/// 一个资源类型（string / drawable / mipmap …）及其全部配置。
#[derive(Debug, Clone, Default)]
pub struct TypeSpec {
    pub id: u8,
    pub name: String,
    pub entry_flags: Vec<u32>,
    pub configs: Vec<TypeConfig>,
}

#[derive(Debug, Clone, Default)]
pub struct Package {
    pub id: u32,
    pub name: String,
    pub type_strings: StringPool,
    pub key_strings: StringPool,
    /// 保持出现顺序（locale / density 列表顺序依赖它）。
    pub type_specs: Vec<TypeSpec>,
}

impl Package {
    pub fn find_type_spec(&self, type_id: u8) -> Option<&TypeSpec> {
        self.type_specs.iter().find(|t| t.id == type_id)
    }
    fn type_spec_mut_or_insert(&mut self, type_id: u8) -> &mut TypeSpec {
        let idx = match self.type_specs.iter().position(|t| t.id == type_id) {
            Some(i) => i,
            None => {
                let name = self.type_name(type_id);
                self.type_specs.push(TypeSpec {
                    id: type_id,
                    name,
                    ..Default::default()
                });
                self.type_specs.len() - 1
            }
        };
        &mut self.type_specs[idx]
    }
    fn type_name(&self, type_id: u8) -> String {
        (type_id as u32)
            .checked_sub(1)
            .and_then(|i| self.type_strings.get(i))
            .map(str::to_string)
            .unwrap_or_else(|| format!("type{type_id}"))
    }
}

// ---------------------------------------------------------------------------
// 资源表

/// 已解析的资源表。
#[derive(Debug, Default)]
pub struct ResourceTable {
    global_strings: StringPool,
    packages: Vec<Package>,
}

fn split_id(resource_id: u32) -> (u32, u8, u32) {
    (
        (resource_id >> 24) & 0xFF,
        ((resource_id >> 16) & 0xFF) as u8,
        resource_id & 0xFFFF,
    )
}

impl ResourceTable {
    /// 解析资源表。表头或全局字符串池损坏时报错；单个 package / chunk 损坏则跳过。
    pub fn parse(data: &[u8]) -> Result<Self, ArscError> {
        let mut r = ByteReader::new(data);
        let ty = r.u16()?;
        if ty != RES_TABLE_TYPE {
            // Dart 版不校验类型；这里同样只记录，继续尽力解析
            log::debug!("arsc: unexpected table type 0x{ty:04x}");
        }
        let header_size = r.u16()? as usize;
        let _total = r.u32()?;
        let package_count = r.u32()?;
        if header_size > 12 {
            r.skip(header_size - 12)?;
        }

        // 跳过混淆器注入的 NULL chunk，寻找全局字符串池
        skip_null_chunks(&mut r, data.len());
        let global_strings = StringPool::parse(&mut r)?;

        let mut packages = Vec::new();
        for _ in 0..package_count {
            if r.remain() == 0 {
                break;
            }
            match parse_package(&mut r) {
                Ok(p) => packages.push(p),
                Err(e) => {
                    log::debug!("arsc: package parse failed: {e}");
                    break;
                }
            }
        }
        Ok(Self {
            global_strings,
            packages,
        })
    }

    pub fn packages(&self) -> &[Package] {
        &self.packages
    }

    pub fn global_strings(&self) -> &StringPool {
        &self.global_strings
    }

    fn find_type_spec(&self, resource_id: u32) -> Option<(&TypeSpec, u32)> {
        let (pkg, ty, entry) = split_id(resource_id);
        let package = self.packages.iter().find(|p| p.id == pkg)?;
        Some((package.find_type_spec(ty)?, entry))
    }

    /// 条目的字符串值所指向的全局字符串（`Some(None)` 表示是字符串类型但索引越界）。
    fn entry_string(&self, cfg: &TypeConfig, entry: u32) -> Option<Option<String>> {
        let v = cfg.entries.get(&entry)?.value?;
        if !v.is_string() {
            return None;
        }
        Some(self.global_strings.get(v.data).map(str::to_string))
    }

    /// 按资源 ID 取字符串值；`locale` 为 `None` 时取默认配置（找不到再退任意配置）。
    pub fn string_value(&self, resource_id: u32, locale: Option<&str>) -> Option<String> {
        let (spec, entry) = self.find_type_spec(resource_id)?;
        let locale = locale.filter(|l| !l.is_empty());
        for cfg in &spec.configs {
            let cl = cfg.config.locale();
            let matched = match locale {
                Some(l) => cl == l,
                None => cl.is_empty(),
            };
            if !matched {
                continue;
            }
            // 与 Dart 一致：命中字符串类型条目即返回（即使索引越界返回 None）
            if let Some(s) = self.entry_string(cfg, entry) {
                return s;
            }
        }
        if locale.is_none() {
            for cfg in &spec.configs {
                if let Some(s) = self.entry_string(cfg, entry) {
                    return s;
                }
            }
        }
        None
    }

    /// 按资源 ID 取文件路径：优先指定 density → 默认 density → 任意配置。
    pub fn file_path(&self, resource_id: u32, density: Option<u32>) -> Option<String> {
        let (spec, entry) = self.find_type_spec(resource_id)?;
        if let Some(d) = density {
            for cfg in spec.configs.iter().filter(|c| c.config.density == d) {
                if let Some(s) = self.entry_string(cfg, entry) {
                    return s;
                }
            }
        }
        for cfg in &spec.configs {
            if density.is_some() && cfg.config.density != 0 {
                continue;
            }
            if let Some(s) = self.entry_string(cfg, entry) {
                return s;
            }
        }
        for cfg in &spec.configs {
            if let Some(s) = self.entry_string(cfg, entry) {
                return s;
            }
        }
        None
    }

    /// 资源 ID 的所有 density 变体文件路径（已过滤垃圾路径）。
    pub fn all_file_paths(&self, resource_id: u32) -> Vec<(u32, String)> {
        let Some((spec, entry)) = self.find_type_spec(resource_id) else {
            return Vec::new();
        };
        spec.configs
            .iter()
            .filter_map(|cfg| {
                let path = self.entry_string(cfg, entry)??;
                is_valid_resource_path(&path).then_some((cfg.config.density, path))
            })
            .collect()
    }

    /// 全部非默认 locale，按首次出现顺序去重。
    pub fn locales(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for cfg in self.all_configs() {
            let l = cfg.locale();
            if !l.is_empty() && !out.contains(&l) {
                out.push(l);
            }
        }
        out
    }

    /// 全部有效 density（排除 0=默认 与 >=0xFFFE 的 any/none 哨兵值），按首次出现顺序去重。
    pub fn densities(&self) -> Vec<u32> {
        let mut out = Vec::new();
        for cfg in self.all_configs() {
            let d = cfg.density;
            if d > 0 && d < 0xFFFE && !out.contains(&d) {
                out.push(d);
            }
        }
        out
    }

    fn all_configs(&self) -> impl Iterator<Item = &ResourceConfig> {
        self.packages
            .iter()
            .flat_map(|p| p.type_specs.iter())
            .flat_map(|t| t.configs.iter())
            .map(|c| &c.config)
    }

    /// 导出所有资源条目（按 ID 聚合），供图标渲染器使用。
    pub fn all_resources(&self) -> BTreeMap<u32, ResourceInfo> {
        let mut results: BTreeMap<u32, ResourceInfo> = BTreeMap::new();
        for package in &self.packages {
            for spec in &package.type_specs {
                let type_name = package.type_name(spec.id);
                for cfg in &spec.configs {
                    for (&entry_index, entry) in &cfg.entries {
                        let resource_id =
                            (package.id << 24) | ((spec.id as u32) << 16) | entry_index;
                        let info = results.entry(resource_id).or_insert_with(|| ResourceInfo {
                            resource_id,
                            type_name: type_name.clone(),
                            key_name: package
                                .key_strings
                                .get(entry.key_index)
                                .map(str::to_string)
                                .unwrap_or_else(|| format!("key{entry_index}")),
                            ..Default::default()
                        });
                        let Some(v) = entry.value else { continue };
                        if v.is_string() {
                            if let Some(s) = self.global_strings.get(v.data) {
                                if !info.file_paths.iter().any(|p| p == s) {
                                    info.file_paths.push(s.to_string());
                                }
                            }
                        } else if v.is_color() {
                            if let Some(c) = v.color_hex() {
                                if !info.colors.contains(&c) {
                                    info.colors.push(c);
                                }
                            }
                        } else if v.is_reference()
                            && v.data != 0
                            && !info.references.contains(&v.data)
                        {
                            info.references.push(v.data);
                        }
                    }
                }
            }
        }
        results
    }
}

/// 合法 Android 资源路径。混淆 APK 的资源表可能含垃圾路径
/// （如 `AndroidManifest.xml/uF.xml`、`classes.dex.png`），需要过滤。
///
/// Dart 版还要求 `res/` 前缀；但资源路径混淆（如微信输入法的 `r/a3/a.png`）是合法做法，
/// 那样会丢掉全部图标，所以这里只要求是包内某个目录下的相对路径。
fn is_valid_resource_path(path: &str) -> bool {
    match path.rsplit_once('/') {
        Some((dir, file)) if !dir.is_empty() && !file.is_empty() && !path.starts_with('/') => {}
        _ => return false,
    }
    let lower = path.to_lowercase();
    if lower.contains("classes.dex")
        || lower.contains("androidmanifest.xml")
        || lower.contains("resources.arsc")
    {
        return false;
    }
    [".png", ".jpg", ".jpeg", ".webp", ".gif", ".xml"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// 跳过混淆器注入的非字符串池 chunk（如 type=0 的 NULL chunk），直到遇到字符串池或边界。
fn skip_null_chunks(r: &mut ByteReader, boundary: usize) {
    while r.pos() + 8 <= boundary {
        let pos = r.pos();
        let (Some(ty), Some(size)) = (r.peek_u16(), r.peek_u32_at(pos + 4)) else {
            break;
        };
        if ty == RES_STRING_POOL_TYPE {
            break;
        }
        let size = size as usize;
        if size < 8 || pos + size > boundary {
            break;
        }
        r.seek(pos + size);
    }
}

fn parse_package(r: &mut ByteReader) -> Result<Package, Eof> {
    let start = r.pos();
    let _ty = r.u16()?;
    let header_size = r.u16()? as usize;
    let total_size = r.u32()? as usize;
    let end = start.saturating_add(total_size);
    let id = r.u32()?;

    // 包名：128 个 UTF-16 码元，NUL 截断
    let mut name = crate::axml::decode_utf16le(r.bytes(256)?);
    if let Some(i) = name.find('\0') {
        name.truncate(i);
    }
    let type_strings_off = r.u32()? as usize;
    let _last_public_type = r.u32()?;
    let key_strings_off = r.u32()? as usize;
    let _last_public_key = r.u32()?;
    let read = r.pos() - start;
    if header_size > read {
        r.skip(header_size - read)?;
    }

    r.seek(start.saturating_add(type_strings_off));
    skip_null_chunks(r, end);
    let type_strings = StringPool::parse(r)?;
    r.seek(start.saturating_add(key_strings_off));
    skip_null_chunks(r, end);
    let key_strings = StringPool::parse(r)?;

    let mut package = Package {
        id,
        name,
        type_strings,
        key_strings,
        type_specs: Vec::new(),
    };

    while r.pos() < end && r.remain() >= 8 {
        let chunk_start = r.pos();
        let chunk_type = r.u16()?;
        let chunk_header = r.u16()? as usize;
        let chunk_size = r.u32()? as usize;
        if chunk_size < 8 {
            break;
        }
        // 单个 chunk 损坏只丢弃它本身，继续解析后续 chunk
        let res = match chunk_type {
            RES_TABLE_TYPE_SPEC_TYPE => parse_type_spec(r, &mut package),
            RES_TABLE_TYPE_TYPE => parse_type(r, chunk_start, chunk_header, &mut package),
            _ => Ok(()),
        };
        if let Err(e) = res {
            log::debug!("arsc: chunk 0x{chunk_type:04x} at {chunk_start} skipped: {e}");
        }
        r.seek(chunk_start.saturating_add(chunk_size));
    }
    r.seek(end);
    Ok(package)
}

fn parse_type_spec(r: &mut ByteReader, package: &mut Package) -> Result<(), Eof> {
    let type_id = r.u8()?;
    r.skip(3)?; // res0 + res1
    let count = r.u32()? as usize;
    // 先确认数据足够再分配，防止损坏的计数导致巨量分配
    if count.saturating_mul(4) > r.remain() {
        return Err(Eof {
            pos: r.pos(),
            want: count.saturating_mul(4),
            len: r.len(),
        });
    }
    let mut flags = Vec::with_capacity(count);
    for _ in 0..count {
        flags.push(r.u32()?);
    }
    let spec = package.type_spec_mut_or_insert(type_id);
    if spec.entry_flags.is_empty() {
        spec.entry_flags = flags;
    }
    Ok(())
}

fn parse_type(
    r: &mut ByteReader,
    chunk_start: usize,
    header_size: usize,
    package: &mut Package,
) -> Result<(), Eof> {
    let type_id = r.u8()?;
    let type_flags = r.u8()?;
    r.skip(2)?; // reserved
    let entry_count = r.u32()? as usize;
    let entries_start = r.u32()? as usize;
    let config = ResourceConfig::read(r)?;

    r.seek(chunk_start + header_size);
    // (条目下标, 相对 entries_start 的偏移)。Dart 版只支持普通 u32 偏移表；
    // 这里额外支持 aapt2 的稀疏表与 16 位偏移表，否则这类 APK 会读到错位数据。
    let unit = if type_flags & (TYPE_FLAG_SPARSE | TYPE_FLAG_OFFSET16) != 0 {
        2
    } else {
        4
    };
    let table_bytes = entry_count.saturating_mul(if type_flags & TYPE_FLAG_SPARSE != 0 {
        4
    } else {
        unit
    });
    if table_bytes > r.remain() {
        return Err(Eof {
            pos: r.pos(),
            want: table_bytes,
            len: r.len(),
        });
    }
    let mut offsets: Vec<(u32, usize)> = Vec::with_capacity(entry_count);
    for i in 0..entry_count {
        if type_flags & TYPE_FLAG_SPARSE != 0 {
            let idx = r.u16()? as u32;
            let off = r.u16()? as usize * 4;
            offsets.push((idx, off));
        } else if type_flags & TYPE_FLAG_OFFSET16 != 0 {
            let off = r.u16()?;
            if off != NO_ENTRY16 {
                offsets.push((i as u32, off as usize * 4));
            }
        } else {
            let off = r.u32()?;
            if off != NO_ENTRY {
                offsets.push((i as u32, off as usize));
            }
        }
    }

    let data_start = chunk_start + entries_start;
    let mut entries = BTreeMap::new();
    for (idx, off) in offsets {
        r.seek(data_start.saturating_add(off));
        entries.insert(idx, read_entry(r)?);
    }

    package
        .type_spec_mut_or_insert(type_id)
        .configs
        .push(TypeConfig { config, entries });
    Ok(())
}

fn read_entry(r: &mut ByteReader) -> Result<ResourceEntry, Eof> {
    let first = r.u16()?; // 普通条目是 size；紧凑条目是 key
    let flags = r.u16()?;
    if flags & ENTRY_FLAG_COMPACT != 0 {
        let data = r.u32()?;
        let value = ResourceValue {
            size: 8,
            data_type: (flags >> 8) as u8,
            data,
        };
        return Ok(ResourceEntry {
            flags,
            key_index: first as u32,
            value: Some(value),
            map_values: Vec::new(),
        });
    }
    let key_index = r.u32()?;
    if flags & ENTRY_FLAG_COMPLEX != 0 {
        let _parent = r.u32()?;
        let count = r.u32()?;
        let mut map_values = Vec::new();
        for _ in 0..count {
            let name = r.u32()?;
            map_values.push((name, ResourceValue::read(r)?));
        }
        return Ok(ResourceEntry {
            flags,
            key_index,
            value: None,
            map_values,
        });
    }
    let value = ResourceValue::read(r)?;
    Ok(ResourceEntry {
        flags,
        key_index,
        value: Some(value),
        map_values: Vec::new(),
    })
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axml::tests::{string_pool_utf16, string_pool_utf8};

    fn u16le(v: &mut Vec<u8>, x: u16) {
        v.extend_from_slice(&x.to_le_bytes());
    }
    fn u32le(v: &mut Vec<u8>, x: u32) {
        v.extend_from_slice(&x.to_le_bytes());
    }
    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// Android `ResTable_config::packLanguage` 的独立实现，用来生成测试输入。
    fn pack3(s: &str, base: u8) -> [u8; 2] {
        let b = s.as_bytes();
        let (first, second, third) = (b[0] - base, b[1] - base, b[2] - base);
        [
            0x80 | (third << 2) | (second >> 3),
            ((second & 0x07) << 5) | first,
        ]
    }

    fn config_bytes(lang: [u8; 2], country: [u8; 2], density: u16, script: &[u8; 4]) -> Vec<u8> {
        let mut c = vec![0u8; 64];
        c[0..4].copy_from_slice(&64u32.to_le_bytes());
        c[8..10].copy_from_slice(&lang);
        c[10..12].copy_from_slice(&country);
        c[14..16].copy_from_slice(&density.to_le_bytes());
        c[36..40].copy_from_slice(script);
        c
    }

    fn read_config(bytes: &[u8]) -> ResourceConfig {
        let mut r = ByteReader::new(bytes);
        let c = ResourceConfig::read(&mut r).unwrap();
        assert_eq!(r.pos(), c.size as usize);
        c
    }

    #[test]
    fn config_decodes_plain_packed_and_script_locales() {
        assert_eq!(
            read_config(&config_bytes(*b"zh", *b"CN", 0, b"Hans")).locale(),
            "zh-Hans-CN"
        );
        assert_eq!(
            read_config(&config_bytes(*b"sr", [0, 0], 0, b"Latn")).locale(),
            "sr-Latn"
        );
        let ast = read_config(&config_bytes(pack3("ast", b'a'), [0, 0], 0, &[0; 4]));
        assert_eq!(ast.locale(), "ast");
        let latam = read_config(&config_bytes(*b"es", pack3("419", b'0'), 0, &[0; 4]));
        assert_eq!(latam.locale(), "es-419");
        let d = read_config(&config_bytes([0, 0], [0, 0], 480, &[0; 4]));
        assert_eq!((d.locale(), d.density), (String::new(), 480));
    }

    #[test]
    fn config_tolerates_truncated_size() {
        // size=16：仍能读到语言与 density，但没有 script
        let mut c = config_bytes(*b"fr", *b"FR", 240, b"Latn");
        c[0..4].copy_from_slice(&16u32.to_le_bytes());
        let cfg = read_config(&c);
        assert_eq!((cfg.locale().as_str(), cfg.density), ("fr-FR", 240));
        // size<8：只返回 size
        let mut tiny = vec![0u8; 8];
        tiny[0..4].copy_from_slice(&6u32.to_le_bytes());
        assert_eq!(read_config(&tiny).language, "");
    }

    #[test]
    fn color_hex_formats() {
        let v = |t, d| ResourceValue {
            size: 8,
            data_type: t,
            data: d,
        };
        assert_eq!(
            v(TYPE_INT_COLOR_ARGB8, 0x80ff0000).color_hex().unwrap(),
            "#80ff0000"
        );
        assert_eq!(
            v(TYPE_INT_COLOR_RGB8, 0xff00ff00).color_hex().unwrap(),
            "#ff00ff00"
        );
        assert_eq!(
            v(TYPE_INT_COLOR_ARGB4, 0x8f0a).color_hex().unwrap(),
            "#88ff00aa"
        );
        assert_eq!(
            v(TYPE_INT_COLOR_RGB4, 0x0f0a).color_hex().unwrap(),
            "#ffff00aa"
        );
        assert_eq!(v(TYPE_STRING, 1).color_hex(), None);
    }

    #[test]
    fn string_pool_parses_utf8_and_utf16_and_bad_counts() {
        for pool in [
            string_pool_utf8(&strs(&["a", "中文"])),
            string_pool_utf16(&strs(&["a", "中文"])),
        ] {
            let mut r = ByteReader::new(&pool);
            let p = StringPool::parse(&mut r).unwrap();
            assert_eq!(
                (p.get(0), p.get(1), p.get(2)),
                (Some("a"), Some("中文"), None)
            );
            assert_eq!(r.pos(), pool.len());
        }
        let mut bad = string_pool_utf8(&strs(&["a"]));
        bad[8..12].copy_from_slice(&0x4000_0000u32.to_le_bytes());
        assert!(StringPool::parse(&mut ByteReader::new(&bad))
            .unwrap()
            .is_empty());
    }

    // ---- 合成资源表

    enum Entries {
        /// 普通 u32 偏移表；None 表示 NO_ENTRY
        Dense(Vec<Option<(u32, u8, u32)>>),
        /// 稀疏表：(idx, key, type, data)
        Sparse(Vec<(u16, u32, u8, u32)>),
        /// 紧凑条目 + 16 位偏移
        Compact(Vec<(u16, u8, u32)>),
    }

    fn type_chunk(type_id: u8, config: &[u8], entries: &Entries) -> Vec<u8> {
        let header = 20 + config.len();
        let (flags, table, data, count) = match entries {
            Entries::Dense(list) => {
                let (mut table, mut data) = (Vec::new(), Vec::new());
                for e in list {
                    match e {
                        None => u32le(&mut table, NO_ENTRY),
                        Some((key, ty, val)) => {
                            u32le(&mut table, data.len() as u32);
                            u16le(&mut data, 8);
                            u16le(&mut data, 0);
                            u32le(&mut data, *key);
                            u16le(&mut data, 8);
                            data.push(0);
                            data.push(*ty);
                            u32le(&mut data, *val);
                        }
                    }
                }
                (0u8, table, data, list.len())
            }
            Entries::Sparse(list) => {
                let (mut table, mut data) = (Vec::new(), Vec::new());
                for (idx, key, ty, val) in list {
                    u16le(&mut table, *idx);
                    u16le(&mut table, (data.len() / 4) as u16);
                    u16le(&mut data, 8);
                    u16le(&mut data, 0);
                    u32le(&mut data, *key);
                    u16le(&mut data, 8);
                    data.push(0);
                    data.push(*ty);
                    u32le(&mut data, *val);
                }
                (TYPE_FLAG_SPARSE, table, data, list.len())
            }
            Entries::Compact(list) => {
                let (mut table, mut data) = (Vec::new(), Vec::new());
                for (key, ty, val) in list {
                    u16le(&mut table, (data.len() / 4) as u16);
                    u16le(&mut data, *key);
                    u16le(&mut data, ENTRY_FLAG_COMPACT | ((*ty as u16) << 8));
                    u32le(&mut data, *val);
                }
                (TYPE_FLAG_OFFSET16, table, data, list.len())
            }
        };
        let mut table = table;
        while table.len() % 4 != 0 {
            table.push(0);
        }
        let entries_start = header + table.len();
        let mut c = Vec::new();
        u16le(&mut c, RES_TABLE_TYPE_TYPE);
        u16le(&mut c, header as u16);
        u32le(&mut c, (entries_start + data.len()) as u32);
        c.push(type_id);
        c.push(flags);
        u16le(&mut c, 0);
        u32le(&mut c, count as u32);
        u32le(&mut c, entries_start as u32);
        c.extend_from_slice(config);
        c.extend(table);
        c.extend(data);
        c
    }

    fn type_spec_chunk(type_id: u8, count: u32) -> Vec<u8> {
        let mut c = Vec::new();
        u16le(&mut c, RES_TABLE_TYPE_SPEC_TYPE);
        u16le(&mut c, 16);
        u32le(&mut c, 16 + 4 * count);
        c.push(type_id);
        c.push(0);
        u16le(&mut c, 0);
        u32le(&mut c, count);
        for _ in 0..count {
            u32le(&mut c, 0);
        }
        c
    }

    fn table(
        global: &[&str],
        types: &[&str],
        keys: &[&str],
        chunks: &[Vec<u8>],
        null_chunk: bool,
    ) -> Vec<u8> {
        let type_pool = string_pool_utf16(&strs(types));
        let key_pool = string_pool_utf8(&strs(keys));
        let pkg_header = 288usize;
        let mut pkg = Vec::new();
        u16le(&mut pkg, 0x0200);
        u16le(&mut pkg, pkg_header as u16);
        let body_len: usize =
            type_pool.len() + key_pool.len() + chunks.iter().map(Vec::len).sum::<usize>();
        u32le(&mut pkg, (pkg_header + body_len) as u32);
        u32le(&mut pkg, 0x7f);
        let mut name = [0u8; 256];
        for (i, u) in "com.example".encode_utf16().enumerate() {
            name[i * 2..i * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
        pkg.extend_from_slice(&name);
        u32le(&mut pkg, pkg_header as u32);
        u32le(&mut pkg, 0);
        u32le(&mut pkg, (pkg_header + type_pool.len()) as u32);
        u32le(&mut pkg, 0);
        u32le(&mut pkg, 0); // typeIdOffset，凑满 288 字节头
        pkg.extend(type_pool);
        pkg.extend(key_pool);
        for c in chunks {
            pkg.extend_from_slice(c);
        }

        let global_pool = string_pool_utf8(&strs(global));
        let mut nul = Vec::new();
        if null_chunk {
            u16le(&mut nul, 0);
            u16le(&mut nul, 8);
            u32le(&mut nul, 16);
            nul.extend_from_slice(&[0xAA; 8]);
        }
        let mut t = Vec::new();
        u16le(&mut t, RES_TABLE_TYPE);
        u16le(&mut t, 12);
        u32le(
            &mut t,
            (12 + nul.len() + global_pool.len() + pkg.len()) as u32,
        );
        u32le(&mut t, 1);
        t.extend(nul);
        t.extend(global_pool);
        t.extend(pkg);
        t
    }

    fn sample_table() -> ResourceTable {
        let global = [
            "App",
            "应用",
            "res/mipmap-mdpi/ic.png",
            "res/mipmap-xxhdpi/ic.png",
            "classes.dex.png",
            "Ast App",
        ];
        let none = [0u8, 0];
        let chunks = vec![
            type_spec_chunk(1, 1),
            type_chunk(
                1,
                &config_bytes(none, none, 0, &[0; 4]),
                &Entries::Dense(vec![Some((0, TYPE_STRING, 0))]),
            ),
            type_chunk(
                1,
                &config_bytes(*b"zh", *b"CN", 0, &[0; 4]),
                &Entries::Dense(vec![Some((0, TYPE_STRING, 1))]),
            ),
            type_chunk(
                1,
                &config_bytes(pack3("ast", b'a'), none, 0, &[0; 4]),
                &Entries::Sparse(vec![(0, 0, TYPE_STRING, 5)]),
            ),
            type_spec_chunk(2, 3),
            type_chunk(
                2,
                &config_bytes(none, none, 160, &[0; 4]),
                &Entries::Dense(vec![
                    Some((1, TYPE_STRING, 2)),
                    None,
                    Some((2, TYPE_INT_COLOR_RGB8, 0x123456)),
                ]),
            ),
            type_chunk(
                2,
                &config_bytes(none, none, 480, &[0; 4]),
                &Entries::Compact(vec![(1, TYPE_STRING, 3)]),
            ),
            type_chunk(
                2,
                &config_bytes(none, none, 0xFFFF, &[0; 4]),
                &Entries::Dense(vec![Some((1, TYPE_STRING, 4))]),
            ),
            type_chunk(
                2,
                &config_bytes(none, none, 0, &[0; 4]),
                &Entries::Dense(vec![None, Some((3, TYPE_REFERENCE, 0x7f010000))]),
            ),
        ];
        ResourceTable::parse(&table(
            &global,
            &["string", "mipmap"],
            &["app_name", "ic_launcher", "accent", "alias"],
            &chunks,
            true,
        ))
        .unwrap()
    }

    #[test]
    fn synthetic_table_queries() {
        let t = sample_table();
        assert_eq!(t.packages()[0].name, "com.example");
        assert_eq!(t.string_value(0x7f010000, None).as_deref(), Some("App"));
        assert_eq!(
            t.string_value(0x7f010000, Some("zh-CN")).as_deref(),
            Some("应用")
        );
        assert_eq!(
            t.string_value(0x7f010000, Some("ast")).as_deref(),
            Some("Ast App")
        );
        assert_eq!(t.string_value(0x7f010000, Some("fr")), None);
        assert_eq!(t.string_value(0x7f990000, None), None);
        assert_eq!(t.locales(), vec!["zh-CN", "ast"]);
        assert_eq!(t.densities(), vec![160, 480]);
        assert_eq!(
            t.all_file_paths(0x7f020000),
            vec![
                (160, "res/mipmap-mdpi/ic.png".to_string()),
                (480, "res/mipmap-xxhdpi/ic.png".to_string())
            ]
        );
        assert_eq!(
            t.file_path(0x7f020000, Some(480)).as_deref(),
            Some("res/mipmap-xxhdpi/ic.png")
        );
        assert_eq!(
            t.file_path(0x7f020000, Some(320)).as_deref(),
            Some("res/mipmap-mdpi/ic.png")
        );

        let all = t.all_resources();
        let ic = &all[&0x7f020000];
        assert_eq!(ic.name(), "mipmap/ic_launcher");
        assert_eq!(
            ic.file_paths,
            vec![
                "res/mipmap-mdpi/ic.png",
                "res/mipmap-xxhdpi/ic.png",
                "classes.dex.png"
            ]
        );
        assert_eq!(all[&0x7f020002].colors, vec!["#ff123456"]);
        assert_eq!(all[&0x7f020001].references, vec![0x7f010000]);
        assert_eq!(all[&0x7f020001].key_name, "alias");
        assert_eq!(all[&0x7f010000].id_hex(), "0x7f010000");
    }

    #[test]
    fn rejects_garbage_paths() {
        assert!(is_valid_resource_path("res/a.9.png"));
        assert!(is_valid_resource_path("res/drawable/x.XML"));
        assert!(!is_valid_resource_path("AndroidManifest.xml/uF.xml"));
        assert!(!is_valid_resource_path("res/classes.dex.png"));
        assert!(!is_valid_resource_path("res/raw/a.mp3"));
        assert!(is_valid_resource_path("r/a3/a.png"));
        assert!(!is_valid_resource_path("icon.png"));
        assert!(!is_valid_resource_path("/res/a.png"));
    }

    #[test]
    fn truncated_table_is_error_or_partial() {
        assert!(ResourceTable::parse(&[2, 0, 12]).is_err());
        let full = table(&["x"], &["string"], &["k"], &[type_spec_chunk(1, 1)], false);
        // 截掉 package 尾部：仍能得到全局池，不 panic
        let t = ResourceTable::parse(&full[..full.len() - 10]).unwrap();
        assert_eq!(t.global_strings().get(0), Some("x"));
    }
}
