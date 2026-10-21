//! 流式文件哈希：一次读取同时喂 MD5 与 SHA1，内存恒定。

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use md5::Digest as _;

/// 返回 `(md5, sha1)`，均为小写十六进制。
pub fn file_hashes(path: &Path) -> io::Result<(String, String)> {
    let mut f = File::open(path)?;
    let mut md5 = md5::Md5::new();
    let mut sha1 = sha1::Sha1::new();
    let mut buf = vec![0u8; 4 * 1024 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        md5.update(&buf[..n]);
        sha1.update(&buf[..n]);
    }
    Ok((hex::encode(md5.finalize()), hex::encode(sha1.finalize())))
}

/// 内存数据的 SHA-256 / SHA-1 / MD5（证书指纹用）。
pub fn digests(data: &[u8]) -> (String, String, String) {
    let sha256 = hex::encode(sha2::Sha256::digest(data));
    let sha1 = hex::encode(sha1::Sha1::digest(data));
    let md5 = hex::encode(md5::Md5::digest(data));
    (sha256, sha1, md5)
}

/// `ab:cd:ef` 风格的指纹格式。
pub fn colon_hex(hex: &str) -> String {
    hex.as_bytes()
        .chunks(2)
        .map(|c| std::str::from_utf8(c).unwrap_or("??").to_uppercase())
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        let (s256, s1, m) = digests(b"abc");
        assert_eq!(
            s256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(s1, "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(m, "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(colon_hex("abcd"), "AB:CD");
    }

    #[test]
    fn file_hash_matches_memory() {
        let dir = std::env::temp_dir().join(format!("apkinfo-hash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.bin");
        std::fs::write(&p, b"hello world").unwrap();
        let (m, s) = file_hashes(&p).unwrap();
        assert_eq!(m, "5eb63bbbe01eeed093cb22bb8f5acdc3");
        assert_eq!(s, "2aae6c35c94fcfb415dbe95f408b9ce91ee846ed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
