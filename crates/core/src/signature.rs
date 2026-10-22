//! APK 签名证书提取：v1（META-INF 的 PKCS#7）与 v2 / v3 / v3.1（APK Signing Block）。
//!
//! 只提取并解析证书，不验证摘要与签名——目标是把「谁签的」展示出来，而不是替代 apksigner。

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::hash::{colon_hex, digests};
use crate::info::{SignatureInfo, SignerInfo};

const APK_SIG_BLOCK_MAGIC: &[u8; 16] = b"APK Sig Block 42";
const V2_ID: u32 = 0x7109_871a;
const V3_ID: u32 = 0xf053_68c0;
const V31_ID: u32 = 0x1b93_ad61;
const STAMP_V2_ID: u32 = 0x6dff_800d;
const STAMP_V1_ID: u32 = 0x2b09_189e;

pub fn parse_file(path: &Path) -> Result<SignatureInfo, String> {
    let mut f = File::open(path).map_err(|e| e.to_string())?;
    let mut info = SignatureInfo::default();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    // ---- v2 / v3：Signing Block ----
    match read_signing_block(&mut f) {
        Ok(Some(block)) => {
            for (id, value) in parse_block_pairs(&block) {
                match id {
                    V2_ID => {
                        info.has_v2 = true;
                        collect_block_signers(value, &mut info, &mut seen, "v2");
                    }
                    V3_ID => {
                        info.has_v3 = true;
                        collect_block_signers(value, &mut info, &mut seen, "v3");
                    }
                    V31_ID => {
                        info.has_v31 = true;
                        collect_block_signers(value, &mut info, &mut seen, "v3.1");
                    }
                    STAMP_V2_ID | STAMP_V1_ID => info.has_v4_stamp = true,
                    _ => {}
                }
            }
        }
        Ok(None) => {}
        Err(e) => info.warnings.push(format!("signing block: {}", e)),
    }

    // ---- v1：META-INF/*.RSA|DSA|EC ----
    f.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    match crate::zip::open_archive(std::io::BufReader::new(f)) {
        Ok(mut zip) => {
            let names: Vec<String> = zip
                .file_names()
                .filter_map(|n| n.ok().map(|c| c.into_owned()))
                .filter(|n| {
                    let l = n.to_ascii_lowercase();
                    l.starts_with("meta-inf/")
                        && !l[9..].contains('/')
                        && (l.ends_with(".rsa") || l.ends_with(".dsa") || l.ends_with(".ec"))
                })
                .collect();
            for name in names {
                info.has_v1 = true;
                let mut bytes = Vec::new();
                if let Ok(mut e) = zip.by_name(&name) {
                    let _ = e.read_to_end(&mut bytes);
                }
                match pkcs7_certificates(&bytes) {
                    Ok(certs) => {
                        for c in certs {
                            push_signer(c, &mut info, &mut seen);
                        }
                    }
                    Err(e) => info.warnings.push(format!("{}: {}", name, e)),
                }
            }
        }
        Err(e) => info.warnings.push(format!("zip: {}", e)),
    }

    Ok(info)
}

fn collect_block_signers(
    value: &[u8],
    info: &mut SignatureInfo,
    seen: &mut BTreeSet<String>,
    scheme: &str,
) {
    match block_certificates(value) {
        Ok(certs) => {
            for c in certs {
                push_signer(c, info, seen);
            }
        }
        Err(e) => info.warnings.push(format!("{}: {}", scheme, e)),
    }
}

fn push_signer(der: &[u8], info: &mut SignatureInfo, seen: &mut BTreeSet<String>) {
    let (sha256, _, _) = digests(der);
    if !seen.insert(sha256) {
        return;
    }
    match describe_certificate(der) {
        Ok(s) => info.signers.push(s),
        Err(e) => info.warnings.push(format!("certificate: {}", e)),
    }
}

// ---------------------------------------------------------------------------
// ZIP 定位：EOCD → 中央目录偏移 → Signing Block
// ---------------------------------------------------------------------------

/// 读出整个 APK Signing Block（含首尾 size 与 magic）；没有返回 `None`。
fn read_signing_block<R: Read + Seek>(f: &mut R) -> Result<Option<Vec<u8>>, String> {
    let len = f.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
    if len < 22 {
        return Err("file too small".into());
    }
    // EOCD 注释最长 65535，所以倒着扫最后 64KB+22。
    let tail_len = len.min(65_535 + 22);
    f.seek(SeekFrom::Start(len - tail_len))
        .map_err(|e| e.to_string())?;
    let mut tail = vec![0u8; tail_len as usize];
    f.read_exact(&mut tail).map_err(|e| e.to_string())?;
    let eocd_rel = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&i| {
            tail[i..i + 4] == [0x50, 0x4b, 0x05, 0x06] && {
                let comment_len = u16::from_le_bytes([tail[i + 20], tail[i + 21]]) as usize;
                i + 22 + comment_len == tail.len()
            }
        })
        .ok_or("eocd not found")?;
    let eocd = &tail[eocd_rel..];
    let mut cd_offset = u32::from_le_bytes([eocd[16], eocd[17], eocd[18], eocd[19]]) as u64;
    let eocd_abs = len - tail_len + eocd_rel as u64;

    if cd_offset == 0xFFFF_FFFF {
        // zip64：EOCD 前 20 字节是 locator，指向 zip64 EOCD record。
        if eocd_abs < 20 {
            return Err("zip64 locator missing".into());
        }
        f.seek(SeekFrom::Start(eocd_abs - 20))
            .map_err(|e| e.to_string())?;
        let mut loc = [0u8; 20];
        f.read_exact(&mut loc).map_err(|e| e.to_string())?;
        if loc[..4] != [0x50, 0x4b, 0x06, 0x07] {
            return Err("zip64 locator signature mismatch".into());
        }
        let rec_off = u64::from_le_bytes(loc[8..16].try_into().unwrap());
        f.seek(SeekFrom::Start(rec_off))
            .map_err(|e| e.to_string())?;
        let mut rec = [0u8; 56];
        f.read_exact(&mut rec).map_err(|e| e.to_string())?;
        if rec[..4] != [0x50, 0x4b, 0x06, 0x06] {
            return Err("zip64 eocd signature mismatch".into());
        }
        cd_offset = u64::from_le_bytes(rec[48..56].try_into().unwrap());
    }

    if cd_offset < 32 || cd_offset > len {
        return Ok(None);
    }
    // 中央目录前 24 字节：u64 size_of_block + 16 字节 magic。
    f.seek(SeekFrom::Start(cd_offset - 24))
        .map_err(|e| e.to_string())?;
    let mut footer = [0u8; 24];
    f.read_exact(&mut footer).map_err(|e| e.to_string())?;
    if &footer[8..24] != APK_SIG_BLOCK_MAGIC {
        return Ok(None);
    }
    let size_of_block = u64::from_le_bytes(footer[..8].try_into().unwrap());
    // 全部用 checked / 减法形式比较：size 来自文件，u64 加法在恶意输入下会回绕。
    let plausible = (24..=64 * 1024 * 1024).contains(&size_of_block)
        && size_of_block.checked_add(8).is_some_and(|t| t <= cd_offset);
    if !plausible {
        return Err(format!("implausible signing block size {}", size_of_block));
    }
    let start = cd_offset - size_of_block - 8;
    f.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
    let mut block = vec![0u8; (size_of_block + 8) as usize];
    f.read_exact(&mut block).map_err(|e| e.to_string())?;
    let head = u64::from_le_bytes(block[..8].try_into().unwrap());
    if head != size_of_block {
        return Err("signing block size mismatch".into());
    }
    Ok(Some(block))
}

/// 拆出 (id, value) 对。`block` 含首尾 size 与 magic。
fn parse_block_pairs(block: &[u8]) -> Vec<(u32, &[u8])> {
    let mut out = Vec::new();
    if block.len() < 32 {
        return out;
    }
    let end = block.len() - 24;
    let mut pos = 8;
    while pos + 12 <= end {
        let len = u64::from_le_bytes(block[pos..pos + 8].try_into().unwrap()) as usize;
        pos += 8;
        // `len > end - pos` 而不是 `pos + len > end`：后者在 len 极大时回绕后能通过检查
        if len < 4 || len > end - pos {
            break;
        }
        let id = u32::from_le_bytes(block[pos..pos + 4].try_into().unwrap());
        out.push((id, &block[pos + 4..pos + len]));
        pos += len;
    }
    out
}

/// 读一个 u32 长度前缀的片段。
fn take_prefixed<'a>(data: &'a [u8], pos: &mut usize) -> Result<&'a [u8], String> {
    if *pos + 4 > data.len() {
        return Err("truncated length prefix".into());
    }
    let len = u32::from_le_bytes(data[*pos..*pos + 4].try_into().unwrap()) as usize;
    *pos += 4;
    if len > data.len() - *pos {
        return Err("truncated length-prefixed field".into());
    }
    let s = &data[*pos..*pos + len];
    *pos += len;
    Ok(s)
}

/// v2 / v3 块共同的前缀结构：signers → signer → signed data → digests, certificates。
fn block_certificates(value: &[u8]) -> Result<Vec<&[u8]>, String> {
    let mut out = Vec::new();
    let mut p = 0;
    let signers = take_prefixed(value, &mut p)?;
    let mut sp = 0;
    while sp < signers.len() {
        let signer = take_prefixed(signers, &mut sp)?;
        let mut q = 0;
        let signed_data = take_prefixed(signer, &mut q)?;
        let mut sd = 0;
        let _digests = take_prefixed(signed_data, &mut sd)?;
        let certs = take_prefixed(signed_data, &mut sd)?;
        let mut cp = 0;
        while cp < certs.len() {
            out.push(take_prefixed(certs, &mut cp)?);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// PKCS#7 / DER
// ---------------------------------------------------------------------------

/// 极简 DER TLV：返回 (tag, 内容, 整个元素长度)。
fn der_tlv(data: &[u8]) -> Option<(u8, &[u8], usize)> {
    let tag = *data.first()?;
    let first_len = *data.get(1)? as usize;
    let (len, hdr) = if first_len < 0x80 {
        (first_len, 2)
    } else {
        let n = first_len & 0x7f;
        if n == 0 || n > 4 || data.len() < 2 + n {
            return None;
        }
        let mut l = 0usize;
        for b in &data[2..2 + n] {
            l = (l << 8) | *b as usize;
        }
        (l, 2 + n)
    };
    let end = hdr.checked_add(len)?;
    if end > data.len() {
        return None;
    }
    Some((tag, &data[hdr..end], end))
}

fn der_children(content: &[u8]) -> Vec<(u8, &[u8], &[u8])> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < content.len() {
        let Some((tag, inner, total)) = der_tlv(&content[pos..]) else {
            break;
        };
        out.push((tag, inner, &content[pos..pos + total]));
        pos += total;
    }
    out
}

/// 从 PKCS#7 SignedData 里取出全部证书（DER 原文）。
pub fn pkcs7_certificates(bytes: &[u8]) -> Result<Vec<&[u8]>, String> {
    let (tag, content, _) = der_tlv(bytes).ok_or("not DER")?;
    if tag != 0x30 {
        return Err("ContentInfo is not a SEQUENCE".into());
    }
    let ci = der_children(content);
    // [oid, [0] EXPLICIT SignedData]
    let (_, explicit, _) = ci.get(1).ok_or("missing content")?;
    let signed = der_children(explicit);
    let (stag, sd, _) = signed.first().ok_or("missing SignedData")?;
    if *stag != 0x30 {
        return Err("SignedData is not a SEQUENCE".into());
    }
    // version, digestAlgorithms, encapContentInfo, [0] certificates, [1] crls, signerInfos
    let fields = der_children(sd);
    let certs = fields
        .iter()
        .skip(3)
        .find(|(t, _, _)| *t == 0xA0)
        .ok_or("no certificates")?;
    Ok(der_children(certs.1)
        .into_iter()
        .filter(|(t, _, _)| *t == 0x30)
        .map(|(_, _, whole)| whole)
        .collect())
}

fn oid_name(oid: &str) -> String {
    match oid {
        "1.2.840.113549.1.1.1" => "RSA".into(),
        "1.2.840.113549.1.1.4" => "MD5withRSA".into(),
        "1.2.840.113549.1.1.5" => "SHA1withRSA".into(),
        "1.2.840.113549.1.1.11" => "SHA256withRSA".into(),
        "1.2.840.113549.1.1.12" => "SHA384withRSA".into(),
        "1.2.840.113549.1.1.13" => "SHA512withRSA".into(),
        "1.2.840.113549.1.1.10" => "RSASSA-PSS".into(),
        "1.2.840.10045.2.1" => "EC".into(),
        "1.2.840.10045.4.1" => "SHA1withECDSA".into(),
        "1.2.840.10045.4.3.2" => "SHA256withECDSA".into(),
        "1.2.840.10045.4.3.3" => "SHA384withECDSA".into(),
        "1.2.840.10045.4.3.4" => "SHA512withECDSA".into(),
        "1.2.840.10040.4.1" => "DSA".into(),
        "1.2.840.10040.4.3" => "SHA1withDSA".into(),
        "2.16.840.1.101.3.4.3.2" => "SHA256withDSA".into(),
        "1.3.101.112" => "Ed25519".into(),
        other => other.to_string(),
    }
}

/// RDN 倒序显示（`CN=..., OU=..., O=..., C=..`），与 apksigner / keytool 一致；
/// x509-parser 按 DER 编码顺序给出的是 `C=.., O=.., CN=..`。
fn dn_string(name: &x509_parser::x509::X509Name<'_>) -> String {
    let text = name.to_string();
    let mut parts: Vec<&str> = text.split(", ").collect();
    parts.reverse();
    parts.join(", ")
}

fn describe_certificate(der: &[u8]) -> Result<SignerInfo, String> {
    use x509_parser::prelude::*;
    use x509_parser::public_key::PublicKey;
    let (_, cert) = X509Certificate::from_der(der).map_err(|e| e.to_string())?;
    let (sha256, sha1, md5) = digests(der);
    let (key_algorithm, key_size_bits) = match cert.public_key().parsed() {
        Ok(PublicKey::RSA(rsa)) => ("RSA".to_string(), Some(rsa.key_size() as u32)),
        Ok(PublicKey::EC(ec)) => ("EC".to_string(), Some(ec.key_size() as u32)),
        Ok(PublicKey::DSA(y)) => ("DSA".to_string(), Some((y.len() * 8) as u32)),
        Ok(PublicKey::GostR3410(_)) | Ok(PublicKey::GostR3410_2012(_)) => {
            ("GOST".to_string(), None)
        }
        Ok(PublicKey::Unknown(_)) | Err(_) => {
            // Ed25519 等 x509-parser 不认识的算法：按 OID 给名字，长度未知。
            (
                oid_name(&cert.public_key().algorithm.algorithm.to_id_string()),
                None,
            )
        }
    };
    Ok(SignerInfo {
        subject: dn_string(cert.subject()),
        issuer: dn_string(cert.issuer()),
        serial_hex: cert.raw_serial_as_string().to_uppercase(),
        not_before: cert.validity().not_before.to_string(),
        not_after: cert.validity().not_after.to_string(),
        sha256: colon_hex(&sha256),
        sha1: colon_hex(&sha1),
        md5: colon_hex(&md5),
        key_algorithm,
        key_size_bits,
        signature_algorithm: oid_name(&cert.signature_algorithm.algorithm.to_id_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_pairs_roundtrip() {
        // 手工拼一个含单对 (id=0x7109871a, value=[1,2,3]) 的 signing block。
        let value = [1u8, 2, 3];
        let pair_len = (4 + value.len()) as u64;
        let mut body = Vec::new();
        body.extend_from_slice(&pair_len.to_le_bytes());
        body.extend_from_slice(&V2_ID.to_le_bytes());
        body.extend_from_slice(&value);
        let size_of_block = (body.len() + 24) as u64;
        let mut block = Vec::new();
        block.extend_from_slice(&size_of_block.to_le_bytes());
        block.extend_from_slice(&body);
        block.extend_from_slice(&size_of_block.to_le_bytes());
        block.extend_from_slice(APK_SIG_BLOCK_MAGIC);
        let pairs = parse_block_pairs(&block);
        assert_eq!(pairs, vec![(V2_ID, &value[..])]);
    }

    #[test]
    fn der_walker() {
        // SEQUENCE { INTEGER 1, OCTET STRING "ab" }
        let seq = [0x30, 0x07, 0x02, 0x01, 0x01, 0x04, 0x02, b'a', b'b'];
        let (tag, content, total) = der_tlv(&seq).unwrap();
        assert_eq!((tag, total), (0x30, 9));
        let ch = der_children(content);
        assert_eq!(ch.len(), 2);
        assert_eq!(ch[1].1, b"ab");
        // 长形式长度
        let long = [0x04, 0x81, 0x02, 0xaa, 0xbb];
        assert_eq!(der_tlv(&long).unwrap().1, &[0xaa, 0xbb]);
    }

    #[test]
    fn oid_names() {
        assert_eq!(oid_name("1.2.840.113549.1.1.11"), "SHA256withRSA");
        assert_eq!(oid_name("9.9"), "9.9");
    }
}

#[cfg(test)]
mod manual {
    /// 手工对照：`APKINFO_SIG_APK=/path/x.apk cargo test -p apkinfo-core sig_dump -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn sig_dump() {
        let Ok(p) = std::env::var("APKINFO_SIG_APK") else {
            return;
        };
        let info = super::parse_file(std::path::Path::new(&p)).unwrap();
        println!("{:#?}", info);
    }
}
