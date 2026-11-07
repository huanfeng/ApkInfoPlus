//! AAPT2 的 Resources.proto / Configuration.proto 中用于查看 App Bundle 的字段。
//! 借用 ZIP 条目的字节切片，跳过未知字段，无需生成完整资源模型。

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    axml::XmlElement,
    model::{ApkMeta, ResourceInfo},
};

pub const MANIFEST: &str = "base/manifest/AndroidManifest.xml";

enum Value<'a> {
    Integer(u64),
    Bytes(&'a [u8]),
    Float(f32),
    Ignored,
}

struct Message<'a>(Vec<(u32, Value<'a>)>);

fn take<'a>(data: &mut &'a [u8], len: usize) -> Option<&'a [u8]> {
    let (value, rest) = data.split_at_checked(len)?;
    *data = rest;
    Some(value)
}

fn varint(data: &mut &[u8]) -> Option<u64> {
    let mut value = 0;
    for shift in (0..64).step_by(7) {
        let byte = take(data, 1)?[0];
        if shift == 63 && byte > 1 {
            return None;
        }
        value |= u64::from(byte & 127) << shift;
        if byte < 128 {
            return Some(value);
        }
    }
    None
}

impl<'a> Message<'a> {
    fn parse(mut data: &'a [u8]) -> Option<Self> {
        let mut fields = Vec::new();
        while !data.is_empty() {
            let key = u32::try_from(varint(&mut data)?).ok()?;
            let number = key >> 3;
            if number == 0 {
                return None;
            }
            let value = match key & 7 {
                0 => Value::Integer(varint(&mut data)?),
                1 => {
                    take(&mut data, 8)?;
                    Value::Ignored
                }
                2 => {
                    let len = usize::try_from(varint(&mut data)?).ok()?;
                    Value::Bytes(take(&mut data, len)?)
                }
                5 => Value::Float(f32::from_le_bytes(take(&mut data, 4)?.try_into().ok()?)),
                _ => return None,
            };
            fields.push((number, value));
        }
        Some(Self(fields))
    }

    fn value(&self, number: u32) -> Option<&Value<'a>> {
        self.0
            .iter()
            .rev()
            .find(|(n, _)| *n == number)
            .map(|(_, v)| v)
    }

    fn int(&self, number: u32) -> Option<u32> {
        match self.value(number)? {
            Value::Integer(value) => Some(*value as u32),
            _ => None,
        }
    }

    fn bytes(&self, number: u32) -> Option<&'a [u8]> {
        match self.value(number)? {
            Value::Bytes(value) => Some(value),
            _ => None,
        }
    }

    fn string(&self, number: u32) -> Option<&'a str> {
        std::str::from_utf8(self.bytes(number)?).ok()
    }

    fn message(&self, number: u32) -> Option<Self> {
        Self::parse(self.bytes(number)?)
    }

    fn repeated(&self, number: u32) -> impl Iterator<Item = &'a [u8]> + '_ {
        self.0.iter().filter_map(move |(n, value)| match value {
            Value::Bytes(bytes) if *n == number => Some(*bytes),
            _ => None,
        })
    }
}

fn item_reference(item: &Message<'_>) -> Option<u32> {
    item.message(1)?.int(2).filter(|id| *id != 0)
}

fn item_string<'a>(item: &Message<'a>) -> Option<&'a str> {
    [2, 3, 4]
        .into_iter()
        .find_map(|field| item.message(field)?.string(1))
}

fn primitive_color(primitive: &Message<'_>) -> Option<String> {
    let argb = primitive
        .int(9)
        .or_else(|| primitive.int(10).map(|v| v | 0xff000000))
        .or_else(|| primitive.int(11))
        .or_else(|| primitive.int(12).map(|v| v | 0xff000000))?;
    Some(format!("#{argb:08x}"))
}

/// Resources.proto `Reference.Type`：0 = REFERENCE（`@`），1 = ATTRIBUTE（`?`）。
const REFERENCE_TYPE_ATTRIBUTE: u32 = 1;

/// 将编译后的属性转成现有清单解析器和图标渲染器共用的字符串表示（与 AXML 路径的格式一致）。
fn attribute_value(item: &Message<'_>) -> Option<String> {
    if let Some(id) = item_reference(item) {
        let is_attr = item
            .message(1)
            .and_then(|r| r.int(1))
            .is_some_and(|t| t == REFERENCE_TYPE_ATTRIBUTE);
        return Some(if is_attr {
            format!("@attr/0x{id:x}")
        } else {
            format!("@res/0x{id:x}")
        });
    }
    if let Some(value) = item_string(item) {
        return Some(value.to_owned());
    }
    let primitive = item.message(7)?;
    if let Some(Value::Float(value)) = primitive.value(3) {
        return Some(crate::axml::dart_double(f64::from(*value)));
    }
    primitive
        .int(6)
        .map(|v| (v as i32).to_string())
        .or_else(|| primitive.int(7).map(|v| format!("0x{v:x}")))
        .or_else(|| primitive.int(8).map(|v| (v != 0).to_string()))
        .or_else(|| primitive.int(13).map(crate::axml::format_dimension))
        .or_else(|| primitive_color(&primitive))
}

pub fn xml_root(bytes: &[u8]) -> Option<XmlElement> {
    xml_element(Message::parse(bytes)?.message(1)?, 0)
}

fn xml_element(message: Message<'_>, depth: usize) -> Option<XmlElement> {
    // 文件内容不可信，限制递归深度，避免异常嵌套耗尽线程栈。
    if depth >= 128 {
        return None;
    }
    let mut element = XmlElement {
        name: message.string(3)?.to_owned(),
        ..Default::default()
    };
    for bytes in message.repeated(4) {
        let attr = Message::parse(bytes)?;
        let name = attr.string(2)?;
        // 优先使用编译值，其中的资源 ID 可直接用于查表；未编译的属性仍可读取原文。
        let value = attr
            .message(6)
            .as_ref()
            .and_then(attribute_value)
            .unwrap_or_else(|| attr.string(3).unwrap_or_default().to_owned());
        element.attributes.insert(name.to_owned(), value);
    }
    for bytes in message.repeated(5) {
        let child = Message::parse(bytes)?;
        if let Some(nested) = child.bytes(1) {
            element
                .children
                .push(xml_element(Message::parse(nested)?, depth + 1)?);
        } else if let Some(value) = child.string(2) {
            element
                .attributes
                .entry("#text".into())
                .or_default()
                .push_str(value);
        }
    }
    Some(element)
}

fn push_unique<T: PartialEq>(values: &mut Vec<T>, value: T) {
    if !values.contains(&value) {
        values.push(value);
    }
}

/// 资源路径相对于模块目录。图标渲染复用 APK 的引用解析，资源索引保留 ZIP 内的实际路径。
pub fn resources(
    bytes: &[u8],
    module: &str,
    meta: &mut ApkMeta,
    resources: &mut BTreeMap<u32, ResourceInfo>,
) -> Option<()> {
    let table = Message::parse(bytes)?;
    let label = (module == "base")
        .then(|| meta.label.as_deref().and_then(crate::apk::parse_res_ref))
        .flatten();
    let icon = (module == "base")
        .then(|| {
            meta.application_icon
                .as_deref()
                .and_then(crate::apk::parse_res_ref)
        })
        .flatten();
    let mut locales = BTreeSet::new();
    let mut densities = BTreeSet::new();
    // 与 APK 的 `ResourceTable::string_value` 一致：默认名取第一个无语言配置，
    // 都没有时退回任意配置的第一个；各语言也取第一个，night / vNN 等变体不覆盖。
    let mut default_label = None;
    let mut fallback_label = None;
    // 单条记录损坏只跳过该记录，已收集的 locale / density 照常合并。
    for package in table.repeated(2) {
        let Some(package) = Message::parse(package) else {
            continue;
        };
        let package_id = package.message(1).and_then(|m| m.int(1)).unwrap_or(0);
        for ty in package.repeated(3) {
            let Some(ty) = Message::parse(ty) else {
                continue;
            };
            let (Some(type_id), Some(type_name)) =
                (ty.message(1).and_then(|m| m.int(1)), ty.string(2))
            else {
                continue;
            };
            for entry in ty.repeated(3) {
                let Some(entry) = Message::parse(entry) else {
                    continue;
                };
                // ID 0 在 proto3 中可能省略（例如该类型下第一条资源）。
                let entry_id = entry.message(1).and_then(|m| m.int(1)).unwrap_or(0);
                let id = (package_id << 24) | (type_id << 16) | entry_id;
                // 模块按 base 优先的顺序处理。重复 ID 仍贡献 locale 和 density，
                // 但不能覆盖已索引的图标路径、颜色和引用。
                let index_resource = !resources.contains_key(&id);
                for value in entry.repeated(6) {
                    let Some(value) = Message::parse(value) else {
                        continue;
                    };
                    let config = value.message(1);
                    let density = config.as_ref().and_then(|c| c.int(18)).unwrap_or(0);
                    let locale = config
                        .as_ref()
                        .and_then(|c| c.string(3))
                        .unwrap_or_default();
                    if !locale.is_empty() {
                        locales.insert(locale.to_owned());
                    }
                    // 与 `ResourceTable::densities` 一致：排除 0xFFFE（anydpi）/ 0xFFFF（nodpi）哨兵值。
                    if density != 0 && density < 0xFFFE {
                        densities.insert(density);
                    }
                    if !index_resource {
                        continue;
                    }
                    let Some(item) = value.message(2).and_then(|v| v.message(4)) else {
                        continue;
                    };
                    if Some(id) == label {
                        if let Some(name) = item_string(&item) {
                            fallback_label.get_or_insert_with(|| name.to_owned());
                            if locale.is_empty() {
                                default_label.get_or_insert_with(|| name.to_owned());
                            } else {
                                meta.labels
                                    .entry(locale.to_owned())
                                    .or_insert_with(|| name.to_owned());
                            }
                        }
                    }
                    let path = item.message(5).and_then(|f| f.string(1));
                    let reference = item_reference(&item);
                    let color = item.message(7).as_ref().and_then(primitive_color);
                    if path.is_none() && reference.is_none() && color.is_none() {
                        continue;
                    }
                    let info = resources.entry(id).or_insert_with(|| ResourceInfo {
                        resource_id: id,
                        type_name: type_name.to_owned(),
                        key_name: entry.string(2).unwrap_or_default().to_owned(),
                        ..Default::default()
                    });
                    if let Some(path) = path {
                        let path = format!("{module}/{path}");
                        if Some(id) == icon {
                            meta.icon_paths.insert(density, path.clone());
                        }
                        push_unique(&mut info.file_paths, path);
                    }
                    if let Some(reference) = reference {
                        push_unique(&mut info.references, reference);
                    }
                    if let Some(color) = color {
                        push_unique(&mut info.colors, color);
                    }
                }
            }
        }
    }
    if let Some(name) = default_label.or(fallback_label) {
        meta.label = Some(name);
    }
    meta.locales.extend(locales);
    meta.locales.sort_unstable();
    meta.locales.dedup();
    meta.densities.extend(densities);
    meta.densities.sort_unstable();
    meta.densities.dedup();
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // 最小 protobuf 编码器：只覆盖解析器读取的 varint / length-delimited / fixed32 三种 wire type。
    fn varint_bytes(mut v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    fn int(n: u32, v: u64) -> Vec<u8> {
        let mut out = varint_bytes(u64::from(n << 3));
        out.extend(varint_bytes(v));
        out
    }

    fn bytes(n: u32, data: &[u8]) -> Vec<u8> {
        let mut out = varint_bytes(u64::from(n << 3 | 2));
        out.extend(varint_bytes(data.len() as u64));
        out.extend_from_slice(data);
        out
    }

    fn float(n: u32, v: f32) -> Vec<u8> {
        let mut out = varint_bytes(u64::from(n << 3 | 5));
        out.extend(v.to_le_bytes());
        out
    }

    fn msg(fields: &[Vec<u8>]) -> Vec<u8> {
        fields.concat()
    }

    /// ConfigValue：Configuration{3: locale, 18: density} + Value{4: item}。
    fn config_value(locale: &str, density: u64, item: Vec<u8>) -> Vec<u8> {
        let mut config = Vec::new();
        if !locale.is_empty() {
            config.extend(bytes(3, locale.as_bytes()));
        }
        if density != 0 {
            config.extend(int(18, density));
        }
        msg(&[bytes(1, &config), bytes(2, &bytes(4, &item))])
    }

    fn string_item(s: &str) -> Vec<u8> {
        bytes(2, &bytes(1, s.as_bytes()))
    }

    fn file_item(path: &str) -> Vec<u8> {
        bytes(5, &bytes(1, path.as_bytes()))
    }

    fn entry(id: u64, name: &str, values: &[Vec<u8>]) -> Vec<u8> {
        let mut out = msg(&[bytes(1, &int(1, id)), bytes(2, name.as_bytes())]);
        for v in values {
            out.extend(bytes(6, v));
        }
        out
    }

    fn ty(id: u64, name: &str, entries: &[Vec<u8>]) -> Vec<u8> {
        let mut out = msg(&[bytes(1, &int(1, id)), bytes(2, name.as_bytes())]);
        for e in entries {
            out.extend(bytes(3, e));
        }
        out
    }

    fn table(types: &[Vec<u8>]) -> Vec<u8> {
        let mut package = bytes(1, &int(1, 0x7f));
        for t in types {
            package.extend(bytes(3, t));
        }
        bytes(2, &package)
    }

    fn base_meta() -> ApkMeta {
        ApkMeta {
            label: Some("@res/0x7f020000".into()),
            application_icon: Some("@res/0x7f010000".into()),
            ..Default::default()
        }
    }

    #[test]
    fn varint_round_trip_and_overflow() {
        for v in [0, 1, 127, 128, 300, u64::from(u32::MAX), u64::MAX] {
            let encoded = varint_bytes(v);
            assert_eq!(varint(&mut encoded.as_slice()), Some(v));
        }
        // 第 10 个字节只能是 0 或 1，否则溢出 u64。
        let overflow = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02];
        assert_eq!(varint(&mut overflow.as_slice()), None);
        assert_eq!(varint(&mut [0x80].as_slice()), None);
    }

    #[test]
    fn message_rejects_truncated_and_unknown_wire_types() {
        assert!(Message::parse(&bytes(1, b"abc")[..3]).is_none());
        assert!(Message::parse(&[0x0b]).is_none()); // wire type 3（group）不支持
        assert!(Message::parse(&[0x00, 0x00]).is_none()); // 字段号 0 非法
        let data = msg(&[int(1, 5), int(1, 7)]);
        let m = Message::parse(&data).unwrap();
        assert_eq!(m.int(1), Some(7), "重复的标量字段以最后一个为准");
    }

    #[test]
    fn densities_skip_anydpi_and_nodpi_sentinels() {
        let icon = entry(
            0,
            "ic_launcher",
            &[
                config_value("", 480, file_item("res/mipmap-xxhdpi/ic.png")),
                config_value("", 0xFFFE, file_item("res/mipmap-anydpi-v26/ic.xml")),
                config_value("", 0xFFFF, file_item("res/drawable-nodpi/ic.png")),
            ],
        );
        let mut meta = base_meta();
        let mut index = BTreeMap::new();
        resources(
            &table(&[ty(1, "mipmap", &[icon])]),
            "base",
            &mut meta,
            &mut index,
        )
        .unwrap();
        assert_eq!(meta.densities, vec![480]);
        // 图标候选仍保留 anydpi 路径（自适应图标靠它渲染）。
        assert_eq!(
            meta.icon_paths.get(&0xFFFE).map(String::as_str),
            Some("base/res/mipmap-anydpi-v26/ic.xml")
        );
        assert_eq!(index[&0x7f010000].file_paths.len(), 3);
    }

    #[test]
    fn label_takes_first_default_config_like_apk() {
        let name = entry(
            0,
            "app_name",
            &[
                config_value("", 0, string_item("Default")),
                config_value("", 0, string_item("Night")),
                config_value("zh-CN", 0, string_item("默认")),
                config_value("zh-CN", 0, string_item("夜间")),
            ],
        );
        let mut meta = base_meta();
        resources(
            &table(&[ty(2, "string", &[name])]),
            "base",
            &mut meta,
            &mut BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(meta.label.as_deref(), Some("Default"));
        assert_eq!(meta.labels.get("zh-CN").map(String::as_str), Some("默认"));
        assert_eq!(meta.locales, vec!["zh-CN".to_string()]);
    }

    #[test]
    fn label_falls_back_to_any_config_without_default() {
        let name = entry(
            0,
            "app_name",
            &[config_value("en", 0, string_item("English"))],
        );
        let mut meta = base_meta();
        resources(
            &table(&[ty(2, "string", &[name])]),
            "base",
            &mut meta,
            &mut BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(meta.label.as_deref(), Some("English"));
    }

    #[test]
    fn malformed_records_are_skipped_without_losing_the_rest() {
        // 声明长度 3 实际只有 1 字节：Message::parse 必然失败。
        let truncated = bytes(1, b"abc")[..3].to_vec();
        let broken_type = bytes(2, b"no-type-id");
        let icon = entry(
            0,
            "ic_launcher",
            &[
                truncated.clone(),
                config_value("", 320, file_item("res/mipmap-xhdpi/ic.png")),
            ],
        );
        let name = entry(
            0,
            "app_name",
            &[config_value("ja", 0, string_item("アプリ"))],
        );
        let mut meta = base_meta();
        let mut index = BTreeMap::new();
        let mut mipmap = ty(1, "mipmap", &[icon]);
        mipmap.extend(bytes(3, &truncated)); // 损坏的 entry
                                             // 损坏的 package 排在正常 package 之前。
        let mut data = bytes(2, &truncated);
        data.extend(table(&[
            broken_type,
            truncated.clone(), // 损坏的 type
            mipmap,
            ty(2, "string", &[name]),
        ]));
        assert!(resources(&data, "base", &mut meta, &mut index).is_some());
        assert_eq!(meta.densities, vec![320]);
        assert_eq!(meta.locales, vec!["ja".to_string()]);
        assert!(index.contains_key(&0x7f010000));
    }

    #[test]
    fn feature_module_does_not_override_base_resources() {
        let base_icon = entry(0, "ic", &[config_value("", 480, file_item("res/a.png"))]);
        let feature_icon = entry(0, "ic", &[config_value("", 640, file_item("res/b.png"))]);
        let mut meta = base_meta();
        let mut index = BTreeMap::new();
        resources(
            &table(&[ty(1, "mipmap", &[base_icon])]),
            "base",
            &mut meta,
            &mut index,
        )
        .unwrap();
        resources(
            &table(&[ty(1, "mipmap", &[feature_icon])]),
            "feature",
            &mut meta,
            &mut index,
        )
        .unwrap();
        assert_eq!(
            index[&0x7f010000].file_paths,
            vec!["base/res/a.png".to_string()]
        );
        assert_eq!(meta.densities, vec![480, 640]);
    }

    fn compiled_attr(name: &str, item: Vec<u8>) -> Vec<u8> {
        msg(&[bytes(2, name.as_bytes()), bytes(6, &item)])
    }

    #[test]
    fn xml_attribute_values_match_axml_format() {
        let reference = bytes(1, &int(2, 0x7f01_0000));
        let attr_reference = bytes(1, &msg(&[int(1, 1), int(2, 0x0101_0435)]));
        let float_value = bytes(7, &float(3, 2.0));
        let bool_value = bytes(7, &int(8, 1));
        let element = msg(&[
            bytes(3, b"vector"),
            bytes(4, &compiled_attr("icon", reference)),
            bytes(4, &compiled_attr("fillColor", attr_reference)),
            bytes(4, &compiled_attr("alpha", float_value)),
            bytes(4, &compiled_attr("autoMirrored", bool_value)),
            bytes(4, &msg(&[bytes(2, b"raw"), bytes(3, b"text")])),
        ]);
        let root = xml_root(&bytes(1, &element)).unwrap();
        assert_eq!(root.name, "vector");
        assert_eq!(root.attr("icon"), Some("@res/0x7f010000"));
        assert_eq!(root.attr("fillColor"), Some("@attr/0x1010435"));
        assert_eq!(root.attr("alpha"), Some("2.0"));
        assert_eq!(root.attr("autoMirrored"), Some("true"));
        assert_eq!(root.attr("raw"), Some("text"));
    }

    #[test]
    fn xml_nesting_depth_is_bounded() {
        let mut node = bytes(1, &bytes(3, b"leaf"));
        for _ in 0..200 {
            node = bytes(1, &msg(&[bytes(3, b"g"), bytes(5, &node)]));
        }
        assert!(xml_root(&node).is_none());
    }
}
