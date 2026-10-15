use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// 采样窗口大小：每个采样点取这么多字节
const SAMPLE: usize = 32 * 1024;

/// 均匀采样点数：在整个文件上等距取这么多窗口，避免只看头/中/尾漏掉中部损坏
const SAMPLE_POINTS: usize = 32;

/// Shannon 熵（字节/字符），范围 0.0 ~ 8.0
pub fn shannon_entropy(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return 0.0;
    }
    let mut counts = [0u32; 256];
    for &b in bytes {
        counts[b as usize] += 1;
    }
    let len = bytes.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / len;
            -p * p.log2()
        })
        .sum()
}

/// 在整个文件上均匀取 `SAMPLE_POINTS` 个采样窗口，返回拼接后的采样字节。
///
/// 窗口中心在 `[0, size)` 上等距分布，边界 clamp 防越界；小文件重叠也没关系，
/// 反正只是给熵估计喂字节。相比只取头/中/尾三点，这样能覆盖中部的
/// 全零填充或异常区。
pub fn sample_file(path: &Path, size: u64) -> std::io::Result<Vec<u8>> {
    if size == 0 {
        return Ok(Vec::new());
    }
    let mut f = File::open(path)?;
    let n = SAMPLE as u64;
    let points = SAMPLE_POINTS as u64;

    // 窗口中心等距分布；不足一个窗口宽的小文件只取一次（中心=起点）
    let span = size.saturating_sub(n);
    let stride = if points > 1 { span / (points - 1) } else { 0 };

    let mut out = Vec::with_capacity((points * n) as usize);
    let mut buf = vec![0u8; SAMPLE];
    for i in 0..points {
        let off = if size <= n { 0 } else { i * stride };
        let _ = f.seek(SeekFrom::Start(off))?;
        let want = n.min(size - off) as usize;
        let mut got_total = 0;
        while got_total < want {
            match f.read(&mut buf[got_total..want])? {
                0 => break,
                k => got_total += k,
            }
        }
        out.extend_from_slice(&buf[..got_total]);
    }
    Ok(out)
}

/// 采样是否全部为 0（全零填充）
pub fn all_zero(sample: &[u8]) -> bool {
    !sample.is_empty() && sample.iter().all(|&b| b == 0)
}
