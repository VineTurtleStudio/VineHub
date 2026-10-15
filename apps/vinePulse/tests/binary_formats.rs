use std::fs;
use std::path::PathBuf;

use vine_pulse::{Opts, Report, Severity, verify_file};

static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vine-pulse-binary-{}-{}",
        tag,
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_case(tag: &str, name: &str, data: &[u8]) -> PathBuf {
    let path = temp_dir(tag).join(name);
    fs::write(&path, data).unwrap();
    path
}

fn opts() -> Opts {
    Opts::default()
}

fn codes(r: &Report) -> Vec<&'static str> {
    r.findings.iter().map(|f| f.code).collect()
}

fn assert_corrupt(r: &Report, code: &str) {
    assert_eq!(r.worst(), Severity::Corrupt, "findings: {:?}", r.findings);
    assert!(codes(r).contains(&code), "findings: {:?}", r.findings);
}

// --- ELF ---

/// 最小 ELF64 头（64 字节，无段无节）
fn elf64_header() -> Vec<u8> {
    let mut b = vec![0u8; 64];
    b[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
    b[4] = 2; // ELFCLASS64
    b[5] = 1; // ELFDATA2LSB
    b[6] = 1; // EV_CURRENT
    b[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    b[18..20].copy_from_slice(&62u16.to_le_bytes()); // EM_X86_64
    b[20..24].copy_from_slice(&1u32.to_le_bytes()); // e_version
    b[32..40].copy_from_slice(&0u64.to_le_bytes()); // e_phoff
    b[40..48].copy_from_slice(&0u64.to_le_bytes()); // e_shoff
    b[52..54].copy_from_slice(&64u16.to_le_bytes()); // e_ehsize
    b[54..56].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize
    b[56..58].copy_from_slice(&0u16.to_le_bytes()); // e_phnum
    b[58..60].copy_from_slice(&64u16.to_le_bytes()); // e_shentsize
    b[60..62].copy_from_slice(&0u16.to_le_bytes()); // e_shnum
    b[62..64].copy_from_slice(&0u16.to_le_bytes()); // e_shstrndx
    b
}

#[test]
fn elf_minimal_header_is_ok() {
    let path = write_case("elf-ok", "ok.elf", &elf64_header());
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "ELF");
}

#[test]
fn elf_bad_class_is_corrupt() {
    let mut b = elf64_header();
    b[4] = 5; // 非法 EI_CLASS
    let path = write_case("elf-class", "bad-class.elf", &b);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "ELF_BAD_CLASS");
}

#[test]
fn elf_segment_out_of_range_is_corrupt() {
    // phoff=64, phnum=1（表 56 字节，共 120 字节），段数据声明到 150
    let mut b = elf64_header();
    b[32..40].copy_from_slice(&64u64.to_le_bytes()); // e_phoff
    b[56..58].copy_from_slice(&1u16.to_le_bytes()); // e_phnum
    b.extend_from_slice(&[0u8; 56]);
    // 段项：p_type@0=1(LOAD), p_offset@8=100, p_filesz@32=50 → 100+50=150 > 120
    let base = 64usize;
    b[base + 8..base + 16].copy_from_slice(&100u64.to_le_bytes());
    b[base + 32..base + 40].copy_from_slice(&50u64.to_le_bytes());
    let path = write_case("elf-seg", "seg-oob.elf", &b);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "ELF_SEGMENT_OUT_OF_RANGE");
}

#[test]
fn elf_section_table_beyond_file_is_corrupt() {
    // shoff 指向文件外：节表 1 项 × 64 字节超出文件
    let mut b = elf64_header();
    b[40..48].copy_from_slice(&64u64.to_le_bytes()); // e_shoff
    b[60..62].copy_from_slice(&1u16.to_le_bytes()); // e_shnum
    let path = write_case("elf-sht", "sht-oob.elf", &b);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "ELF_SH_TABLE_OUT_OF_RANGE");
}

#[test]
fn elf_real_binary_has_no_elf_structure_errors() {
    // 真实 Windows 可执行文件不应被误判（file_type 为 PE，此用例守 ELF 不抢跑）
    let exe = std::env::current_exe().unwrap();
    let r = verify_file(&exe, &opts());
    let cs = codes(&r);
    assert!(
        !cs.iter().any(|c| c.starts_with("ELF_")),
        "不应出现 ELF 误报：{:?}",
        r.findings
    );
}

// --- PE ---

/// 最小 PE32+ 映像：DOS 头 + PE 签名 + COFF + 240 字节可选头 + 1 节
fn pe64_image() -> Vec<u8> {
    let lfanew = 0x80usize;
    let opt_size = 240usize;
    let table = lfanew + 24 + opt_size; // 节表
    let mut b = vec![0u8; table + 40 + 64];
    b[0..2].copy_from_slice(b"MZ");
    b[0x3C..0x40].copy_from_slice(&(lfanew as u32).to_le_bytes());
    // PE 签名 + COFF
    b[lfanew..lfanew + 4].copy_from_slice(b"PE\0\0");
    b[lfanew + 4..lfanew + 6].copy_from_slice(&0x8664u16.to_le_bytes()); // Machine
    b[lfanew + 6..lfanew + 8].copy_from_slice(&1u16.to_le_bytes()); // NumberOfSections
    b[lfanew + 20..lfanew + 22].copy_from_slice(&(opt_size as u16).to_le_bytes());
    // 可选头
    let o = lfanew + 24;
    b[o..o + 2].copy_from_slice(&0x20Bu16.to_le_bytes()); // PE32+
    b[o + 56..o + 60].copy_from_slice(&0x1000u32.to_le_bytes()); // SizeOfImage
    b[o + 60..o + 64].copy_from_slice(&448u32.to_le_bytes()); // SizeOfHeaders（≤ 文件大小）
    b[o + 108..o + 112].copy_from_slice(&16u32.to_le_bytes()); // NumberOfRvaAndSizes
    // 节表：数据区 ptr=table+40, size=64（文件尾部 64 字节）
    let s = table;
    b[s..s + 8].copy_from_slice(b".text\0\0\0");
    b[s + 8..s + 12].copy_from_slice(&64u32.to_le_bytes()); // VirtualSize
    b[s + 12..s + 16].copy_from_slice(&0x1000u32.to_le_bytes()); // VirtualAddress
    b[s + 16..s + 20].copy_from_slice(&64u32.to_le_bytes()); // SizeOfRawData
    b[s + 20..s + 24].copy_from_slice(&((table + 40) as u32).to_le_bytes()); // PointerToRawData
    b
}

#[test]
fn pe64_valid_is_ok() {
    let path = write_case("pe-ok", "ok.exe", &pe64_image());
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "PE");
}

#[test]
fn pe_missing_signature_is_corrupt() {
    let mut b = pe64_image();
    let lfanew = 0x80usize;
    b[lfanew..lfanew + 4].copy_from_slice(b"NE\0\0"); // 老式签名
    let path = write_case("pe-sig", "bad-sig.exe", &b);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "PE_BAD_SIG");
}

#[test]
fn pe_section_data_beyond_file_is_corrupt() {
    let mut b = pe64_image();
    // 把节的数据区声明推出文件末尾
    let lfanew = 0x80usize;
    let s = lfanew + 24 + 240;
    b[s + 16..s + 20].copy_from_slice(&0x1000u32.to_le_bytes()); // SizeOfRawData
    b[s + 20..s + 24].copy_from_slice(&0x4000u32.to_le_bytes()); // PointerToRawData
    let path = write_case("pe-sec", "sec-oob.exe", &b);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "PE_SECTION_DATA_OUT_OF_RANGE");
}

#[test]
fn pe_bad_lfanew_is_corrupt() {
    let mut b = pe64_image();
    b[0x3C..0x40].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes()); // 指向文件外
    let path = write_case("pe-lf", "bad-lfanew.exe", &b);
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "PE_BAD_LFANEW");
}

#[test]
fn real_vine_pulse_exe_is_not_flagged() {
    // 本项目自身构建出的 exe：真实 PE，结构遍历不应误报
    let exe = std::env::current_dir()
        .unwrap()
        .join("target")
        .join("debug")
        .join("vine-pulse.exe");
    if !exe.exists() {
        eprintln!("跳过：{} 尚未构建", exe.display());
        return;
    }
    let r = verify_file(&exe, &opts());
    let cs = codes(&r);
    assert!(
        !cs.iter().any(|c| c.starts_with("PE_")),
        "真实 exe 不应有结构误报：{:?}",
        r.findings
    );
}

// --- Mach-O ---

/// 64 位小端 Mach-O（MH_MAGIC_64 = CF FA ED FE）
fn macho64(ncmds: u32, sizeofcmds: u32, first_cmdsize: u32) -> Vec<u8> {
    let mut b = vec![0u8; 32];
    b[0..4].copy_from_slice(&[0xCF, 0xFA, 0xED, 0xFE]);
    b[4..8].copy_from_slice(&0x0100_0007u32.to_le_bytes()); // CPU_TYPE_X86_64
    b[12..16].copy_from_slice(&2u32.to_le_bytes()); // MH_EXECUTE
    b[16..20].copy_from_slice(&ncmds.to_le_bytes());
    b[20..24].copy_from_slice(&sizeofcmds.to_le_bytes());
    let mut lc = vec![0u8; (first_cmdsize.max(8)) as usize];
    lc[0..4].copy_from_slice(&0x19u32.to_le_bytes()); // LC_SEGMENT_64
    lc[4..8].copy_from_slice(&first_cmdsize.to_le_bytes());
    b.extend_from_slice(&lc);
    b
}

#[test]
fn macho64_valid_is_ok() {
    let path = write_case("macho-ok", "ok.bin", &macho64(1, 16, 16));
    let r = verify_file(&path, &opts());
    assert_eq!(r.worst(), Severity::Ok, "findings: {:?}", r.findings);
    assert_eq!(r.file_type, "Mach-O");
}

#[test]
fn macho_bad_cmdsize_is_corrupt() {
    let path = write_case("macho-cmd", "badcmd.bin", &macho64(1, 8, 4));
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "MACHO_BAD_CMD");
}

#[test]
fn macho_cmds_beyond_file_is_corrupt() {
    // ncmds=2 但只放得下一条命令
    let path = write_case("macho-oob", "oob.bin", &macho64(2, 16, 16));
    let r = verify_file(&path, &opts());
    assert_corrupt(&r, "MACHO_CMDS_OUT_OF_RANGE");
}

#[test]
fn macho_truncated_header_is_corrupt() {
    let mut b = macho64(1, 16, 16);
    b.truncate(10); // 保留魔数但不足头
    let path = write_case("macho-trunc", "trunc.bin", &b);
    let r = verify_file(&path, &opts());
    // 10 字节 → detect 命中（4 字节魔数），verify 判不足 28 字节头
    assert_corrupt(&r, "MACHO_TRUNCATED");
}
