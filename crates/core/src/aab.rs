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

/// 将编译后的属性转成现有清单解析器和图标渲染器共用的字符串表示。
fn attribute_value(item: &Message<'_>) -> Option<String> {
    if let Some(id) = item_reference(item) {
        return Some(format!("@res/0x{id:08x}"));
    }
    if let Some(value) = item_string(item) {
        return Some(value.to_owned());
    }
    let primitive = item.message(7)?;
    if let Some(Value::Float(value)) = primitive.value(3) {
        return Some(value.to_string());
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
    for package in table.repeated(2) {
        let package = Message::parse(package)?;
        let package_id = package.message(1).and_then(|m| m.int(1)).unwrap_or(0);
        for ty in package.repeated(3) {
            let ty = Message::parse(ty)?;
            let type_id = ty.message(1)?.int(1)?;
            let type_name = ty.string(2)?;
            for entry in ty.repeated(3) {
                let entry = Message::parse(entry)?;
                // ID 0 在 proto3 中可能省略（例如该类型下第一条资源）。
                let entry_id = entry.message(1).and_then(|m| m.int(1)).unwrap_or(0);
                let id = (package_id << 24) | (type_id << 16) | entry_id;
                // 模块按 base 优先的顺序处理。重复 ID 仍贡献 locale 和 density，
                // 但不能覆盖已索引的图标路径、颜色和引用。
                let index_resource = !resources.contains_key(&id);
                for value in entry.repeated(6) {
                    let value = Message::parse(value)?;
                    let config = value.message(1);
                    let density = config.as_ref().and_then(|c| c.int(18)).unwrap_or(0);
                    let locale = config
                        .as_ref()
                        .and_then(|c| c.string(3))
                        .unwrap_or_default();
                    if !locale.is_empty() {
                        locales.insert(locale.to_owned());
                    }
                    if density != 0 {
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
                            if locale.is_empty() {
                                meta.label = Some(name.to_owned());
                            } else {
                                meta.labels.insert(locale.to_owned(), name.to_owned());
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
    meta.locales.extend(locales);
    meta.locales.sort_unstable();
    meta.locales.dedup();
    meta.densities.extend(densities);
    meta.densities.sort_unstable();
    meta.densities.dedup();
    Some(())
}
