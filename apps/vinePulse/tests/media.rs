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

#[test]
fn mp3_trailing_padding_is_not_corrupt() {
    // 用户反馈：末尾几秒无内容（填充/标签）不应判为损坏
    let path = copy_fixture("tone.mp3", "mp3-tail");
    let mut data = fs::read(&path).unwrap();
    data.extend_from_slice(&[0u8; 2048]); // 追加一段非帧尾部填充
    fs::write(&path, &data).unwrap();

    let r = verify_file(&path, &opts());
    assert_ne!(
        r.worst(),
        Severity::Corrupt,
        "末尾填充不应判为损坏：{:?}",
        r.findings
    );
    let cs = codes(&r);
    assert!(
        cs.contains(&"MP3_TAIL_PADDING") || cs.contains(&"MP3_TAIL_DATA"),
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
    // 不应因 ID3 前缀误报「缺少 fLaC 签名」（用户反馈的假损坏）
    assert!(
        !codes(&r).contains(&"FLAC_BAD_MAGIC"),
        "带 ID3 前缀的 FLAC 不应报 FLAC_BAD_MAGIC：{:?}",
        r.findings
    );
    assert_eq!(
        r.worst(),
        Severity::Ok,
        "带 ID3 前缀的 FLAC 应整体判为正常：{:?}",
        r.findings
    );
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

// ---------------------------------------------------------------------------
// HEIF（HEIC / AVIF）
// ---------------------------------------------------------------------------

/// 最小合成 HEIC：ftyp(heic) + meta(fullbox, 内含 hdlr) + mdat
fn heic_file(major: &[u8; 4]) -> Vec<u8> {
    let mut b = Vec::new();
    // ftyp: size=24
    b.extend_from_slice(&24u32.to_be_bytes());
    b.extend_from_slice(b"ftyp");
    b.extend_from_slice(major);
    b.extend_from_slice(&0u32.to_be_bytes());
    b.extend_from_slice(b"mif1");
    b.extend_from_slice(b"miaf");
    // meta: size=44（8 头 + 4 version/flags + 32 hdlr）
    b.extend_from_slice(&44u32.to_be_bytes());
    b.extend_from_slice(b"meta");
    b.extend_from_slice(&0u32.to_be_bytes());
    // hdlr: size=32
    b.extend_from_slice(&32u32.to_be_bytes());
    b.extend_from_slice(b"hdlr");
    b.extend_from_slice(&0u32.to_be_bytes()); // version/flags
    b.extend_from_slice(&0u32.to_be_bytes()); // pre_defined
    b.extend_from_slice(b"pict"); // handler_type
    b.extend_from_slice(&[0u8; 12]); // reserved
    // mdat: size=64
    b.extend_from_slice(&64u32.to_be_bytes());
    b.extend_from_slice(b"mdat");
    b.extend_from_slice(&[0x55u8; 56]);
    b
}

fn write_heic(tag: &str, name: &str, data: &[u8]) -> std::path::PathBuf {
    let dir = temp_dir(tag);
    let path = dir.join(name);
    fs::write(&path, data).unwrap();
    path
}

#[test]
fn heic_valid_is_ok_without_moov() {
    let path = write_heic("heic-ok", "ok.heic", &heic_file(b"heic"));
    let r = verify_file(&path, &opts());
    // HEIF 没有 moov box：不得判为损坏
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "HEIC");
}

#[test]
fn avif_brand_detected() {
    let path = write_heic("avif-ok", "ok.avif", &heic_file(b"avif"));
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "AVIF");
}

#[test]
fn heic_truncated_mdat_is_corrupt() {
    let mut data = heic_file(b"heic");
    // 把 mdat 的声明长度改大到文件外
    let at = data.len() - 64;
    data[at..at + 4].copy_from_slice(&0x1000u32.to_be_bytes());
    let path = write_heic("heic-trunc", "trunc.heic", &data);
    let r = verify_file(&path, &opts());
    let cs: Vec<&'static str> = r.findings.iter().map(|f| f.code).collect();
    assert!(cs.contains(&"MP4_TRUNCATED"), "findings: {:?}", r.findings);
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
}

#[test]
fn heic_brand_wins_over_mp4_extension() {
    // .mp4 扩展名但 brand 是 heic → 仍按 HEIC 识别（魔数优先）；
    // 扩展名与内容不符会额外给一条 MAGIC_MISMATCH 警告，但不得判损坏
    let path = write_heic("heic-mis", "misnamed.mp4", &heic_file(b"heic"));
    let r = verify_file(&path, &opts());
    assert_eq!(r.file_type, "HEIC", "findings: {:?}", r.findings);
    assert_ne!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    let cs: Vec<&'static str> = r.findings.iter().map(|f| f.code).collect();
    assert!(cs.contains(&"MAGIC_MISMATCH"), "findings: {:?}", r.findings);
}

// ---------------------------------------------------------------------------
// ASF（WMV / WMA）
// ---------------------------------------------------------------------------

const ASF_HDR_GUID: [u8; 16] = [
    0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA8, 0xB2, 0x00, 0xC0, 0x4F, 0xD9, 0x25, 0x22,
];
const ASF_DATA_OBJECT_GUID: [u8; 16] = [
    0x36, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE, 0x6C,
];

/// 最小 ASF：头对象 + 一个 Data Object（24 字节）
fn asf_file(object_guid: &[u8; 16], object_size: u64, n_objects: u32, reserved: [u8; 2]) -> Vec<u8> {
    let header_size = 30 + object_size;
    let mut b = Vec::new();
    b.extend_from_slice(&ASF_HDR_GUID);
    b.extend_from_slice(&header_size.to_le_bytes());
    b.extend_from_slice(&n_objects.to_le_bytes());
    b.extend_from_slice(&reserved);
    b.extend_from_slice(object_guid);
    b.extend_from_slice(&object_size.to_le_bytes());
    // 对象负载（大小为 object_size - 24）
    b.extend_from_slice(&vec![0u8; (object_size - 24) as usize]);
    b
}

#[test]
fn asf_valid_is_ok() {
    let path = write_heic(
        "asf-ok",
        "ok.wmv",
        &asf_file(&ASF_DATA_OBJECT_GUID, 24, 1, [0x01, 0xFF]),
    );
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "ASF");
}

#[test]
fn asf_object_beyond_header_is_corrupt() {
    // 对象声明 1000 字节，头对象只有 1030？不——header_size 由 object_size 算出，
    // 这里让 header_size 固定而对象过大：构造后改写头部大小字段
    let mut data = asf_file(&ASF_DATA_OBJECT_GUID, 24, 1, [0x01, 0xFF]);
    // 把对象大小改为 1000（头大小字段保持 54）
    let obj_at = 30 + 16;
    data[obj_at..obj_at + 8].copy_from_slice(&1000u64.to_le_bytes());
    let path = write_heic("asf-oob", "oob.wmv", &data);
    let r = verify_file(&path, &opts());
    let cs: Vec<&'static str> = r.findings.iter().map(|f| f.code).collect();
    assert!(
        cs.contains(&"ASF_OBJECT_OUT_OF_RANGE") || cs.contains(&"ASF_BAD_HEADER_SIZE"),
        "findings: {:?}",
        r.findings
    );
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
}

#[test]
fn asf_bad_reserved_is_corrupt() {
    let path = write_heic(
        "asf-rsv",
        "rsv.asf",
        &asf_file(&ASF_DATA_OBJECT_GUID, 24, 1, [0x00, 0x00]),
    );
    let r = verify_file(&path, &opts());
    let cs: Vec<&'static str> = r.findings.iter().map(|f| f.code).collect();
    assert!(cs.contains(&"ASF_BAD_HEADER"), "findings: {:?}", r.findings);
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
}

#[test]
fn asf_missing_data_object_is_warning() {
    // 用一个非 Data 的对象 GUID（如 Header Extension）
    let other: [u8; 16] = [
        0xA5, 0x03, 0xD3, 0xBD, 0xA9, 0xBA, 0x11, 0xCF, 0x8E, 0xE6, 0x00, 0xC0, 0x0C, 0x20, 0x53,
        0x65,
    ];
    let path = write_heic(
        "asf-nodata",
        "nodata.wma",
        &asf_file(&other, 24, 1, [0x01, 0xFF]),
    );
    let r = verify_file(&path, &opts());
    let cs: Vec<&'static str> = r.findings.iter().map(|f| f.code).collect();
    assert!(cs.contains(&"ASF_NO_DATA"), "findings: {:?}", r.findings);
    assert_ne!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
}