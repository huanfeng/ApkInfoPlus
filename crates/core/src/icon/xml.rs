//! XML 来源统一：二进制 XML 走 `axml::parse_document`，文本 XML 用 roxmltree 解析后
//! 转成同样的 [`XmlElement`]，让渲染器只面对一种树。

use std::collections::BTreeMap;

use crate::axml::{self, XmlElement};

/// 文本节点内容存放的伪属性名。`XmlElement` 没有文本字段，而 `<color>#fff</color>`
/// 这类资源需要文本；`#` 不可能出现在合法属性名里，所以不会与真实属性冲突。
pub(crate) const TEXT_KEY: &str = "#text";

/// 解析 XML 字节为顶层元素（`<vector>` / `<adaptive-icon>` 等）。
pub(crate) fn parse_root(bytes: &[u8]) -> Option<XmlElement> {
    if axml::is_binary_xml(bytes) {
        match axml::parse_document(bytes) {
            Ok(doc) => return doc.children.into_iter().next(),
            Err(e) => {
                log::debug!("icon: binary xml parse failed: {e}");
                return None;
            }
        }
    }
    parse_text(bytes)
}

/// 文本 XML → [`XmlElement`]。元素名与属性名都去掉命名空间前缀，
/// 与二进制 XML 解析结果保持一致（`android:fillColor` → `fillColor`）。
pub(crate) fn parse_text(bytes: &[u8]) -> Option<XmlElement> {
    let text = String::from_utf8_lossy(bytes);
    // 去掉 BOM，roxmltree 不接受
    let text = text.trim_start_matches('\u{feff}');
    let opts = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let doc = match roxmltree::Document::parse_with_options(text, opts) {
        Ok(d) => d,
        Err(e) => {
            log::debug!("icon: text xml parse failed: {e}");
            return None;
        }
    };
    Some(convert(doc.root_element()))
}

fn convert(node: roxmltree::Node) -> XmlElement {
    let mut attributes = BTreeMap::new();
    for a in node.attributes() {
        attributes.insert(a.name().to_string(), a.value().to_string());
    }
    let mut children = Vec::new();
    let mut text = String::new();
    for c in node.children() {
        if c.is_element() {
            children.push(convert(c));
        } else if c.is_text() {
            if let Some(t) = c.text() {
                text.push_str(t);
            }
        }
    }
    let text = text.trim();
    if !text.is_empty() {
        attributes.insert(TEXT_KEY.to_string(), text.to_string());
    }
    XmlElement {
        name: node.tag_name().name().to_string(),
        attributes,
        children,
        extra_strings: Vec::new(),
    }
}

/// 元素本地名（小写，去掉可能残留的 `prefix:`）。
pub(crate) fn tag(e: &XmlElement) -> String {
    local_name(&e.name).to_ascii_lowercase()
}

fn local_name(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

/// 按本地名取属性；先精确匹配，再容忍残留前缀（`android:xxx`）。
pub(crate) fn attr<'a>(e: &'a XmlElement, name: &str) -> Option<&'a str> {
    if let Some(v) = e.attr(name) {
        return Some(v);
    }
    e.attributes
        .iter()
        .find(|(k, _)| k.as_str() != TEXT_KEY && local_name(k) == name)
        .map(|(_, v)| v.as_str())
}

pub(crate) fn attr_opt<'a>(e: Option<&'a XmlElement>, name: &str) -> Option<&'a str> {
    e.and_then(|e| attr(e, name))
}

/// 元素文本（来自文本 XML）。
pub(crate) fn text(e: &XmlElement) -> Option<&str> {
    e.attributes.get(TEXT_KEY).map(|s| s.as_str())
}

/// 第一个本地名匹配（大小写不敏感）的直接子元素。
pub(crate) fn find_child<'a>(e: &'a XmlElement, name: &str) -> Option<&'a XmlElement> {
    e.children
        .iter()
        .find(|c| local_name(&c.name).eq_ignore_ascii_case(name))
}

pub(crate) fn children_named<'a>(
    e: &'a XmlElement,
    name: &'a str,
) -> impl Iterator<Item = &'a XmlElement> + 'a {
    e.children
        .iter()
        .filter(move |c| local_name(&c.name).eq_ignore_ascii_case(name))
}

/// 遍历非文本属性（按名字排序）。
pub(crate) fn real_attrs(e: &XmlElement) -> impl Iterator<Item = (&str, &str)> {
    e.attributes
        .iter()
        .filter(|(k, _)| k.as_str() != TEXT_KEY)
        .map(|(k, v)| (k.as_str(), v.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_xml_strips_namespaces() {
        let src = br##"<?xml version="1.0" encoding="utf-8"?>
<vector xmlns:android="http://schemas.android.com/apk/res/android"
    xmlns:aapt="http://schemas.android.com/aapt"
    android:width="24dp" android:viewportWidth="24">
  <path android:pathData="M0,0h1" android:fillColor="#ff0000">
    <aapt:attr name="android:fillColor"><gradient android:type="linear"/></aapt:attr>
  </path>
</vector>"##;
        let root = parse_root(src).expect("parse");
        assert_eq!(root.name, "vector");
        assert_eq!(attr(&root, "viewportWidth"), Some("24"));
        let path = &root.children[0];
        assert_eq!(attr(path, "fillColor"), Some("#ff0000"));
        let aapt = find_child(path, "attr").expect("aapt:attr");
        assert_eq!(attr(aapt, "name"), Some("android:fillColor"));
        assert_eq!(tag(&aapt.children[0]), "gradient");
    }

    #[test]
    fn text_content_kept() {
        let root = parse_root(b"<color>  #ff00ff00 </color>").expect("parse");
        assert_eq!(text(&root), Some("#ff00ff00"));
        assert_eq!(real_attrs(&root).count(), 0);
    }

    #[test]
    fn garbage_is_none() {
        assert!(parse_root(b"\x00\x01garbage").is_none());
    }
}
