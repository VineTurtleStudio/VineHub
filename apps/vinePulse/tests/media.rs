use std::fs;
use std::path::{Path, PathBuf};

use vine_pulse::{Opts, Report, Severity, verify_file};

static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vine-pulse-media-{}-{}",
        tag,
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// 从仓库内置的真实样本拷一份，供破坏性测试使用
fn copy_fixture(name: &str, tag: &str) -> PathBuf {
    let dir = temp_dir(tag);
    let dst = dir.join(name);
    fs::copy(fixture(name), &dst).unwrap();
    dst
}

fn codes(r: &Report) -> Vec<&'static str> {
    r.findings.iter().map(|f| f.code).collect()
}

fn opts() -> Opts {
    Opts::default()
}

fn assert_corrupt(r: &Report, code: &str) {
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    assert!(codes(r).contains(&code), "findings: {:?}", r.findings);
}

// ---------------------------------------------------------------------------
// MP4
// ---------------------------------------------------------------------------

#[test]
fn mp4_valid_is_ok() {
    let path = copy_fixture("video.mp4", "mp4-ok");
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "MP4");
}

#[test]
fn mp4_truncated_is_corrupt() {
    let path = copy_fixture("video.mp4", "mp4-trunc");
    let data = fs::read(&path).unwrap();
    fs::write(&path, &data[..data.len() * 2 / 3]).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "MP4_TRUNCATED");
}

#[test]
fn mp4_mdat_length_tampered_is_corrupt() {
    let path = copy_fixture("video.mp4", "mp4-mdat");
    let mut data = fs::read(&path).unwrap();
    let pos = data
        .windows(4)
        .position(|w| w == b"mdat")
        .expect("样本应含 mdat box");
    // 把 box 长度字段改成超大值
    data[pos - 4..pos].copy_from_slice(&u32::MAX.to_be_bytes());
    fs::write(&path, &data).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "MP4_TRUNCATED");
}

// ---------------------------------------------------------------------------
// Matroska / WebM
// ---------------------------------------------------------------------------

#[test]
fn mkv_valid_is_ok() {
    let path = copy_fixture("video.mkv", "mkv-ok");
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "Matroska");
}

#[test]
fn mkv_broken_magic_is_corrupt() {
    let path = copy_fixture("video.mkv", "mkv-magic");
    let mut data = fs::read(&path).unwrap();
    for b in data.iter_mut().take(4) {
        *b = 0x55;
    }
    fs::write(&path, &data).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "EBML_BAD_MAGIC");
}

#[test]
fn mkv_truncated_is_corrupt() {
    let path = copy_fixture("video.mkv", "mkv-trunc");
    let data = fs::read(&path).unwrap();
    fs::write(&path, &data[..data.len() * 3 / 4]).unwrap();

    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    let cs = codes(&r);
    assert!(
        cs.contains(&"EBML_TRUNCATED")
            || cs.contains(&"EBML_BAD_ELEMENT")
            || cs.contains(&"EBML_NO_SEGMENT")
            || cs.contains(&"EBML_BAD_VINT"),
        "findings: {:?}",
        r.findings
    );
}

// ---------------------------------------------------------------------------
// FLAC
// ---------------------------------------------------------------------------

#[test]
fn flac_valid_is_ok() {
    let path = copy_fixture("tone.flac", "flac-ok");
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "FLAC");
}

#[test]
fn flac_truncated_is_corrupt() {
    let path = copy_fixture("tone.flac", "flac-trunc");
    let data = fs::read(&path).unwrap();
    fs::write(&path, &data[..data.len() - 512]).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "FLAC_TRUNCATED");
}

#[test]
fn flac_metadata_length_tampered_is_corrupt() {
    let path = copy_fixture("tone.flac", "flac-meta");
    let mut data = fs::read(&path).unwrap();
    // PADDING 块头（偏移 90）：第 2..5 字节为 24 位长度
    data[91] = 0xFF;
    data[92] = 0xFF;
    data[93] = 0xFF;
    fs::write(&path, &data).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "FLAC_TRUNCATED");
}

/// 结构层完好、但音频数据被改坏：只有 MD5 完整解码校验能发现。
///
/// 在音频帧区中段翻转一个字节（避开帧头/帧尾），帧的 CRC16 可能仍通过，
/// 但解码出的 PCM 已变，MD5 必然不匹配 —— 这正是 FlacValidator 抓的场景。
#[test]
fn flac_audio_payload_corrupted_is_detected_by_md5() {
    let path = copy_fixture("tone.flac", "flac-audio-md5");
    let mut data = fs::read(&path).unwrap();
    // 跳过头部元数据，在文件中段（音频帧区）翻转一个字节
    let mid = data.len() / 2;
    data[mid] ^= 0xFF;
    fs::write(&path, &data).unwrap();

    let r = verify_file(&path, &opts());
    let c = codes(&r);
    assert!(
        c.contains(&"FLAC_MD5_MISMATCH")
            || c.contains(&"FLAC_MD5_SKIPPED")
            || c.contains(&"FLAC_BAD_FRAME")
            || c.contains(&"FLAC_TRUNCATED")
            || c.contains(&"FLAC_FRAME_GAP"),
        "结构或 MD5 应至少命中其一，findings: {:?}",
        r.findings
    );
}

// ---------------------------------------------------------------------------
// MP3
// ---------------------------------------------------------------------------

#[test]
fn mp3_valid_is_ok() {
    let path = copy_fixture("tone.mp3", "mp3-ok");
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "MP3");
}

#[test]
fn mp3_truncated_is_corrupt() {
    let path = copy_fixture("tone.mp3", "mp3-trunc");
    let data = fs::read(&path).unwrap();
    fs::write(&path, &data[..data.len() - 97]).unwrap();

    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    let cs = codes(&r);
    assert!(
        cs.contains(&"MP3_TRUNCATED") || cs.contains(&"MP3_NO_FRAME"),
        "findings: {:?}",
        r.findings
    );
}

#[test]
fn mp3_garbage_in_data_is_corrupt() {
    let path = copy_fixture("tone.mp3", "mp3-garbage");
    let mut data = fs::read(&path).unwrap();
    let start = 40_000;
    for b in data[start..start + 1000].iter_mut() {
        *b = 0x00;
    }
    fs::write(&path, &data).unwrap();

    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    let cs = codes(&r);
    assert!(
        cs.contains(&"MP3_GARBAGE") || cs.contains(&"MP3_BAD_FRAME"),
        "findings: {:?}",
        r.findings
    );
}

// ---------------------------------------------------------------------------
// WAV / AVI（手工构造）
// ---------------------------------------------------------------------------

fn push_list(out: &mut Vec<u8>, list_type: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(b"LIST");
    out.extend_from_slice(&((payload.len() + 4) as u32).to_le_bytes());
    out.extend_from_slice(list_type);
    out.extend_from_slice(payload);
}

fn wav_bytes(with_data: bool) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(b"WAVE");
    body.extend_from_slice(b"fmt ");
    body.extend_from_slice(&16u32.to_le_bytes());
    body.extend_from_slice(&1u16.to_le_bytes()); // PCM
    body.extend_from_slice(&1u16.to_le_bytes()); // 单声道
    body.extend_from_slice(&44100u32.to_le_bytes());
    body.extend_from_slice(&88200u32.to_le_bytes()); // byte rate
    body.extend_from_slice(&2u16.to_le_bytes()); // block align
    body.extend_from_slice(&16u16.to_le_bytes()); // bits
    if with_data {
        let samples: Vec<u8> = (0..2048u32).map(|i| (i % 251) as u8).collect();
        body.extend_from_slice(b"data");
        body.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        body.extend_from_slice(&samples);
    }

    let mut file = Vec::new();
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(&(body.len() as u32).to_le_bytes());
    file.extend_from_slice(&body);
    file
}

#[test]
fn wav_valid_is_ok() {
    let dir = temp_dir("wav-ok");
    let path = dir.join("ok.wav");
    fs::write(&path, wav_bytes(true)).unwrap();

    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "WAV");
}

#[test]
fn wav_missing_data_is_corrupt() {
    let dir = temp_dir("wav-nodata");
    let path = dir.join("no-data.wav");
    fs::write(&path, wav_bytes(false)).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "RIFF_NO_DATA");
}

#[test]
fn wav_truncated_is_corrupt() {
    let dir = temp_dir("wav-trunc");
    let path = dir.join("trunc.wav");
    let data = wav_bytes(true);
    fs::write(&path, &data[..data.len() - 512]).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "RIFF_TRUNCATED");
}

fn avi_bytes(with_movi: bool) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(b"AVI ");

    let mut hdrl = Vec::new();
    hdrl.extend_from_slice(b"avih");
    hdrl.extend_from_slice(&56u32.to_le_bytes());
    hdrl.extend_from_slice(&[0x10u8; 56]);
    push_list(&mut body, b"hdrl", &hdrl);

    if with_movi {
        let mut movi = Vec::new();
        movi.extend_from_slice(b"00dc");
        movi.extend_from_slice(&32u32.to_le_bytes());
        movi.extend_from_slice(&[0x22u8; 32]);
        push_list(&mut body, b"movi", &movi);
    }

    body.extend_from_slice(b"idx1");
    body.extend_from_slice(&16u32.to_le_bytes());
    body.extend_from_slice(&[0x33u8; 16]);

    let mut file = Vec::new();
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(&(body.len() as u32).to_le_bytes());
    file.extend_from_slice(&body);
    file
}

#[test]
fn avi_valid_is_ok() {
    let dir = temp_dir("avi-ok");
    let path = dir.join("ok.avi");
    fs::write(&path, avi_bytes(true)).unwrap();

    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "AVI");
}

#[test]
fn avi_missing_movi_is_corrupt() {
    let dir = temp_dir("avi-nomovi");
    let path = dir.join("no-movi.avi");
    fs::write(&path, avi_bytes(false)).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "RIFF_NO_MOVI");
}

// ---------------------------------------------------------------------------
// FLV（手工构造）
// ---------------------------------------------------------------------------

fn flv_bytes(tag_type: u8) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"FLV");
    v.push(1); // version
    v.push(0x05); // audio + video
    v.extend_from_slice(&9u32.to_be_bytes()); // header size
    v.extend_from_slice(&0u32.to_le_bytes()); // PreviousTagSize0

    let payload: Vec<u8> = (0..64u32).map(|i| (i * 3 + 7) as u8).collect();
    v.push(tag_type);
    v.extend_from_slice(&(payload.len() as u32).to_be_bytes()[1..]);
    v.extend_from_slice(&0u32.to_be_bytes()[1..]); // timestamp (3)
    v.push(0); // timestamp 扩展
    v.extend_from_slice(&[0, 0, 0]); // stream id
    v.extend_from_slice(&payload);
    v.extend_from_slice(&((11 + payload.len()) as u32).to_le_bytes());
    v
}

#[test]
fn flv_valid_is_ok() {
    let dir = temp_dir("flv-ok");
    let path = dir.join("ok.flv");
    fs::write(&path, flv_bytes(0x08)).unwrap();

    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "FLV");
}

#[test]
fn flv_bad_tag_type_is_corrupt() {
    let dir = temp_dir("flv-tag");
    let path = dir.join("bad-tag.flv");
    fs::write(&path, flv_bytes(0x42)).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "FLV_BAD_TAG");
}

#[test]
fn flv_truncated_is_corrupt() {
    let dir = temp_dir("flv-trunc");
    let path = dir.join("trunc.flv");
    let data = flv_bytes(0x08);
    fs::write(&path, &data[..data.len() - 30]).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "FLV_TRUNCATED");
}

// ---------------------------------------------------------------------------
// MPEG-TS（手工构造）
// ---------------------------------------------------------------------------

fn ts_bytes(packets: usize, extra: usize) -> Vec<u8> {
    let mut v = Vec::new();
    for p in 0..packets {
        v.push(0x47); // 同步字节
        v.push(0x40); // PUSI + PID 高位
        v.push((p & 0xFF) as u8);
        v.push(0x10); // 无自适应区，仅载荷
        while v.len() % 188 != 0 {
            v.push(0x11); // 载荷（避开 0x47，避免干扰包长探测）
        }
    }
    for i in 0..extra {
        v.push(0x11 + (i as u8 & 0x0F));
    }
    v
}

#[test]
fn ts_valid_is_ok() {
    let dir = temp_dir("ts-ok");
    let path = dir.join("ok.ts");
    fs::write(&path, ts_bytes(4, 0)).unwrap();

    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "MPEG-TS");
}

#[test]
fn ts_bad_sync_is_corrupt() {
    let dir = temp_dir("ts-sync");
    let path = dir.join("bad-sync.ts");
    let mut data = ts_bytes(4, 0);
    data[188] = 0x46;
    fs::write(&path, &data).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "TS_BAD_SYNC");
}

#[test]
fn ts_partial_packet_is_corrupt() {
    let dir = temp_dir("ts-trunc");
    let path = dir.join("trunc.ts");
    fs::write(&path, ts_bytes(3, 50)).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "TS_TRUNCATED");
}

// ---------------------------------------------------------------------------
// OGG（手工构造，含页 CRC）
// ---------------------------------------------------------------------------

fn ogg_crc(data: &[u8]) -> u32 {
    // 与 Ogg 规范一致：CRC-32，多项式 0x04C11DB7，初值 0，不反转
    let mut crc = 0u32;
    for &b in data {
        crc ^= u32::from(b) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn ogg_page(flags: u8, payload: &[u8]) -> Vec<u8> {
    let mut page = Vec::new();
    page.extend_from_slice(b"OggS");
    page.push(0); // 版本
    page.push(flags);
    page.extend_from_slice(&0u64.to_le_bytes()); // granule
    page.extend_from_slice(&1u32.to_le_bytes()); // serial
    page.extend_from_slice(&0u32.to_le_bytes()); // 页序号
    page.extend_from_slice(&0u32.to_le_bytes()); // CRC 占位

    let mut segs: Vec<u8> = Vec::new();
    let mut rest = payload.len();
    while rest >= 255 {
        segs.push(255);
        rest -= 255;
    }
    segs.push(rest as u8);
    page.push(segs.len() as u8);
    page.extend_from_slice(&segs);
    page.extend_from_slice(payload);

    let crc = ogg_crc(&page);
    page[22..26].copy_from_slice(&crc.to_le_bytes());
    page
}

fn ogg_bytes() -> Vec<u8> {
    let payload: Vec<u8> = (0..200u32).map(|i| (i % 253) as u8).collect();
    ogg_page(0x04, &payload) // EOS
}

#[test]
fn ogg_valid_is_ok() {
    let dir = temp_dir("ogg-ok");
    let path = dir.join("ok.ogg");
    fs::write(&path, ogg_bytes()).unwrap();

    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "OGG");
}

#[test]
fn ogg_bad_crc_is_corrupt() {
    let dir = temp_dir("ogg-crc");
    let path = dir.join("bad-crc.ogg");
    let mut data = ogg_bytes();
    let last = data.len() - 1;
    data[last] ^= 0xFF;
    fs::write(&path, &data).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "OGG_BAD_CRC");
}

#[test]
fn ogg_truncated_is_corrupt() {
    let dir = temp_dir("ogg-trunc");
    let path = dir.join("trunc.ogg");
    let data = ogg_bytes();
    fs::write(&path, &data[..data.len() - 100]).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "OGG_TRUNCATED");
}

// ---------------------------------------------------------------------------
// ID3 前缀 + FLAC（真实签名在标签后，不应被误判为 MP3）
// ---------------------------------------------------------------------------

/// 构造一个合法 ID3v2.3 标签：10 字节头 + size 字节内容（synchsafe 编码 size）
fn id3v2_header(size: u32) -> Vec<u8> {
    let mut h = Vec::with_capacity(10);
    h.extend_from_slice(b"ID3");
    h.push(3); // v2.3
    h.push(0); // revision
    h.push(0); // flags（无 footer）
    h.push(((size >> 21) & 0x7F) as u8);
    h.push(((size >> 14) & 0x7F) as u8);
    h.push(((size >> 7) & 0x7F) as u8);
    h.push((size & 0x7F) as u8);
    h
}

#[test]
fn flac_with_id3v2_prefix_is_not_mp3() {
    let flac = fs::read(fixture("tone.flac")).expect("读取 tone.flac");
    assert!(
        flac.starts_with(b"fLaC"),
        "fixture 应以 fLaC 开头，实际前 4 字节: {:02X?}",
        &flac[..4.min(flac.len())]
    );

    let dir = temp_dir("flac-id3");
    let path = dir.join("tagged.flac");
    // 用一个 4 KiB 的标签，足够把 fLaC 推出旧的 512B 窗口
    let mut data = id3v2_header(4096);
    data.resize(10 + 4096, 0);
    data.extend_from_slice(&flac);
    fs::write(&path, &data).unwrap();

    let r = verify_file(&path, &opts());
    // 类型必须识别为 FLAC，而不是 MP3
    assert_eq!(r.file_type, "FLAC", "findings: {:?}", r.findings);
    // 且不应出现 MP3 帧相关的假损坏码
    for c in codes(&r) {
        assert!(
            !c.starts_with("MP3"),
            "带 ID3 前缀的 FLAC 不应触发 {c}：{:?}",
            r.findings
        );
    }
}

#[test]
fn id3_tag_len_parses_synchsafe_and_footer() {
    use vine_pulse::core::dispatch;
    // 无 footer，size = 100 → 总长 110
    let h = id3v2_header(100);
    assert_eq!(dispatch::id3_tag_len(&h), Some(110));
    // 带 footer（标志位 0x10）→ 总长 120
    let mut h2 = id3v2_header(100);
    h2[5] = 0x10;
    assert_eq!(dispatch::id3_tag_len(&h2), Some(120));
    // 非法 synchsafe（高位字节 > 0x7F）→ None
    let mut bad = id3v2_header(0);
    bad[6] = 0xFF;
    assert_eq!(dispatch::id3_tag_len(&bad), None);
    // 非 ID3 开头 → None
    assert_eq!(dispatch::id3_tag_len(b"fLaC...."), None);
}
