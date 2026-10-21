//! Android 二进制 XML（AXML）解析：字符串池、资源 ID 映射、元素树；含混淆 APK 容错。
//! 移植自 Dart 版 `packages/apk_parser/lib/src/binary_xml.dart`
//! （其上游为 ClassyShark 的 XmlDecompressor）。

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::reader::ByteReader;

/// 解析后的元素。属性值已按类型转成字符串（引用形如 `@res/0x7f010000`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct XmlElement {
    pub name: String,
    /// 使用 BTreeMap 保证遍历顺序稳定（启发式查找依赖确定性）。
    pub attributes: BTreeMap<String, String>,
    pub children: Vec<XmlElement>,
    /// 混淆属性中通过非标准字段（如 ns）恢复的额外字符串值，供 Manifest 启发式使用。
    pub extra_strings: Vec<String>,
}

impl XmlElement {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attributes.get(name).map(|s| s.as_str())
    }
    pub fn child(&self, name: &str) -> Option<&XmlElement> {
        self.children.iter().find(|c| c.name == name)
    }
    pub fn children_named<'a>(
        &'a self,
        name: &'a str,
    ) -> impl Iterator<Item = &'a XmlElement> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AxmlError {
    #[error("not a binary xml (magic 0x{0:08x})")]
    BadMagic(u32),
    #[error("invalid string pool: {0}")]
    StringPool(String),
    #[error("truncated data: {0}")]
    Eof(#[from] crate::reader::Eof),
}

/// 判断字节流是否为二进制 XML（魔数 0x00080003）。
pub fn is_binary_xml(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[..4] == [0x03, 0x00, 0x08, 0x00]
}

/// 解析为元素树；返回的根是一个无名文档节点，其 `children[0]` 通常是 `<manifest>` / `<vector>`。
///
/// 头部（魔数、字符串池）损坏时返回错误；之后的 chunk 损坏则停止并返回已解析部分，
/// 因为混淆 APK 常在尾部塞垃圾 chunk，而前面的 `<manifest>` 信息仍然有用。
pub fn parse_document(bytes: &[u8]) -> Result<XmlElement, AxmlError> {
    let mut r = ByteReader::new(bytes);
    let strings = read_header(&mut r)?;
    let mut res_map: Vec<u32> = Vec::new();
    // 栈底是文档根；元素在 END_ELEMENT（或解析结束）时挂到父节点
    let mut stack: Vec<XmlElement> = vec![XmlElement::default()];

    while r.remain() >= 8 {
        let Some(chunk) = next_chunk(&mut r)? else {
            break;
        };
        match chunk.tag {
            RES_XML_RESOURCE_MAP_TYPE => {
                res_map = parse_resource_id_map(&mut r, &chunk);
            }
            START_ELEMENT_TAG => match parse_start_element(&mut r, &strings, &res_map, chunk.end) {
                Ok(el) => stack.push(el),
                Err(e) => {
                    log::warn!("axml: start element at {} truncated: {e}", chunk.start);
                    break;
                }
            },
            END_ELEMENT_TAG => pop_into_parent(&mut stack),
            // 命名空间与 CDATA 在树模式下不需要
            _ => {}
        }
        r.seek(chunk.end);
    }
    while stack.len() > 1 {
        pop_into_parent(&mut stack);
    }
    Ok(stack.pop().unwrap_or_default())
}

/// 解码为可读 XML 文本（aapt 风格，不带命名空间前缀），供文本信息 / 调试使用。
pub fn decompress_to_text(bytes: &[u8]) -> Result<String, AxmlError> {
    let mut r = ByteReader::new(bytes);
    let strings = read_header(&mut r)?;
    let mut res_map: Vec<u32> = Vec::new();
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let mut ident = 0usize;

    while r.remain() >= 8 {
        let Some(chunk) = next_chunk(&mut r)? else {
            break;
        };
        let res = match chunk.tag {
            RES_XML_RESOURCE_MAP_TYPE => {
                res_map = parse_resource_id_map(&mut r, &chunk);
                Ok(())
            }
            START_ELEMENT_TAG => {
                let res = write_start_tag(&mut out, &mut r, &strings, &res_map, ident);
                ident += 1;
                res
            }
            END_ELEMENT_TAG => {
                ident = ident.saturating_sub(1);
                write_end_tag(&mut out, &mut r, &strings, ident)
            }
            CDATA_TAG => write_cdata(&mut out, &mut r, &strings, ident),
            _ => Ok(()),
        };
        if let Err(e) = res {
            // 调试输出尽量给出已解码的部分，而不是整体失败
            log::warn!("axml: text decode stopped at chunk {}: {e}", chunk.start);
            break;
        }
        r.seek(chunk.end);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// 常量

const PACKED_XML_IDENTIFIER: u32 = 0x0008_0003;
const START_ELEMENT_TAG: u16 = 0x0102;
const END_ELEMENT_TAG: u16 = 0x0103;
const CDATA_TAG: u16 = 0x0104;
const RES_XML_RESOURCE_MAP_TYPE: u16 = 0x0180;
const RES_XML_STRING_TABLE: u16 = 0x0001;

const RES_TYPE_NULL: u8 = 0x00;
const RES_TYPE_REFERENCE: u8 = 0x01;
const RES_TYPE_ATTRIBUTE: u8 = 0x02;
const RES_TYPE_STRING: u8 = 0x03;
const RES_TYPE_FLOAT: u8 = 0x04;
const RES_TYPE_DIMENSION: u8 = 0x05;
const RES_TYPE_FRACTION: u8 = 0x06;
const RES_TYPE_DYNAMIC_REFERENCE: u8 = 0x07;
const RES_TYPE_INT_DEC: u8 = 0x10;
const RES_TYPE_INT_HEX: u8 = 0x11;
const RES_TYPE_INT_BOOLEAN: u8 = 0x12;
const RES_TYPE_INT_COLOR_ARGB8: u8 = 0x1c;
const RES_TYPE_INT_COLOR_RGB8: u8 = 0x1d;
const RES_TYPE_INT_COLOR_ARGB4: u8 = 0x1e;
const RES_TYPE_INT_COLOR_RGB4: u8 = 0x1f;

/// 混淆器把 typed value 的类型字节改成 0xFF 来破坏解析，多种容错路径以它为信号。
const OBFUSCATED_TYPE: u8 = 0xFF;

const RADIX_MULTS: [f64; 4] = [
    1.0,
    1.0 / (1 << 7) as f64,
    1.0 / (1 << 15) as f64,
    1.0 / (1 << 23) as f64,
];

const IDENT_SIZE: usize = 2;
const ATTR_IDENT_SIZE: usize = 4;
const UTF8_FLAG: u32 = 0x100;

/// 标准属性结构 ResXMLTree_attribute 的大小。
const STD_ATTR_SIZE: usize = 20;

/// Android 框架属性资源 ID → 属性名。混淆 APK 常篡改字符串池里的属性名，
/// 但 Resource ID Map 必须正确（否则系统也无法解析），因此优先用它。
///
/// 属性名集合与 Dart 版 `_kFrameworkAttrNames` 相同，但 ID 以 SDK android.jar 的
/// `public.xml` 为准重新核对过：Dart 版约一半条目错位（如把 glEsVersion 0x01010281
/// 记成 smallScreens、把 exported 0x01010010 记成 resource），会把正确的属性名覆盖成错的。
const FRAMEWORK_ATTR_NAMES: &[(u32, &str)] = &[
    (0x01010000, "theme"),
    (0x01010001, "label"),
    (0x01010002, "icon"),
    (0x01010003, "name"),
    (0x01010005, "allowClearUserData"),
    (0x01010006, "permission"),
    (0x0101000c, "hasCode"),
    (0x0101000d, "persistent"),
    (0x0101000e, "enabled"),
    (0x0101000f, "debuggable"),
    (0x01010010, "exported"),
    (0x01010011, "process"),
    (0x01010012, "taskAffinity"),
    (0x01010013, "multiprocess"),
    (0x01010018, "authorities"),
    (0x0101001d, "launchMode"),
    (0x0101001e, "screenOrientation"),
    (0x0101001f, "configChanges"),
    (0x01010024, "value"),
    (0x01010025, "resource"),
    (0x01010026, "mimeType"),
    (0x01010027, "scheme"),
    (0x01010028, "host"),
    (0x01010029, "port"),
    (0x0101002a, "path"),
    (0x0101002b, "pathPrefix"),
    (0x0101002c, "pathPattern"),
    (0x01010204, "allowTaskReparenting"),
    (0x0101020c, "minSdkVersion"),
    (0x0101021b, "versionCode"),
    (0x0101021c, "versionName"),
    (0x0101026c, "anyDensity"),
    (0x01010270, "targetSdkVersion"),
    (0x01010271, "maxSdkVersion"),
    (0x01010284, "smallScreens"),
    (0x01010285, "normalScreens"),
    (0x01010286, "largeScreens"),
    (0x0101028d, "resizeable"),
    (0x0101028e, "required"),
    (0x010102b7, "installLocation"),
    (0x010102bf, "xlargeScreens"),
    (0x010102ca, "screenSize"),
    (0x010102d3, "hardwareAccelerated"),
    (0x01010364, "requiresSmallestWidthDp"),
    (0x01010365, "compatibleWidthLimitDp"),
    (0x01010366, "largestWidthLimitDp"),
    (0x010103a7, "parentActivityName"),
    (0x010103af, "supportsRtl"),
    (0x010103f2, "banner"),
    (0x010103f4, "isGame"),
    (0x010104ea, "extractNativeLibs"),
    (0x010104ec, "usesCleartextTraffic"),
    (0x01010527, "networkSecurityConfig"),
    (0x0101052c, "roundIcon"),
    (0x01010545, "appCategory"),
    (0x0101054c, "targetSandboxVersion"),
    (0x0101056b, "classLoader"),
    (0x01010572, "compileSdkVersion"),
    (0x01010573, "compileSdkVersionCodename"),
    (0x0101057a, "appComponentFactory"),
    (0x01010603, "requestLegacyExternalStorage"),
    (0x0101065b, "localeConfig"),
    (0x0101066c, "enableOnBackInvokedCallback"),
];

fn framework_attr_name(res_id: u32) -> Option<&'static str> {
    FRAMEWORK_ATTR_NAMES
        .iter()
        .find(|(id, _)| *id == res_id)
        .map(|(_, n)| *n)
}

// ---------------------------------------------------------------------------
// chunk 遍历

struct Chunk {
    tag: u16,
    header_size: usize,
    start: usize,
    size: usize,
    end: usize,
}

/// 读魔数 + 跳过文件大小 + 解析紧随其后的字符串池。
fn read_header(r: &mut ByteReader) -> Result<Vec<String>, AxmlError> {
    let magic = r.u32()?;
    if magic != PACKED_XML_IDENTIFIER {
        return Err(AxmlError::BadMagic(magic));
    }
    r.skip(4)?;
    parse_strings(r)
}

/// 读下一个 chunk 头；尺寸非法（<8 或越界）返回 `None` 表示停止遍历。
fn next_chunk(r: &mut ByteReader) -> Result<Option<Chunk>, AxmlError> {
    let start = r.pos();
    let tag = r.u16()?;
    let header_size = r.u16()? as usize;
    let size = r.u32()? as usize;
    if size < 8 {
        log::debug!("axml: invalid chunk size {size} at {start}");
        return Ok(None);
    }
    match start.checked_add(size) {
        Some(end) if end <= r.len() => Ok(Some(Chunk {
            tag,
            header_size,
            start,
            size,
            end,
        })),
        _ => {
            log::debug!(
                "axml: chunk out of bounds: start={start} size={size} total={}",
                r.len()
            );
            Ok(None)
        }
    }
}

fn pop_into_parent(stack: &mut Vec<XmlElement>) {
    if stack.len() > 1 {
        if let Some(el) = stack.pop() {
            if let Some(parent) = stack.last_mut() {
                parent.children.push(el);
            }
        }
    }
}

/// Resource ID Map chunk (0x0180)：下标对应字符串池索引，值为框架资源 ID。
fn parse_resource_id_map(r: &mut ByteReader, chunk: &Chunk) -> Vec<u32> {
    let count = chunk.size.saturating_sub(chunk.header_size) / 4;
    r.seek(chunk.start + chunk.header_size);
    let mut ids = Vec::with_capacity(count.min(r.remain() / 4));
    for _ in 0..count {
        match r.u32() {
            Ok(v) => ids.push(v),
            Err(_) => break,
        }
    }
    ids
}

// ---------------------------------------------------------------------------
// 字符串池

fn parse_strings(r: &mut ByteReader) -> Result<Vec<String>, AxmlError> {
    let start = r.pos();
    let marker = r.u16()?;
    if marker != RES_XML_STRING_TABLE {
        return Err(AxmlError::StringPool(format!(
            "invalid string table identifier: expecting 0x{RES_XML_STRING_TABLE:x}, found 0x{marker:x}"
        )));
    }
    let header_size = r.u16()? as usize;
    let chunk_size = r.u32()? as usize;
    let num_strings = r.u32()? as usize;
    let num_styles = r.u32()? as usize;
    let flags = r.u32()?;
    let string_start = r.u32()? as usize;
    let _styles_start = r.u32()?;
    let is_utf8 = flags & UTF8_FLAG != 0;

    let data_size = chunk_size.checked_sub(header_size).ok_or_else(|| {
        AxmlError::StringPool(format!(
            "header size {header_size} > chunk size {chunk_size}"
        ))
    })?;
    r.seek(start + header_size);
    let data = r.bytes(data_size)?;
    decode_string_pool(
        data,
        header_size,
        num_strings,
        num_styles,
        string_start,
        is_utf8,
    )
}

/// `data` 是去掉池头之后的部分；偏移表与字符串都相对它定位。
fn decode_string_pool(
    data: &[u8],
    header_size: usize,
    num_strings: usize,
    num_styles: usize,
    string_start: usize,
    is_utf8: bool,
) -> Result<Vec<String>, AxmlError> {
    if num_strings == 0 {
        return Ok(Vec::new());
    }
    // 先校验偏移表容量再分配，防止损坏的计数导致巨量内存分配
    let table_bytes = num_strings
        .checked_add(num_styles)
        .and_then(|n| n.checked_mul(4))
        .filter(|&n| n <= data.len())
        .ok_or_else(|| {
            AxmlError::StringPool(format!(
                "offset table overflow: {num_strings}+{num_styles} entries"
            ))
        })?;
    let _ = table_bytes;

    let strings_start = string_start
        .checked_sub(header_size)
        .filter(|&s| s < data.len())
        .ok_or_else(|| {
            AxmlError::StringPool(format!(
                "invalid string pool start: stringStart={string_start} headerSize={header_size} dataSize={}",
                data.len()
            ))
        })?;

    let mut offsets = ByteReader::new(data);
    let mut strings = vec![String::new(); num_strings];
    for slot in strings.iter_mut() {
        let off = offsets.i32()? as i64;
        let pos = strings_start as i64 + off;
        if pos < 0 || pos as usize >= data.len() {
            continue;
        }
        let mut sr = ByteReader::new(data);
        sr.seek(pos as usize);
        // 单个字符串损坏只留空串，不影响其它字符串
        if let Some(s) = read_pool_string(&mut sr, is_utf8) {
            *slot = s;
        }
    }
    Ok(strings)
}

/// 读取字符串池中的一个字符串（UTF-8 或 UTF-16），越界返回 `None`。
/// arsc 字符串池也复用此函数。
pub(crate) fn read_pool_string(r: &mut ByteReader, is_utf8: bool) -> Option<String> {
    if is_utf8 {
        read_length8(r)?; // UTF-16 字符数，解码用不到
        let len = read_length8(r)?;
        if len > r.remain() {
            return None;
        }
        Some(String::from_utf8_lossy(r.bytes(len).ok()?).into_owned())
    } else {
        let len = read_length16(r)?;
        let byte_len = len.checked_mul(2)?;
        if byte_len > r.remain() {
            return None;
        }
        Some(decode_utf16le(r.bytes(byte_len).ok()?))
    }
}

pub(crate) fn decode_utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    String::from_utf16_lossy(&units)
}

/// UTF-8 池长度编码：最高位置位时为两字节 15 位长度。
fn read_length8(r: &mut ByteReader) -> Option<usize> {
    let first = r.u8().ok()? as usize;
    if first & 0x80 == 0 {
        return Some(first);
    }
    let second = r.u8().ok()? as usize;
    Some(((first & 0x7f) << 8) | second)
}

/// UTF-16 池长度编码：最高位置位时为两个 u16 组成的 31 位长度。
fn read_length16(r: &mut ByteReader) -> Option<usize> {
    let first = r.u16().ok()? as usize;
    if first & 0x8000 == 0 {
        return Some(first);
    }
    let second = r.u16().ok()? as usize;
    Some(((first & 0x7fff) << 16) | second)
}

fn safe_get(strings: &[String], index: i32) -> &str {
    usize::try_from(index)
        .ok()
        .and_then(|i| strings.get(i))
        .map(|s| s.as_str())
        .unwrap_or("")
}

/// 名称解析：优先 Resource ID Map 对应的框架属性名，回退到字符串池。
fn resolve_name(strings: &[String], res_map: &[u32], index: i32) -> String {
    if let Some(&res_id) = usize::try_from(index).ok().and_then(|i| res_map.get(i)) {
        if let Some(name) = framework_attr_name(res_id) {
            return name.to_string();
        }
    }
    safe_get(strings, index).to_string()
}

/// 索引落在 Resource ID Map 之外、且不是命名空间 URI 的非空字符串。
/// Resource ID Map 范围内的字符串是属性名，不可能是被藏匿的值。
fn hidden_value_string(strings: &[String], res_map_len: usize, index: i32) -> Option<&str> {
    let i = usize::try_from(index).ok()?;
    if i < res_map_len {
        return None;
    }
    let s = strings.get(i)?;
    if s.is_empty() || s.starts_with("http://") || s.starts_with("https://") {
        return None;
    }
    Some(s)
}

// ---------------------------------------------------------------------------
// 属性

/// 一条原始属性（ResXMLTree_attribute）。
#[derive(Debug, Clone, Copy)]
struct RawAttr {
    ns: i32,
    name: i32,
    raw_value: i32,
    data_type: u8,
    data: i32,
}

/// 读取元素头里的属性扩展与全部原始属性。
/// 混淆 APK 可能把 attrSize 改大（如 24），必须按实际大小跳过额外填充字节。
fn read_raw_attrs(r: &mut ByteReader) -> Result<Vec<RawAttr>, AxmlError> {
    let marker = r.u32()?;
    // marker 低 16 位 attrStart、高 16 位 attrSize；标准值 0x00140014
    let attr_size = ((marker >> 16) & 0xFFFF) as usize;
    let extra = attr_size.saturating_sub(STD_ATTR_SIZE);
    let count = r.u16()? as usize;
    r.skip(6)?; // idIndex / classIndex / styleIndex

    let mut attrs = Vec::with_capacity(count.min(r.remain() / STD_ATTR_SIZE));
    for _ in 0..count {
        let ns = r.i32()?;
        let name = r.i32()?;
        let raw_value = r.i32()?;
        let _size = r.u16()?;
        let _res0 = r.u8()?;
        let data_type = r.u8()?;
        let data = r.i32()?;
        attrs.push(RawAttr {
            ns,
            name,
            raw_value,
            data_type,
            data,
        });
        if extra > 0 {
            r.skip(extra)?;
        }
    }
    Ok(attrs)
}

/// 移位属性：类型字节为 0xFF，而 data 字段里装的是一个合法的 Res_value 头
/// （size=8/12 + 已知类型）。这是某些混淆器把属性整体错位 4 字节造成的。
fn is_shifted_attribute(data_type: u8, data: i32) -> bool {
    if data_type != OBFUSCATED_TYPE {
        return false;
    }
    let data = data as u32;
    let shifted_size = data & 0xFFFF;
    let shifted_type = ((data >> 24) & 0xFF) as u8;
    if shifted_size != 0x0008 && shifted_size != 0x000c {
        return false;
    }
    matches!(
        shifted_type,
        RES_TYPE_REFERENCE
            | RES_TYPE_STRING
            | RES_TYPE_INT_DEC
            | RES_TYPE_INT_HEX
            | RES_TYPE_INT_BOOLEAN
            | RES_TYPE_INT_COLOR_ARGB8
            | RES_TYPE_INT_COLOR_RGB8
    )
}

fn is_placeholder(v: &str) -> bool {
    v == "<empty>" || v == "<undefined>"
}

fn parse_start_element(
    r: &mut ByteReader,
    strings: &[String],
    res_map: &[u32],
    chunk_end: usize,
) -> Result<XmlElement, AxmlError> {
    let mut el = XmlElement::default();
    r.skip(8)?; // 行号 + 注释
    let _ns = r.i32()?;
    let name_idx = r.i32()?;
    el.name = resolve_name(strings, res_map, name_idx);
    if el.name.is_empty() {
        el.name = "unknown".into();
    }

    let raw = read_raw_attrs(r)?;

    // 移位属性在最后一个时，其真实值落在 chunk 末尾的填充区
    let overflow = if r.pos() + 4 <= chunk_end {
        r.i32().ok()
    } else {
        None
    };
    let map_len = res_map.len();

    for (i, a) in raw.iter().enumerate() {
        if is_shifted_attribute(a.data_type, a.data) {
            let shifted_type = (((a.data as u32) >> 24) & 0xFF) as u8;
            // 真实属性名在 rawValue 位置，真实值在下一个属性的 ns 字段（或溢出区）
            let real_value = raw.get(i + 1).map(|n| n.ns).or(overflow);
            if let Some(value) = real_value {
                let mut name = resolve_name(strings, res_map, a.raw_value);
                if name.is_empty() {
                    name = format!("attr_{}", a.raw_value);
                }
                let v = format_value(shifted_type, value, -1, strings, map_len);
                if !is_placeholder(&v) {
                    // 移位恢复的值优先级高，直接覆盖
                    el.attributes.insert(name, v);
                }
            }
            continue;
        }

        let mut name = resolve_name(strings, res_map, a.name);
        if name.is_empty() {
            name = format!("attr_{}", a.name);
        }
        let v = format_value(a.data_type, a.data, a.raw_value, strings, map_len);
        let obfuscated = a.data_type == RES_TYPE_NULL || a.data_type == OBFUSCATED_TYPE;

        // 混淆 APK 可能有重复属性名：不用空值覆盖已有的有效值
        let keep_existing = is_placeholder(&v)
            && el
                .attributes
                .get(&name)
                .is_some_and(|old| !old.is_empty() && !is_placeholder(old));
        if !keep_existing {
            el.attributes.insert(name, v);
        }
        if obfuscated {
            rescue_ns_value(&mut el, strings, map_len, a.ns);
        }
    }
    Ok(el)
}

/// 从混淆属性的 ns 字段中恢复可能被藏匿的值字符串（如 versionName）。
fn rescue_ns_value(el: &mut XmlElement, strings: &[String], map_len: usize, ns: i32) {
    if let Some(s) = hidden_value_string(strings, map_len, ns) {
        el.extra_strings.push(s.to_string());
    }
}

/// 把 typed value 转成字符串，格式与 Dart 版逐字一致。
/// `raw_index` 是 rawValue（字符串池索引，-1 表示无）。
fn format_value(
    data_type: u8,
    data: i32,
    raw_index: i32,
    strings: &[String],
    res_map_len: usize,
) -> String {
    let u = data as u32;
    match data_type {
        RES_TYPE_NULL => {
            // 混淆 APK 可能把 typed value 置空而 rawValue 仍有效
            if let Some(s) = hidden_value_string(strings, res_map_len, raw_index) {
                return s.to_string();
            }
            if data == 0 { "<undefined>" } else { "<empty>" }.to_string()
        }
        RES_TYPE_REFERENCE => format!("@res/0x{}", dart_hex(data)),
        RES_TYPE_ATTRIBUTE => format!("@attr/0x{}", dart_hex(data)),
        RES_TYPE_STRING => safe_get(strings, raw_index).to_string(),
        RES_TYPE_FLOAT => dart_double(f32::from_bits(u) as f64),
        RES_TYPE_DIMENSION => format!("{}{}", dart_double(complex_value(u)), dimension_unit(u)),
        RES_TYPE_FRACTION => format!("{}{}", dart_double(complex_value(u)), fraction_unit(u)),
        RES_TYPE_DYNAMIC_REFERENCE => format!("@dyn/0x{}", dart_hex(data)),
        RES_TYPE_INT_DEC => data.to_string(),
        RES_TYPE_INT_HEX => format!("0x{u:x}"),
        // Android 语义是非零即真（aapt 写入 0xffffffff）。Dart 版因有符号比较总得到 false，这里按规范修正。
        RES_TYPE_INT_BOOLEAN => if u != 0 { "true" } else { "false" }.to_string(),
        RES_TYPE_INT_COLOR_ARGB8 | RES_TYPE_INT_COLOR_ARGB4 => format!("#{u:08x}"),
        RES_TYPE_INT_COLOR_RGB8 | RES_TYPE_INT_COLOR_RGB4 => format!("#ff{:06x}", u & 0x00ff_ffff),
        _ => format!("0x{u:x}"),
    }
}

/// Dart `int.toRadixString(16)` 对有符号 32 位值的行为（负数带 `-`）。
fn dart_hex(v: i32) -> String {
    if v < 0 {
        format!("-{:x}", -(v as i64))
    } else {
        format!("{v:x}")
    }
}

/// Dart `double.toString()`：整数值带 `.0`；绝对值 <1e-6 或 >=1e21 用指数形式（`1e-7`、`1e+21`）。
pub(crate) fn dart_double(v: f64) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    let a = v.abs();
    if a != 0.0 && !(1e-6..1e21).contains(&a) {
        let s = format!("{v:e}");
        return match s.split_once('e') {
            Some((m, e)) if !e.starts_with('-') => format!("{m}e+{e}"),
            _ => s,
        };
    }
    let s = format!("{v}");
    if s.contains('.') {
        s
    } else {
        format!("{s}.0")
    }
}

/// complex 值：尾数 × 基数倍率。与 Dart 版一致按无符号尾数计算。
fn complex_value(data: u32) -> f64 {
    let mantissa = (data & 0xffff_ff00) as f64;
    mantissa * RADIX_MULTS[((data >> 4) & 0x3) as usize]
}

fn dimension_unit(data: u32) -> &'static str {
    match data & 0xf {
        0 => "px",
        1 => "dp",
        2 => "sp",
        3 => "pt",
        4 => "in",
        5 => "mm",
        _ => " (unknown unit)",
    }
}

/// 注意：Dart 版输出的就是字面 `%%`（源自 Java printf 写法的遗留），保持一致。
fn fraction_unit(data: u32) -> &'static str {
    match data & 0xf {
        0 => "%%",
        1 => "%%p",
        _ => "(unknown unit)",
    }
}

// ---------------------------------------------------------------------------
// 文本输出

fn indent(out: &mut String, n: usize) {
    out.extend(std::iter::repeat_n(' ', n));
}

fn write_start_tag(
    out: &mut String,
    r: &mut ByteReader,
    strings: &[String],
    res_map: &[u32],
    ident: usize,
) -> Result<(), AxmlError> {
    indent(out, ident * IDENT_SIZE);
    out.push('<');
    r.skip(8)?;
    let _ns = r.i32()?;
    let name = resolve_name(strings, res_map, r.i32()?);
    out.push_str(if name.is_empty() { "unknown" } else { &name });

    // 文本模式与 Dart 一致不做移位恢复，只逐条格式化；但同样跳过额外填充字节，避免错位
    for a in read_raw_attrs(r)? {
        out.push('\n');
        indent(out, ident * IDENT_SIZE + ATTR_IDENT_SIZE);
        let mut attr_name = resolve_name(strings, res_map, a.name);
        if attr_name.is_empty() {
            attr_name = format!("attr_{}", a.name);
        }
        let v = format_value(a.data_type, a.data, a.raw_value, strings, res_map.len());
        let _ = write!(out, "{attr_name}=\"{v}\"");
    }
    out.push_str(">\n");
    Ok(())
}

fn write_end_tag(
    out: &mut String,
    r: &mut ByteReader,
    strings: &[String],
    ident: usize,
) -> Result<(), AxmlError> {
    r.skip(8)?;
    let _ns = r.i32()?;
    let name = r.i32()?;
    indent(out, ident * IDENT_SIZE);
    let _ = writeln!(out, "</{}>", safe_get(strings, name));
    Ok(())
}

fn write_cdata(
    out: &mut String,
    r: &mut ByteReader,
    strings: &[String],
    ident: usize,
) -> Result<(), AxmlError> {
    r.skip(8)?;
    let idx = r.i32()?;
    r.skip(8)?;
    indent(out, ident * IDENT_SIZE);
    out.push_str("<![CDATA[\n");
    indent(out, ident * IDENT_SIZE + 1);
    out.push_str(safe_get(strings, idx));
    indent(out, ident * IDENT_SIZE);
    out.push_str("]]>\n");
    Ok(())
}

// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 按 AAPT 格式拼出最小二进制 XML 的构造器。
    struct AxmlBuilder {
        strings: Vec<String>,
        res_ids: Vec<u32>,
        body: Vec<u8>,
    }

    fn push_u16(v: &mut Vec<u8>, x: u16) {
        v.extend_from_slice(&x.to_le_bytes());
    }
    fn push_u32(v: &mut Vec<u8>, x: u32) {
        v.extend_from_slice(&x.to_le_bytes());
    }

    /// (ns, name, rawValue, type, data)
    type Attr = (i32, i32, i32, u8, u32);

    impl AxmlBuilder {
        fn new(strings: &[&str], res_ids: &[u32]) -> Self {
            Self {
                strings: strings.iter().map(|s| s.to_string()).collect(),
                res_ids: res_ids.to_vec(),
                body: Vec::new(),
            }
        }
        fn idx(&self, s: &str) -> i32 {
            self.strings.iter().position(|x| x == s).unwrap() as i32
        }
        fn start(&mut self, name: &str, attrs: &[Attr]) {
            self.start_raw(name, attrs, 20, &[]);
        }
        fn start_raw(&mut self, name: &str, attrs: &[Attr], attr_size: u16, trailer: &[u8]) {
            let name = self.idx(name);
            let extra = attr_size as usize - 20;
            let size = 16 + 20 + attrs.len() * attr_size as usize + trailer.len();
            let b = &mut self.body;
            push_u16(b, START_ELEMENT_TAG);
            push_u16(b, 16);
            push_u32(b, size as u32);
            push_u32(b, 1); // line
            push_u32(b, 0xffff_ffff); // comment
            push_u32(b, 0xffff_ffff); // ns
            push_u32(b, name as u32);
            push_u16(b, 20); // attrStart
            push_u16(b, attr_size);
            push_u16(b, attrs.len() as u16);
            push_u16(b, 0);
            push_u16(b, 0);
            push_u16(b, 0);
            for &(ns, an, raw, ty, data) in attrs {
                push_u32(b, ns as u32);
                push_u32(b, an as u32);
                push_u32(b, raw as u32);
                push_u16(b, 8);
                b.push(0);
                b.push(ty);
                push_u32(b, data);
                b.extend(std::iter::repeat_n(0u8, extra));
            }
            b.extend_from_slice(trailer);
        }
        fn end(&mut self, name: &str) {
            let name = self.idx(name);
            let b = &mut self.body;
            push_u16(b, END_ELEMENT_TAG);
            push_u16(b, 16);
            push_u32(b, 24);
            push_u32(b, 1);
            push_u32(b, 0xffff_ffff);
            push_u32(b, 0xffff_ffff);
            push_u32(b, name as u32);
        }
        fn build(&self) -> Vec<u8> {
            let pool = string_pool_utf8(&self.strings);
            let mut map = Vec::new();
            push_u16(&mut map, RES_XML_RESOURCE_MAP_TYPE);
            push_u16(&mut map, 8);
            push_u32(&mut map, 8 + 4 * self.res_ids.len() as u32);
            for id in &self.res_ids {
                push_u32(&mut map, *id);
            }
            let mut out = Vec::new();
            push_u32(&mut out, PACKED_XML_IDENTIFIER);
            push_u32(
                &mut out,
                (8 + pool.len() + map.len() + self.body.len()) as u32,
            );
            out.extend(pool);
            out.extend(map);
            out.extend_from_slice(&self.body);
            out
        }
    }

    pub(crate) fn string_pool_utf8(strings: &[String]) -> Vec<u8> {
        let mut data = Vec::new();
        let mut offsets = Vec::new();
        for s in strings {
            offsets.push(data.len() as u32);
            let n = s.chars().count();
            let b = s.len();
            for len in [n, b] {
                if len > 0x7f {
                    data.push(0x80 | (len >> 8) as u8);
                    data.push(len as u8);
                } else {
                    data.push(len as u8);
                }
            }
            data.extend_from_slice(s.as_bytes());
            data.push(0);
        }
        while data.len() % 4 != 0 {
            data.push(0);
        }
        build_pool(&offsets, &data, UTF8_FLAG)
    }

    pub(crate) fn string_pool_utf16(strings: &[String]) -> Vec<u8> {
        let mut data = Vec::new();
        let mut offsets = Vec::new();
        for s in strings {
            offsets.push(data.len() as u32);
            let units: Vec<u16> = s.encode_utf16().collect();
            let n = units.len();
            if n > 0x7fff {
                push_u16(&mut data, 0x8000 | (n >> 16) as u16);
                push_u16(&mut data, n as u16);
            } else {
                push_u16(&mut data, n as u16);
            }
            for u in units {
                push_u16(&mut data, u);
            }
            push_u16(&mut data, 0);
        }
        while data.len() % 4 != 0 {
            data.push(0);
        }
        build_pool(&offsets, &data, 0)
    }

    fn build_pool(offsets: &[u32], data: &[u8], flags: u32) -> Vec<u8> {
        let header = 28u32;
        let strings_start = header + 4 * offsets.len() as u32;
        let mut v = Vec::new();
        push_u16(&mut v, RES_XML_STRING_TABLE);
        push_u16(&mut v, header as u16);
        push_u32(&mut v, strings_start + data.len() as u32);
        push_u32(&mut v, offsets.len() as u32);
        push_u32(&mut v, 0);
        push_u32(&mut v, flags);
        push_u32(&mut v, strings_start);
        push_u32(&mut v, 0);
        for o in offsets {
            push_u32(&mut v, *o);
        }
        v.extend_from_slice(data);
        v
    }

    fn parse_pool(bytes: &[u8]) -> Vec<String> {
        parse_strings(&mut ByteReader::new(bytes)).unwrap()
    }

    #[test]
    fn string_pool_utf8_with_long_length() {
        let long = "长".repeat(100); // 300 字节，需要两字节长度编码
        let src = vec![
            "a".to_string(),
            long.clone(),
            String::new(),
            "héllo".to_string(),
        ];
        assert_eq!(parse_pool(&string_pool_utf8(&src)), src);
    }

    #[test]
    fn string_pool_utf16_with_long_length() {
        let long = "x".repeat(40000); // > 0x7fff 个码元，需要两个 u16 长度编码
        let src = vec!["中文".to_string(), long, "😀".to_string()];
        assert_eq!(parse_pool(&string_pool_utf16(&src)), src);
    }

    #[test]
    fn string_pool_bad_offset_yields_empty_string() {
        let mut pool = string_pool_utf8(&["ok".to_string(), "bad".to_string()]);
        // 第二个偏移改成越界
        pool[32..36].copy_from_slice(&0x7fff_0000u32.to_le_bytes());
        assert_eq!(parse_pool(&pool), vec!["ok".to_string(), String::new()]);
    }

    #[test]
    fn value_formatting_matches_dart() {
        let s: Vec<String> = vec!["str".into()];
        assert_eq!(
            format_value(RES_TYPE_REFERENCE, 0x7f010000, -1, &s, 0),
            "@res/0x7f010000"
        );
        assert_eq!(
            format_value(RES_TYPE_ATTRIBUTE, 0x0101_0000, -1, &s, 0),
            "@attr/0x1010000"
        );
        assert_eq!(
            format_value(RES_TYPE_DYNAMIC_REFERENCE, 0x7e01_0002, -1, &s, 0),
            "@dyn/0x7e010002"
        );
        assert_eq!(format_value(RES_TYPE_STRING, 0, 0, &s, 0), "str");
        assert_eq!(format_value(RES_TYPE_INT_DEC, -3, -1, &s, 0), "-3");
        assert_eq!(format_value(RES_TYPE_INT_HEX, -1, -1, &s, 0), "0xffffffff");
        assert_eq!(format_value(RES_TYPE_INT_BOOLEAN, -1, -1, &s, 0), "true");
        assert_eq!(format_value(RES_TYPE_INT_BOOLEAN, 0, -1, &s, 0), "false");
        assert_eq!(
            format_value(RES_TYPE_INT_COLOR_ARGB8, 0x00ff_0000, -1, &s, 0),
            "#00ff0000"
        );
        assert_eq!(
            format_value(RES_TYPE_INT_COLOR_RGB8, 0x1234_5678, -1, &s, 0),
            "#ff345678"
        );
        assert_eq!(
            format_value(RES_TYPE_INT_COLOR_RGB4, 0xabc, -1, &s, 0),
            "#ff000abc"
        );
        assert_eq!(
            format_value(RES_TYPE_FLOAT, 1.5f32.to_bits() as i32, -1, &s, 0),
            "1.5"
        );
        assert_eq!(
            format_value(RES_TYPE_FLOAT, 2.0f32.to_bits() as i32, -1, &s, 0),
            "2.0"
        );
        // 16dp：尾数 16<<8，radix 0，unit 1
        assert_eq!(
            format_value(RES_TYPE_DIMENSION, (16 << 8) | 1, -1, &s, 0),
            "4096.0dp"
        );
        assert_eq!(
            format_value(RES_TYPE_FRACTION, 0x100 | 1, -1, &s, 0),
            "256.0%%p"
        );
        assert_eq!(format_value(0x42, 0x10, -1, &s, 0), "0x10");
        assert_eq!(format_value(RES_TYPE_NULL, 0, -1, &s, 0), "<undefined>");
        assert_eq!(format_value(RES_TYPE_NULL, 1, -1, &s, 0), "<empty>");
        // rawValue 回退：索引须在 Resource ID Map 之外
        assert_eq!(format_value(RES_TYPE_NULL, 0, 0, &s, 0), "str");
        assert_eq!(format_value(RES_TYPE_NULL, 0, 0, &s, 1), "<undefined>");
    }

    #[test]
    fn dart_double_formatting() {
        assert_eq!(dart_double(1.0), "1.0");
        assert_eq!(dart_double(-0.0), "-0.0");
        assert_eq!(dart_double(0.5), "0.5");
        assert_eq!(dart_double(0.000001), "0.000001");
        assert_eq!(dart_double(1e-7), "1e-7");
        assert_eq!(dart_double(1.5e21), "1.5e+21");
        assert_eq!(dart_double(0.1f32 as f64), "0.10000000149011612");
    }

    fn manifest_builder() -> AxmlBuilder {
        AxmlBuilder::new(
            &[
                "versionCode",
                "label",
                "package",
                "manifest",
                "application",
                "com.example.app",
                "http://schemas.android.com/apk/res/android",
            ],
            &[0x0101021b, 0x01010001],
        )
    }

    #[test]
    fn parses_synthetic_document_and_text() {
        let mut b = manifest_builder();
        let pkg = b.idx("package");
        let val = b.idx("com.example.app");
        b.start(
            "manifest",
            &[
                (-1, pkg, val, RES_TYPE_STRING, val as u32),
                (6, 0, -1, RES_TYPE_INT_DEC, 42),
            ],
        );
        b.start("application", &[(6, 1, -1, RES_TYPE_REFERENCE, 0x7f0a0001)]);
        b.end("application");
        b.end("manifest");
        let bytes = b.build();
        assert!(is_binary_xml(&bytes));

        let doc = parse_document(&bytes).unwrap();
        assert_eq!(doc.name, "");
        let m = &doc.children[0];
        assert_eq!(m.name, "manifest");
        assert_eq!(m.attr("package"), Some("com.example.app"));
        assert_eq!(m.attr("versionCode"), Some("42"));
        assert_eq!(
            m.child("application").unwrap().attr("label"),
            Some("@res/0x7f0a0001")
        );

        let text = decompress_to_text(&bytes).unwrap();
        let expect = concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
            "<manifest\n    package=\"com.example.app\"\n    versionCode=\"42\">\n",
            "  <application\n      label=\"@res/0x7f0a0001\">\n",
            "  </application>\n",
            "</manifest>\n",
        );
        assert_eq!(text, expect);
    }

    #[test]
    fn handles_padded_attr_size() {
        let mut b = manifest_builder();
        b.start_raw(
            "manifest",
            &[
                (6, 0, -1, RES_TYPE_INT_DEC, 7),
                (6, 1, -1, RES_TYPE_INT_HEX, 0xff),
            ],
            24,
            &[],
        );
        b.end("manifest");
        let doc = parse_document(&b.build()).unwrap();
        let m = &doc.children[0];
        assert_eq!(m.attr("versionCode"), Some("7"));
        assert_eq!(m.attr("label"), Some("0xff"));
    }

    #[test]
    fn recovers_shifted_attribute_and_keeps_existing_value() {
        let mut b = manifest_builder();
        let val = b.idx("com.example.app");
        // 第一个属性：type=0xFF，data 是 size=8,type=INT_DEC 的 Res_value 头，
        // 真实名在 rawValue(=0 → versionCode)，真实值在下一个属性的 ns 字段（=99）
        let shifted_header = 0x1000_0008u32;
        // 第二个属性是重复的 versionCode 空值，不应覆盖已恢复的 99；其 ns 指向池外字符串应被收入 extra_strings
        b.start(
            "manifest",
            &[
                (-1, 3, 0, OBFUSCATED_TYPE, shifted_header),
                (99, 0, -1, RES_TYPE_NULL, 0),
                (val, 1, -1, RES_TYPE_NULL, 0),
            ],
        );
        b.end("manifest");
        let doc = parse_document(&b.build()).unwrap();
        let m = &doc.children[0];
        assert_eq!(m.attr("versionCode"), Some("99"));
        assert_eq!(m.attr("label"), Some("<undefined>"));
        assert_eq!(m.extra_strings, vec!["com.example.app".to_string()]);
    }

    #[test]
    fn shifted_last_attribute_uses_overflow() {
        let mut b = manifest_builder();
        b.start_raw(
            "manifest",
            &[(-1, 3, 0, OBFUSCATED_TYPE, 0x1000_0008)],
            20,
            &123u32.to_le_bytes(),
        );
        b.end("manifest");
        let doc = parse_document(&b.build()).unwrap();
        assert_eq!(doc.children[0].attr("versionCode"), Some("123"));
    }

    #[test]
    fn rejects_bad_magic_and_tolerates_truncation() {
        assert!(matches!(
            parse_document(&[0u8; 16]),
            Err(AxmlError::BadMagic(0))
        ));
        let mut b = manifest_builder();
        b.start("manifest", &[]);
        b.start("application", &[]);
        let mut bytes = b.build();
        bytes.truncate(bytes.len() - 4); // 最后一个 chunk 越界 → 停止但保留前面的元素
        let doc = parse_document(&bytes).unwrap();
        assert_eq!(doc.children[0].name, "manifest");
    }
}
