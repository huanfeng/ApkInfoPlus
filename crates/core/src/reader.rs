//! 小端字节游标。所有二进制解析器共用；越界读返回错误而不是 panic，
//! 因为输入是用户给的任意文件（含刻意损坏的混淆 APK）。

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Eof {
    pub pos: usize,
    pub want: usize,
    pub len: usize,
}

impl fmt::Display for Eof {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unexpected end of data at {} (want {} bytes, total {})",
            self.pos, self.want, self.len
        )
    }
}

impl std::error::Error for Eof {}

pub struct ByteReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> ByteReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    pub fn len(&self) -> usize {
        self.data.len()
    }
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
    pub fn pos(&self) -> usize {
        self.pos
    }
    /// 定位；超出末尾则钉在末尾（随后读会报 Eof）。
    pub fn seek(&mut self, pos: usize) {
        self.pos = pos.min(self.data.len());
    }
    pub fn remain(&self) -> usize {
        self.data.len() - self.pos
    }
    pub fn skip(&mut self, n: usize) -> Result<(), Eof> {
        self.check(n)?;
        self.pos += n;
        Ok(())
    }
    fn check(&self, n: usize) -> Result<(), Eof> {
        if self.pos + n > self.data.len() {
            return Err(Eof {
                pos: self.pos,
                want: n,
                len: self.data.len(),
            });
        }
        Ok(())
    }
    pub fn u8(&mut self) -> Result<u8, Eof> {
        self.check(1)?;
        let v = self.data[self.pos];
        self.pos += 1;
        Ok(v)
    }
    pub fn u16(&mut self) -> Result<u16, Eof> {
        self.check(2)?;
        let v = u16::from_le_bytes([self.data[self.pos], self.data[self.pos + 1]]);
        self.pos += 2;
        Ok(v)
    }
    pub fn u32(&mut self) -> Result<u32, Eof> {
        self.check(4)?;
        let b = &self.data[self.pos..self.pos + 4];
        self.pos += 4;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub fn i32(&mut self) -> Result<i32, Eof> {
        Ok(self.u32()? as i32)
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], Eof> {
        self.check(n)?;
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    /// 不移动游标读取 u16（用于窥探 chunk 类型）。
    pub fn peek_u16(&self) -> Option<u16> {
        self.data
            .get(self.pos..self.pos + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
    }
    pub fn peek_u32_at(&self, pos: usize) -> Option<u32> {
        self.data
            .get(pos..pos + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_little_endian() {
        let d = [0x01u8, 0x02, 0x03, 0x04, 0xff];
        let mut r = ByteReader::new(&d);
        assert_eq!(r.u16().unwrap(), 0x0201);
        assert_eq!(r.u16().unwrap(), 0x0403);
        assert_eq!(r.u8().unwrap(), 0xff);
        assert!(r.u8().is_err());
    }

    #[test]
    fn seek_clamps_to_end() {
        let d = [0u8; 4];
        let mut r = ByteReader::new(&d);
        r.seek(100);
        assert_eq!(r.remain(), 0);
        assert!(r.u32().is_err());
    }
}
