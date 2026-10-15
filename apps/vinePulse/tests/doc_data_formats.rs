use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use vine_pulse::{Opts, Report, Severity, verify_file};

static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vine-pulse-docdata-{}-{}",
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

fn opts() -> Opts {
    Opts::default()
}

fn assert_corrupt(r: &Report, code: &str) {
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    assert!(codes(r).contains(&code), "findings: {:?}", r.findings);
}

fn assert_warning(r: &Report, code: &str) {
    assert_ne!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    assert!(codes(r).contains(&code), "findings: {:?}", r.findings);
}

fn write(dir: &Path, name: &str, data: &[u8]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, data).unwrap();
    path
}

// ---------------------------------------------------------------------------
// PDF
// ---------------------------------------------------------------------------

fn build_pdf() -> Vec<u8> {
    let mut v: Vec<u8> = Vec::new();
    v.extend_from_slice(b"%PDF-1.4\n");
    v.extend_from_slice(b"1 0 obj\n<< /Type /Catalog >>\nendobj\n");
    let xref_off = v.len();
    v.extend_from_slice(b"xref\n0 2\n");
    v.extend_from_slice(b"0000000000 65535 f \n");
    v.extend_from_slice(b"0000000009 00000 n \n");
    v.extend_from_slice(b"trailer\n<< /Size 2 >>\n");
    v.extend_from_slice(format!("startxref\n{xref_off}\n").as_bytes());
    v.extend_from_slice(b"%%EOF\n");
    v
}

#[test]
fn pdf_valid_is_ok() {
    let dir = temp_dir("pdf-ok");
    let path = write(&dir, "ok.pdf", &build_pdf());
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "PDF");
}

#[test]
fn pdf_missing_eof_is_corrupt() {
    let dir = temp_dir("pdf-noeof");
    let pdf = build_pdf();
    let cut = pdf.windows(5).position(|w| w == b"%%EOF").unwrap();
    let path = write(&dir, "noeof.pdf", &pdf[..cut]);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "PDF_NO_EOF");
}

#[test]
fn pdf_bad_xref_offset_is_corrupt() {
    let dir = temp_dir("pdf-xref");
    let mut pdf = build_pdf();
    let pos = pdf.windows(4).position(|w| w == b"xref").unwrap();
    pdf[pos..pos + 4].copy_from_slice(b"XXXX");
    let path = write(&dir, "bad_xref.pdf", &pdf);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "PDF_BAD_XREF");
}

// ---------------------------------------------------------------------------
// RTF
// ---------------------------------------------------------------------------

#[test]
fn rtf_valid_is_ok() {
    let dir = temp_dir("rtf-ok");
    let path = write(&dir, "ok.rtf", b"{\\rtf1\\ansi Hello}");
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "RTF");
}

#[test]
fn rtf_missing_end_brace_is_corrupt() {
    let dir = temp_dir("rtf-noend");
    let path = write(&dir, "noend.rtf", b"{\\rtf1\\ansi Hello");
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "RTF_NO_END");
}

#[test]
fn rtf_bad_header_by_ext_is_corrupt() {
    let dir = temp_dir("rtf-badhdr");
    let path = write(&dir, "fake.rtf", b"this is not rtf at all");
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "RTF_BAD_HEADER");
}

// ---------------------------------------------------------------------------
// 容器（DOCX/EPUB/ODT）
// ---------------------------------------------------------------------------

fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored);
    for (name, data) in entries {
        w.start_file(*name, opts).unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap().into_inner()
}

#[test]
fn docx_valid_is_ok() {
    let dir = temp_dir("docx-ok");
    let data = build_zip(&[
        ("[Content_Types].xml", b"<Types/>"),
        ("word/document.xml", b"<doc/>"),
    ]);
    let path = write(&dir, "ok.docx", &data);
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "ZIP"); // 魔数识别优先展示
}

#[test]
fn docx_missing_content_types_is_corrupt() {
    let dir = temp_dir("docx-noct");
    let data = build_zip(&[("word/document.xml", b"<doc/>")]);
    let path = write(&dir, "noct.docx", &data);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "DOC_MISSING_ENTRY");
}

#[test]
fn docx_truncated_is_corrupt() {
    let dir = temp_dir("docx-trunc");
    let data = build_zip(&[
        ("[Content_Types].xml", b"<Types/>"),
        ("word/document.xml", b"<doc/>"),
    ]);
    let path = write(&dir, "trunc.docx", &data[..data.len() / 2]);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "ZIP_NO_EOCD");
}

#[test]
fn epub_valid_is_ok() {
    let dir = temp_dir("epub-ok");
    let data = build_zip(&[
        ("mimetype", b"application/epub+zip"),
        ("META-INF/container.xml", b"<container/>"),
    ]);
    let path = write(&dir, "ok.epub", &data);
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
}

#[test]
fn epub_missing_container_is_corrupt() {
    let dir = temp_dir("epub-nocont");
    let data = build_zip(&[("mimetype", b"application/epub+zip")]);
    let path = write(&dir, "nocont.epub", &data);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "DOC_MISSING_ENTRY");
}

#[test]
fn epub_bad_mimetype_is_corrupt() {
    let dir = temp_dir("epub-mime");
    let data = build_zip(&[
        ("mimetype", b"application/zip"),
        ("META-INF/container.xml", b"<container/>"),
    ]);
    let path = write(&dir, "badmime.epub", &data);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "DOC_BAD_MIMETYPE");
}

#[test]
fn odt_valid_is_ok() {
    let dir = temp_dir("odt-ok");
    let data = build_zip(&[
        ("mimetype", b"application/vnd.oasis.opendocument.text"),
        ("META-INF/manifest.xml", b"<manifest/>"),
    ]);
    let path = write(&dir, "ok.odt", &data);
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
}

// ---------------------------------------------------------------------------
// TXT
// ---------------------------------------------------------------------------

#[test]
fn txt_utf8_is_ok() {
    let dir = temp_dir("txt-utf8");
    let path = write(&dir, "ok.txt", "你好，world\n第二行\n".as_bytes());
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
}

#[test]
fn txt_utf16_bom_is_ok() {
    let dir = temp_dir("txt-utf16");
    let mut data = vec![0xFF, 0xFE];
    for unit in "hi text".encode_utf16() {
        data.extend_from_slice(&unit.to_le_bytes());
    }
    let path = write(&dir, "utf16.txt", &data);
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
}

#[test]
fn txt_bad_utf8_with_bom_is_corrupt() {
    let dir = temp_dir("txt-badbom");
    let mut data = vec![0xEF, 0xBB, 0xBF];
    data.extend_from_slice(b"abc \xFF\xFE broken");
    let path = write(&dir, "badbom.txt", &data);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "TEXT_BAD_UTF8");
}

#[test]
fn txt_nul_byte_is_warning() {
    let dir = temp_dir("txt-nul");
    let path = write(&dir, "has_nul.txt", b"hello\0world"); // 不能叫 nul.txt：Windows 保留设备名
    let r = verify_file(&path, &opts());
    assert_warning(&r, "TEXT_HAS_NUL");
}

// ---------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------

#[test]
fn csv_valid_quoted_is_ok() {
    let dir = temp_dir("csv-ok");
    let path = write(
        &dir,
        "ok.csv",
        b"name,age\n\"a,b\",3\nc,\"d\ne\"\n",
    );
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "text"); // 文本类扩展名由通用逻辑展示
}

#[test]
fn csv_ragged_row_is_corrupt() {
    let dir = temp_dir("csv-ragged");
    let path = write(&dir, "ragged.csv", b"a,b,c\n1,2\n");
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "CSV_RAGGED_ROW");
}

#[test]
fn csv_unclosed_quote_is_corrupt() {
    let dir = temp_dir("csv-quote");
    let path = write(&dir, "quote.csv", b"a,b\n\"x,1\n");
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "CSV_UNCLOSED_QUOTE");
}

#[test]
fn csv_bad_utf8_bom_is_corrupt() {
    let dir = temp_dir("csv-badbom");
    let mut data = vec![0xEF, 0xBB, 0xBF];
    data.extend_from_slice(b"a,b\n\xff,2\n");
    let path = write(&dir, "badbom.csv", &data);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "TEXT_BAD_UTF8");
}

// ---------------------------------------------------------------------------
// SQLite
// ---------------------------------------------------------------------------

fn make_db(tag: &str) -> PathBuf {
    let dir = temp_dir(tag);
    let path = dir.join("ok.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE t(a TEXT); INSERT INTO t VALUES('x');")
        .unwrap();
    conn.close().unwrap();
    path
}

#[test]
fn sqlite_valid_is_ok() {
    let path = make_db("sqlite-ok");
    let r = verify_file(&path, &opts());
    // 新建的小库多为空页，可能触发低熵提示；关键是没有任何 SQLITE_* 问题
    assert_ne!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    assert!(
        codes(&r).iter().all(|c| !c.starts_with("SQLITE")),
        "findings: {:?}",
        r.findings
    );
    assert_eq!(r.file_type, "SQLite");
}

#[test]
fn sqlite_tiny_file_is_corrupt() {
    let dir = temp_dir("sqlite-tiny");
    let path = write(&dir, "tiny.db", b"SQLITE");
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "SQLITE_TRUNCATED");
}

#[test]
fn sqlite_tampered_magic_is_corrupt() {
    let path = make_db("sqlite-magic");
    let mut data = fs::read(&path).unwrap();
    data[0] = b'X'; // "SQLite format 3\0" -> "XQLite format 3\0"
    fs::write(&path, &data).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "SQLITE_BAD_MAGIC");
}

#[test]
fn sqlite_cut_page_is_corrupt() {
    let path = make_db("sqlite-cut");
    let data = fs::read(&path).unwrap();
    fs::write(&path, &data[..100]).unwrap();

    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "SQLITE_QUICK_CHECK");
}

// ---------------------------------------------------------------------------
// Parquet
// ---------------------------------------------------------------------------

fn build_parquet(footer: &[u8]) -> Vec<u8> {
    let mut v = b"PAR1".to_vec();
    v.extend_from_slice(&[0u8; 64]); // 伪数据区
    v.extend_from_slice(footer);
    v.extend_from_slice(&(footer.len() as u32).to_le_bytes());
    v.extend_from_slice(b"PAR1");
    v
}

#[test]
fn parquet_valid_is_ok() {
    let dir = temp_dir("parquet-ok");
    let path = write(&dir, "ok.parquet", &build_parquet(b"FAKE"));
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "Parquet");
}

#[test]
fn parquet_truncated_is_corrupt() {
    let dir = temp_dir("parquet-trunc");
    let data = build_parquet(b"FAKE");
    let path = write(&dir, "trunc.parquet", &data[..data.len() - 4]);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "PARQUET_BAD_MAGIC");
}

#[test]
fn parquet_bad_footer_len_is_corrupt() {
    let dir = temp_dir("parquet-footer");
    let mut data = build_parquet(b"FAKE");
    let n = data.len();
    data[n - 8..n - 4].copy_from_slice(&u32::MAX.to_le_bytes());
    let path = write(&dir, "badfooter.parquet", &data);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "PARQUET_BAD_FOOTER");
}
