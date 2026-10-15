use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use vine_pulse::{Opts, Report, Severity, verify_file};

static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vine-pulse-full-{}-{}",
        tag,
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn codes(r: &Report) -> Vec<&'static str> {
    r.findings.iter().map(|f| f.code).collect()
}

fn full_opts() -> Opts {
    Opts {
        full: true,
        ..Opts::default()
    }
}

fn write(dir: &Path, name: &str, data: &[u8]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, data).unwrap();
    path
}

fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, data) in entries {
        w.start_file(*name, opts).unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap().into_inner()
}

fn bad_zip() -> Vec<u8> {
    let mut v = b"PK\x03\x04".to_vec();
    v.extend_from_slice(&[0xAB; 60]);
    v
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

// ---------------------------------------------------------------------------
// 归档成员递归（全面模式）
// ---------------------------------------------------------------------------

#[test]
fn zip_nested_bad_member_full_reports_inner() {
    let dir = temp_dir("zip-full");
    let inner = bad_zip();
    let outer = build_zip(&[("bad.bin", &inner), ("note.txt", b"hello")]);
    let path = write(&dir, "outer.zip", &outer);

    let r = verify_file(&path, &full_opts());
    let hit = r
        .findings
        .iter()
        .find(|f| f.message.contains("[bad.bin]"))
        .expect("应有带成员名前缀的内层 finding");
    assert_eq!(hit.severity, Severity::Corrupt, "findings: {:?}", r.findings);
}

#[test]
fn zip_nested_bad_member_default_does_not_recurse() {
    let dir = temp_dir("zip-default");
    let inner = bad_zip();
    let outer = build_zip(&[("bad.bin", &inner)]);
    let path = write(&dir, "outer.zip", &outer);

    let r = verify_file(&path, &Opts::default());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert!(
        !r.findings.iter().any(|f| f.message.contains("[bad.bin]")),
        "默认模式不应递归成员：{:?}",
        r.findings
    );
}

#[test]
fn zip_nested_good_member_full_is_ok() {
    let dir = temp_dir("zip-good");
    let inner = build_zip(&[("x.txt", b"inner content")]);
    let outer = build_zip(&[("good.zip", &inner)]);
    let path = write(&dir, "outer.zip", &outer);

    let r = verify_file(&path, &full_opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
}

fn build_tar(name: &str, data: &[u8]) -> Vec<u8> {
    let mut block = vec![0u8; 512];
    block[..name.len()].copy_from_slice(name.as_bytes());
    block[100..108].copy_from_slice(b"0000644\0");
    block[108..116].copy_from_slice(b"0000000\0");
    block[116..124].copy_from_slice(b"0000000\0");
    let size_oct = format!("{:011o}\0", data.len());
    block[124..136].copy_from_slice(size_oct.as_bytes());
    block[136..148].copy_from_slice(b"00000000000\0");
    block[156] = b'0';
    block[257..263].copy_from_slice(b"ustar\0");
    block[263..265].copy_from_slice(b"00");
    let sum: u64 = block
        .iter()
        .enumerate()
        .map(|(i, &b)| u64::from(if (148..156).contains(&i) { b' ' } else { b }))
        .sum();
    let ck = format!("{sum:06o}\0 ");
    block[148..156].copy_from_slice(ck.as_bytes());

    let mut out = block;
    out.extend_from_slice(data);
    let pad = (512 - data.len() % 512) % 512;
    out.extend(std::iter::repeat_n(0u8, pad));
    out.extend_from_slice(&[0u8; 1024]);
    out
}

#[test]
fn tar_nested_bad_member_full_reports_inner() {
    let dir = temp_dir("tar-full");
    let inner = bad_zip();
    let tar = build_tar("inner.zip", &inner);
    let path = write(&dir, "outer.tar", &tar);

    let r = verify_file(&path, &full_opts());
    let hit = r
        .findings
        .iter()
        .find(|f| f.message.contains("[inner.zip]"))
        .expect("应有带成员名前缀的内层 finding");
    assert_eq!(hit.severity, Severity::Corrupt, "findings: {:?}", r.findings);
}

#[test]
fn tar_nested_bad_member_default_does_not_recurse() {
    let dir = temp_dir("tar-default");
    let inner = bad_zip();
    let tar = build_tar("inner.zip", &inner);
    let path = write(&dir, "outer.tar", &tar);

    let r = verify_file(&path, &Opts::default());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert!(
        !r.findings.iter().any(|f| f.message.contains("[inner.zip]")),
        "默认模式不应递归成员：{:?}",
        r.findings
    );
}

// ---------------------------------------------------------------------------
// PDF FlateDecode 流级解压（全面模式）
// ---------------------------------------------------------------------------

fn build_pdf_with_stream(dict_extra: &str, stream_data: &[u8], length_value: &str) -> Vec<u8> {
    let mut v: Vec<u8> = Vec::new();
    v.extend_from_slice(b"%PDF-1.4\n");
    v.extend_from_slice(b"1 0 obj\n<< /Type /Catalog >>\nendobj\n");
    v.extend_from_slice(b"2 0 obj\n<< ");
    v.extend_from_slice(dict_extra.as_bytes());
    v.extend_from_slice(length_value.as_bytes());
    v.extend_from_slice(b" >>\nstream\n");
    v.extend_from_slice(stream_data);
    v.extend_from_slice(b"\nendstream\nendobj\n");
    let xref_off = v.len();
    v.extend_from_slice(b"xref\n0 3\n");
    v.extend_from_slice(b"0000000000 65535 f \n");
    v.extend_from_slice(b"0000000009 00000 n \n");
    v.extend_from_slice(b"0000000000 00000 n \n");
    v.extend_from_slice(b"trailer\n<< /Size 3 >>\n");
    v.extend_from_slice(format!("startxref\n{xref_off}\n").as_bytes());
    v.extend_from_slice(b"%%EOF\n");
    v
}

#[test]
fn pdf_flate_stream_ok_full() {
    let dir = temp_dir("pdf-stream-ok");
    let payload = zlib(b"hello pdf stream");
    let pdf = build_pdf_with_stream(
        "/Filter /FlateDecode ",
        &payload,
        &format!("/Length {} ", payload.len()),
    );
    let path = write(&dir, "ok.pdf", &pdf);

    let r = verify_file(&path, &full_opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert!(codes(&r).contains(&"PDF_STREAMS_CHECKED"), "{:?}", r.findings);
}

#[test]
fn pdf_flate_stream_corrupt_full_reports() {
    let dir = temp_dir("pdf-stream-bad");
    let pdf = build_pdf_with_stream(
        "/Filter /FlateDecode ",
        b"this is definitely not zlib data",
        &format!("/Length {} ", 32),
    );
    let path = write(&dir, "bad.pdf", &pdf);

    let r = verify_file(&path, &full_opts());
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    assert!(
        codes(&r).contains(&"PDF_STREAM_DECODE_FAILED"),
        "{:?}",
        r.findings
    );
}

#[test]
fn pdf_flate_stream_corrupt_default_skips_stream_check() {
    let dir = temp_dir("pdf-stream-skip");
    let pdf = build_pdf_with_stream(
        "/Filter /FlateDecode ",
        b"this is definitely not zlib data",
        &format!("/Length {} ", 32),
    );
    let path = write(&dir, "bad.pdf", &pdf);

    let r = verify_file(&path, &Opts::default());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert!(!codes(&r).contains(&"PDF_STREAM_DECODE_FAILED"));
}

#[test]
fn pdf_indirect_length_resolves_full() {
    let dir = temp_dir("pdf-len-ref");
    let payload = zlib(b"indirect length payload");
    let mut pdf = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.4\n");
    pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog >>\nendobj\n");
    pdf.extend_from_slice(b"2 0 obj\n<< /Length 3 0 R /Filter /FlateDecode >>\nstream\n");
    pdf.extend_from_slice(&payload);
    pdf.extend_from_slice(b"\nendstream\nendobj\n");
    pdf.extend_from_slice(format!("3 0 obj\n{}\nendobj\n", payload.len()).as_bytes());
    let xref_off = pdf.len();
    pdf.extend_from_slice(b"xref\n0 4\n");
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    pdf.extend_from_slice(b"0000000009 00000 n \n");
    pdf.extend_from_slice(b"0000000000 00000 n \n");
    pdf.extend_from_slice(b"0000000000 00000 n \n");
    pdf.extend_from_slice(b"trailer\n<< /Size 4 >>\n");
    pdf.extend_from_slice(format!("startxref\n{xref_off}\n").as_bytes());
    pdf.extend_from_slice(b"%%EOF\n");

    let path = write(&dir, "ref.pdf", &pdf);
    let r = verify_file(&path, &full_opts());
    assert!(
        codes(&r).contains(&"PDF_STREAMS_CHECKED"),
        "间接 /Length 应可解析：{:?}",
        r.findings
    );
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
}
