//! 顺序字节流读取器：基于 `BufReader::fill_buf/consume` 的顺序访问，带逻辑位置跟踪。
//! 供各格式验证器做**流式结构遍历**，使 GB 级大文件无需整读进内存（消灭 TOO_LARGE 跳过）。

use std::io::{BufRead, Seek, SeekFrom};

pub struct ByteStream<'a, R: BufRead + Seek> {
    r: &'a mut R,
    pos: u64,
}

impl<'a, R: BufRead + Seek> ByteStream<'a, R> {
    pub fn new(r: &'a mut R) -> Self {
        Self { r, pos: 0 }
    }

    /// 当前逻辑偏移
    pub fn pos(&self) -> u64 {
        self.pos
    }

    /// 读 1 字节；EOF 返回 None
    pub fn byte(&mut self) -> Option<u8> {
        let buf = self.r.fill_buf().ok()?;
        if buf.is_empty() {
            return None;
        }
        let b = buf[0];
        self.r.consume(1);
        self.pos += 1;
        Some(b)
    }

    /// 窥视下一字节（不消耗）
    pub fn peek_byte(&mut self) -> Option<u8> {
        let buf = self.r.fill_buf().ok()?;
        buf.first().copied()
    }

    /// 尽量读满，返回实际读到的字节数
    pub fn read_up_to(&mut self, dst: &mut [u8]) -> usize {
        let mut total = 0;
        while total < dst.len() {
            let buf = match self.r.fill_buf() {
                Ok(b) => b,
                Err(_) => break,
            };
            if buf.is_empty() {
                break;
            }
            let take = (dst.len() - total).min(buf.len());
            dst[total..total + take].copy_from_slice(&buf[..take]);
            self.r.consume(take);
            self.pos += take as u64;
            total += take;
        }
        total
    }

    /// 读满返回 true；提前 EOF 时已读部分保留在 `dst` 中并返回 false
    pub fn read_exact(&mut self, dst: &mut [u8]) -> bool {
        self.read_up_to(dst) == dst.len()
    }

    /// 读 2 字节大端
    pub fn read_u16_be(&mut self) -> Option<u16> {
        let mut b = [0u8; 2];
        self.read_exact(&mut b).then(|| u16::from_be_bytes(b))
    }

    /// 绝对定位。注意 seek 超过 EOF 不报错，截断判断由调用方对照文件大小做。
    pub fn seek_to(&mut self, pos: u64) {
        if self.r.seek(SeekFrom::Start(pos)).is_ok() {
            self.pos = pos;
        }
    }

    /// 跳过 n 字节（相对当前位）
    pub fn skip(&mut self, n: u64) {
        self.seek_to(self.pos.saturating_add(n));
    }
}
