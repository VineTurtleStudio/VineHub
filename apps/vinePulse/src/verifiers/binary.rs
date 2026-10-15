//! 可执行文件验证器：ELF（Linux/Unix）与 PE（Windows 可移植可执行）
//!
//! 结构遍历：标识/版本字段、节表（section）与段表（program header）边界、
//! 节数据范围、PE 可选头与数据目录。纯 Rust 顺序读取，不限文件大小。

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use crate::core::dispatch::{MagicKind, expected_by_ext};
use crate::core::{FileHint, Finding, Mode, Opts, Report, Verifier};

pub struct ElfVerifier;
pub struct PeVerifier;

fn u16_at(b: &[u8], off: usize, be: bool) -> Option<u16> {
    let s: [u8; 2] = b.get(off..off + 2)?.try_into().ok()?;
    Some(if be { u16::from_be_bytes(s) } else { u16::from_le_bytes(s) })
}

fn u32_at(b: &[u8], off: usize, be: bool) -> Option<u32> {
    let s: [u8; 4] = b.get(off..off + 4)?.try_into().ok()?;
    Some(if be { u32::from_be_bytes(s) } else { u32::from_le_bytes(s) })
}

fn u64_at(b: &[u8], off: usize, be: bool) -> Option<u64> {
    let s: [u8; 8] = b.get(off..off + 8)?.try_into().ok()?;
    Some(if be { u64::from_be_bytes(s) } else { u64::from_le_bytes(s) })
}

// ---------------------------------------------------------------------------
// ELF
// ---------------------------------------------------------------------------

impl Verifier for ElfVerifier {
    fn supports(&self, hint: &FileHint) -> bool {
        hint.magic == Some(MagicKind::Elf)
            || (hint.magic.is_none() && expected_by_ext(&hint.ext) == Some(MagicKind::Elf))
    }

    fn verify(&self, path: &Path, _opts: &Opts) -> Report {
        let mut report = Report::new(path.to_path_buf(), "ELF", Mode::Blind);
        verify_elf(path, &mut report);
        report
    }
}

fn verify_elf(path: &Path, report: &mut Report) {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            report.push(Finding::unknown(
                "IO_ERROR",
                format!("打开文件失败：{e}"),
                None,
            ));
            return;
        }
    };
    let size = match file.metadata() {
        Ok(m) => m.len(),
        Err(e) => {
            report.push(Finding::unknown(
                "IO_ERROR",
                format!("读取文件大小失败：{e}"),
                None,
            ));
            return;
        }
    };
    let mut r = BufReader::new(file);

    if size < 16 {
        report.push(Finding::corrupt(
            "ELF_TRUNCATED",
            format!("文件仅 {size} 字节，不足 ELF 标识（e_ident）"),
            Some(0),
        ));
        return;
    }
    let mut ident = [0u8; 16];
    if r.read_exact(&mut ident).is_err() {
        report.push(Finding::unknown("IO_ERROR", "读取 ELF 标识失败", Some(0)));
        return;
    }
    if ident[0..4] != [0x7F, b'E', b'L', b'F'] {
        report.push(Finding::corrupt(
            "ELF_BAD_SIG",
            "缺少 ELF 魔数 7F 45 4C 46",
            Some(0),
        ));
        return;
    }
    let class = ident[4]; // 1=32 位, 2=64 位
    let be = match ident[5] {
        1 => false,
        2 => true,
        d => {
            report.push(Finding::corrupt(
                "ELF_BAD_DATA",
                format!("EI_DATA 为 {d}（应为 1=小端 / 2=大端）"),
                Some(5),
            ));
            return;
        }
    };
    if class != 1 && class != 2 {
        report.push(Finding::corrupt(
            "ELF_BAD_CLASS",
            format!("EI_CLASS 为 {class}（应为 1=ELF32 / 2=ELF64）"),
            Some(4),
        ));
        return;
    }
    if ident[6] != 1 {
        report.push(Finding::corrupt(
            "ELF_BAD_VERSION",
            format!("EI_VERSION 为 {}（应为 1）", ident[6]),
            Some(6),
        ));
        return;
    }

    let is64 = class == 2;
    let ehsize: usize = if is64 { 64 } else { 52 };
    let phent_ok: u16 = if is64 { 56 } else { 32 };
    let shent_ok: u16 = if is64 { 64 } else { 40 };
    if size < ehsize as u64 {
        report.push(Finding::corrupt(
            "ELF_TRUNCATED",
            format!("文件仅 {size} 字节，不足 ELF 头（{ehsize} 字节）"),
            Some(0),
        ));
        return;
    }
    let mut hdr = vec![0u8; ehsize];
    if r.seek(SeekFrom::Start(0)).is_err() || r.read_exact(&mut hdr).is_err() {
        report.push(Finding::unknown("IO_ERROR", "读取 ELF 头失败", Some(0)));
        return;
    }

    let v_at = |off: usize| -> Option<u64> {
        if is64 { u64_at(&hdr, off, be) } else { u32_at(&hdr, off, be).map(u64::from) }
    };
    let w_at = |off: usize| u16_at(&hdr, off, be);
    let d_at = |off: usize| u32_at(&hdr, off, be);

    if d_at(20) != Some(1) {
        report.push(Finding::corrupt(
            "ELF_BAD_VERSION",
            "e_version 不为 1",
            Some(20),
        ));
        return;
    }
    let (eh_off, phoff_v, shoff_v) = if is64 { (52usize, 32usize, 40usize) } else { (40, 28, 32) };
    let (phent_v, phnum_v, shent_v, shnum_v, shstr_v) = if is64 {
        (54usize, 56usize, 58usize, 60usize, 62usize)
    } else {
        (42, 44, 46, 48, 50)
    };

    match w_at(eh_off) {
        Some(eh) if u64::from(eh) == ehsize as u64 => {}
        Some(eh) => {
            report.push(Finding::corrupt(
                "ELF_BAD_EHSIZE",
                format!("e_ehsize 为 {eh}，应为 {ehsize}"),
                Some(eh_off as u64),
            ));
            return;
        }
        None => return,
    }

    let phoff = v_at(phoff_v).unwrap_or(0);
    let shoff = v_at(shoff_v).unwrap_or(0);
    let mut phnum = u64::from(w_at(phnum_v).unwrap_or(0));
    let mut shnum = u64::from(w_at(shnum_v).unwrap_or(0));
    let mut shstrndx = u64::from(w_at(shstr_v).unwrap_or(0));
    let phentsize = w_at(phent_v).unwrap_or(0);
    let shentsize = w_at(shent_v).unwrap_or(0);

    // 扩展编号：e_shnum/e_phnum/e_shstrndx 为 0/0xFFFF/0xFFFF 时实际值在节 0
    let need_sec0 =
        (shoff != 0 && (shnum == 0 || shstrndx == 0xFFFF)) || (phoff != 0 && phnum == 0xFFFF);
    let mut sec0 = [0u8; 64];
    if need_sec0 {
        let want = if is64 { 64 } else { 40 };
        let ok = r.seek(SeekFrom::Start(shoff)).is_ok()
            && r.read_exact(&mut sec0[..want]).is_ok();
        if ok {
            if shnum == 0 && shoff != 0 {
                shnum = if is64 {
                    u64_at(&sec0, 32, be).unwrap_or(0)
                } else {
                    u64::from(u32_at(&sec0, 20, be).unwrap_or(0))
                };
            }
            if shstrndx == 0xFFFF {
                shstrndx = u64::from(u32_at(&sec0, if is64 { 52 } else { 40 }, be).unwrap_or(0));
            }
            if phnum == 0xFFFF {
                phnum = u64::from(u32_at(&sec0, if is64 { 56 } else { 44 }, be).unwrap_or(0));
            }
        }
        // 读不到节 0（表本身越界）→ 让下面的表边界检查统一报错
    }

    // 段表（program headers）
    if phnum > 0 {
        if phentsize != phent_ok {
            report.push(Finding::corrupt(
                "ELF_BAD_PHENTSIZE",
                format!("e_phentsize 为 {phentsize}，应为 {phent_ok}"),
                Some(phent_v as u64),
            ));
            return;
        }
        let Some(byte_len) = (phnum).checked_mul(u64::from(phentsize)) else {
            report.push(Finding::corrupt(
                "ELF_PH_TABLE_OUT_OF_RANGE",
                "段表大小溢出",
                Some(phoff),
            ));
            return;
        };
        if phoff.checked_add(byte_len).is_none_or(|end| end > size) {
            report.push(Finding::corrupt(
                "ELF_PH_TABLE_OUT_OF_RANGE",
                format!("段表 {phnum} 项（自偏移 {phoff} 起 {byte_len} 字节）超出文件范围"),
                Some(phoff),
            ));
            return;
        }
        if verify_elf_segments(&mut r, phoff, phnum, is64, be, size, report).is_err() {
            return;
        }
    }

    // 节表（section headers）
    if shnum > 0 {
        if shentsize != shent_ok {
            report.push(Finding::corrupt(
                "ELF_BAD_SHENTSIZE",
                format!("e_shentsize 为 {shentsize}，应为 {shent_ok}"),
                Some(shent_v as u64),
            ));
            return;
        }
        let Some(byte_len) = shnum.checked_mul(u64::from(shentsize)) else {
            report.push(Finding::corrupt(
                "ELF_SH_TABLE_OUT_OF_RANGE",
                "节表大小溢出",
                Some(shoff),
            ));
            return;
        };
        if shoff.checked_add(byte_len).is_none_or(|end| end > size) {
            report.push(Finding::corrupt(
                "ELF_SH_TABLE_OUT_OF_RANGE",
                format!("节表 {shnum} 项（自偏移 {shoff} 起 {byte_len} 字节）超出文件范围"),
                Some(shoff),
            ));
            return;
        }
        if shstrndx != 0 && shstrndx >= shnum {
            report.push(Finding::corrupt(
                "ELF_BAD_SHSTRNDX",
                format!("e_shstrndx 为 {shstrndx}，越界（节表共 {shnum} 项）"),
                Some(shstr_v as u64),
            ));
            return;
        }
        let _ = verify_elf_sections(&mut r, shoff, shnum, is64, be, size, report);
    }
}

/// 遍历段表：p_offset + p_filesz 不得超出文件
fn verify_elf_segments(
    r: &mut BufReader<File>,
    phoff: u64,
    phnum: u64,
    is64: bool,
    be: bool,
    size: u64,
    report: &mut Report,
) -> Result<(), ()> {
    let phentsize: u16 = if is64 { 56 } else { 32 };
    if r.seek(SeekFrom::Start(phoff)).is_err() {
        report.push(Finding::unknown("IO_ERROR", "定位段表失败", Some(phoff)));
        return Err(());
    }
    let ent = phentsize as usize;
    let mut buf = vec![0u8; ent];
    for i in 0..phnum {
        buf.iter_mut().for_each(|b| *b = 0);
        if r.read_exact(&mut buf).is_err() {
            report.push(Finding::unknown("IO_ERROR", "读取段表项失败", Some(phoff)));
            return Err(());
        }
        let (off_field, filesz_field) = if is64 { (8usize, 32usize) } else { (4, 16) };
        let (p_offset, p_filesz) = match (
            v_any(&buf, off_field, is64, be),
            v_any(&buf, filesz_field, is64, be),
        ) {
            (Some(o), Some(f)) => (o, f),
            _ => continue,
        };
        if p_filesz > 0 && p_offset.checked_add(p_filesz).is_none_or(|e| e > size) {
            report.push(Finding::corrupt(
                "ELF_SEGMENT_OUT_OF_RANGE",
                format!(
                    "段 {i}（p_offset={p_offset} p_filesz={p_filesz}）超出文件范围 {size}"
                ),
                Some(phoff + i * u64::from(phentsize)),
            ));
            return Err(());
        }
    }
    Ok(())
}

/// 遍历节表：SHT_NOBITS 之外的节 sh_offset + sh_size 不得超出文件
fn verify_elf_sections(
    r: &mut BufReader<File>,
    shoff: u64,
    shnum: u64,
    is64: bool,
    be: bool,
    size: u64,
    report: &mut Report,
) -> Result<(), ()> {
    let shentsize: u16 = if is64 { 64 } else { 40 };
    if r.seek(SeekFrom::Start(shoff)).is_err() {
        report.push(Finding::unknown("IO_ERROR", "定位节表失败", Some(shoff)));
        return Err(());
    }
    let ent = shentsize as usize;
    let mut buf = vec![0u8; ent];
    for i in 0..shnum {
        buf.iter_mut().for_each(|b| *b = 0);
        if r.read_exact(&mut buf).is_err() {
            report.push(Finding::unknown("IO_ERROR", "读取节表项失败", Some(shoff)));
            return Err(());
        }
        let sh_type = u32_at(&buf, 4, be).unwrap_or(0);
        if sh_type == 8 {
            continue; // SHT_NOBITS：占位不占文件（.bss）
        }
        let (off_field, size_field) = if is64 { (24usize, 32usize) } else { (16, 20) };
        let (sh_offset, sh_size) = match (
            v_any(&buf, off_field, is64, be),
            v_any(&buf, size_field, is64, be),
        ) {
            (Some(o), Some(s)) => (o, s),
            _ => continue,
        };
        if sh_size > 0 && sh_offset.checked_add(sh_size).is_none_or(|e| e > size) {
            report.push(Finding::corrupt(
                "ELF_SECTION_OUT_OF_RANGE",
                format!(
                    "节 {i}（sh_offset={sh_offset} sh_size={sh_size}）超出文件范围 {size}"
                ),
                Some(shoff + i * u64::from(shentsize)),
            ));
            return Err(());
        }
    }
    Ok(())
}

fn v_any(b: &[u8], off: usize, is64: bool, be: bool) -> Option<u64> {
    if is64 { u64_at(b, off, be) } else { u32_at(b, off, be).map(u64::from) }
}

// ---------------------------------------------------------------------------
// PE
// ---------------------------------------------------------------------------

impl Verifier for PeVerifier {
    fn supports(&self, hint: &FileHint) -> bool {
        hint.magic == Some(MagicKind::Pe)
            || (hint.magic.is_none() && expected_by_ext(&hint.ext) == Some(MagicKind::Pe))
    }

    fn verify(&self, path: &Path, _opts: &Opts) -> Report {
        let mut report = Report::new(path.to_path_buf(), "PE", Mode::Blind);
        verify_pe(path, &mut report);
        report
    }
}

fn verify_pe(path: &Path, report: &mut Report) {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            report.push(Finding::unknown("IO_ERROR", format!("打开文件失败：{e}"), None));
            return;
        }
    };
    let size = match file.metadata() {
        Ok(m) => m.len(),
        Err(e) => {
            report.push(Finding::unknown("IO_ERROR", format!("读取文件大小失败：{e}"), None));
            return;
        }
    };
    let mut r = BufReader::new(file);

    if size < 64 {
        report.push(Finding::corrupt(
            "PE_TRUNCATED",
            format!("文件仅 {size} 字节，不足 DOS 头（64 字节）"),
            Some(0),
        ));
        return;
    }
    let mut dos = [0u8; 64];
    if r.read_exact(&mut dos).is_err() {
        report.push(Finding::unknown("IO_ERROR", "读取 DOS 头失败", Some(0)));
        return;
    }
    if &dos[0..2] != b"MZ" {
        report.push(Finding::corrupt(
            "PE_BAD_SIG",
            "缺少 MZ 签名（DOS 头已损坏）",
            Some(0),
        ));
        return;
    }
    let lfanew = u32::from_le_bytes(dos[0x3C..0x40].try_into().unwrap()) as u64;
    if lfanew.checked_add(24).is_none_or(|e| e > size) {
        report.push(Finding::corrupt(
            "PE_BAD_LFANEW",
            format!("e_lfanew 指向 {lfanew}，超出文件范围（PE 头不存在）"),
            Some(0x3C),
        ));
        return;
    }
    let mut coff = [0u8; 24];
    if r.seek(SeekFrom::Start(lfanew)).is_err() || r.read_exact(&mut coff).is_err() {
        report.push(Finding::unknown("IO_ERROR", "读取 PE 头失败", Some(lfanew)));
        return;
    }
    if &coff[0..4] != b"PE\0\0" {
        report.push(Finding::corrupt(
            "PE_BAD_SIG",
            "e_lfanew 处缺少 PE\\0\\0 签名",
            Some(lfanew),
        ));
        return;
    }
    let nsec = u16::from_le_bytes(coff[6..8].try_into().unwrap());
    let opt_size = u16::from_le_bytes(coff[20..22].try_into().unwrap());
    let opt_off = lfanew + 24;

    let mut is_pe32_plus = false;
    if opt_size == 0 {
        report.push(Finding::warning(
            "PE_NO_OPTIONAL_HEADER",
            "COFF 头的 SizeOfOptionalHeader 为 0（非常规映像文件）",
            Some(lfanew + 20),
        ));
    } else {
        if opt_off + u64::from(opt_size) > size {
            report.push(Finding::corrupt(
                "PE_TRUNCATED",
                format!("可选头声明 {opt_size} 字节，超出文件范围"),
                Some(lfanew + 20),
            ));
            return;
        }
        let min_size: u16 = 112; // PE32+ 标准字段下限；PE32 为 96
        if opt_size < 96 {
            report.push(Finding::corrupt(
                "PE_BAD_OPTIONAL_SIZE",
                format!("可选头仅 {opt_size} 字节，短于 PE32 标准下限 96"),
                Some(lfanew + 20),
            ));
            return;
        }
        let mut opt = vec![0u8; (opt_size.min(240)) as usize];
        if r.read_exact(&mut opt).is_err() {
            report.push(Finding::unknown("IO_ERROR", "读取可选头失败", Some(opt_off)));
            return;
        }
        let magic = u16_at(&opt, 0, false).unwrap_or(0);
        match magic {
            0x10B => {}
            0x20B => is_pe32_plus = true,
            m => {
                report.push(Finding::corrupt(
                    "PE_BAD_OPTIONAL_MAGIC",
                    format!("可选头 Magic 为 0x{m:04X}（应为 0x010B PE32 / 0x020B PE32+）"),
                    Some(opt_off),
                ));
                return;
            }
        }
        if opt_size < min_size && is_pe32_plus {
            report.push(Finding::corrupt(
                "PE_BAD_OPTIONAL_SIZE",
                format!("可选头仅 {opt_size} 字节，短于 PE32+ 标准下限 {min_size}"),
                Some(lfanew + 20),
            ));
            return;
        }
        // SizeOfHeaders（偏移 60，两种布局一致）
        if opt.len() >= 64 {
            let soh = u32::from_le_bytes(opt[60..64].try_into().unwrap()) as u64;
            if soh > size {
                report.push(Finding::corrupt(
                    "PE_BAD_HEADERS_SIZE",
                    format!("SizeOfHeaders 声明 {soh} 字节，超出文件范围"),
                    Some(opt_off + 60),
                ));
                return;
            }
        }
        // NumberOfRvaAndSizes（PE32 @96 / PE32+ @108）
        let rva_off = if is_pe32_plus { 108usize } else { 96usize };
        if opt.len() >= rva_off + 4 {
            let n = u32::from_le_bytes(opt[rva_off..rva_off + 4].try_into().unwrap());
            if n > 16 {
                report.push(Finding::warning(
                    "PE_BAD_RVA_COUNT",
                    format!("NumberOfRvaAndSizes 为 {n}（规范上限 16）"),
                    Some(opt_off + rva_off as u64),
                ));
            }
        }
    }

    // 节表
    let table = opt_off + u64::from(opt_size);
    if nsec == 0 {
        report.push(Finding::warning(
            "PE_NO_SECTIONS",
            "节表为空（NumberOfSections = 0）",
            Some(lfanew + 6),
        ));
        return;
    }
    let Some(tbl_len) = u64::from(nsec).checked_mul(40) else {
        report.push(Finding::corrupt(
            "PE_SECTION_TABLE_OUT_OF_RANGE",
            "节表大小溢出",
            Some(table),
        ));
        return;
    };
    if table.checked_add(tbl_len).is_none_or(|e| e > size) {
        report.push(Finding::corrupt(
            "PE_SECTION_TABLE_OUT_OF_RANGE",
            format!("节表 {nsec} 项（自偏移 {table} 起 {tbl_len} 字节）超出文件范围"),
            Some(table),
        ));
        return;
    }
    if r.seek(SeekFrom::Start(table)).is_err() {
        report.push(Finding::unknown("IO_ERROR", "定位节表失败", Some(table)));
        return;
    }
    let mut ent = [0u8; 40];
    for i in 0..nsec {
        ent.iter_mut().for_each(|b| *b = 0);
        if r.read_exact(&mut ent).is_err() {
            report.push(Finding::unknown("IO_ERROR", "读取节表项失败", Some(table)));
            return;
        }
        let raw_size = u32::from_le_bytes(ent[16..20].try_into().unwrap());
        let raw_ptr = u32::from_le_bytes(ent[20..24].try_into().unwrap());
        let at = table + u64::from(i) * 40;
        if raw_size > 0 && raw_ptr > 0 {
            if u64::from(raw_ptr) + u64::from(raw_size) > size {
                report.push(Finding::corrupt(
                    "PE_SECTION_DATA_OUT_OF_RANGE",
                    format!(
                        "节 {i} 数据区（ptr={raw_ptr} size={raw_size}）超出文件范围 {size}"
                    ),
                    Some(at + 20),
                ));
                return;
            }
        } else if raw_size > 0 && raw_ptr == 0 {
            report.push(Finding::corrupt(
                "PE_SECTION_DATA_OUT_OF_RANGE",
                format!("节 {i} 声明数据 {raw_size} 字节但 PointerToRawData 为 0"),
                Some(at + 16),
            ));
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// Mach-O（macOS / iOS 可执行文件）
// ---------------------------------------------------------------------------

pub struct MachoVerifier;

impl Verifier for MachoVerifier {
    fn supports(&self, hint: &FileHint) -> bool {
        hint.magic == Some(MagicKind::Macho)
    }

    fn verify(&self, path: &Path, _opts: &Opts) -> Report {
        let mut report = Report::new(path.to_path_buf(), "Mach-O", Mode::Blind);
        verify_macho(path, &mut report);
        report
    }
}

fn verify_macho(path: &Path, report: &mut Report) {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            report.push(Finding::unknown("IO_ERROR", format!("打开文件失败：{e}"), None));
            return;
        }
    };
    let size = match file.metadata() {
        Ok(m) => m.len(),
        Err(e) => {
            report.push(Finding::unknown("IO_ERROR", format!("读取文件大小失败：{e}"), None));
            return;
        }
    };
    if size < 28 {
        report.push(Finding::corrupt(
            "MACHO_TRUNCATED",
            format!("文件仅 {size} 字节，不足 Mach-O 头（28 字节）"),
            Some(0),
        ));
        return;
    }
    let mut r = BufReader::new(file);
    let mut mag = [0u8; 4];
    if r.read_exact(&mut mag).is_err() {
        report.push(Finding::unknown("IO_ERROR", "读取 Mach-O 魔数失败", Some(0)));
        return;
    }
    let (be, is64) = match mag {
        [0xFE, 0xED, 0xFA, 0xCE] => (true, false),
        [0xFE, 0xED, 0xFA, 0xCF] => (true, true),
        [0xCE, 0xFA, 0xED, 0xFE] => (false, false),
        [0xCF, 0xFA, 0xED, 0xFE] => (false, true),
        _ => {
            report.push(Finding::corrupt(
                "MACHO_BAD_MAGIC",
                format!("魔数 {mag:02x?} 不是 Mach-O 的四种字节序变体"),
                Some(0),
            ));
            return;
        }
    };
    let hdr_size: u64 = if is64 { 32 } else { 28 };
    if size < hdr_size {
        report.push(Finding::corrupt(
            "MACHO_TRUNCATED",
            format!("文件仅 {size} 字节，不足 {} 位 Mach-O 头（{hdr_size} 字节）", if is64 { 64 } else { 32 }),
            Some(0),
        ));
        return;
    }
    let mut hdr = vec![0u8; hdr_size as usize];
    if r.seek(SeekFrom::Start(0)).is_err() || r.read_exact(&mut hdr).is_err() {
        report.push(Finding::unknown("IO_ERROR", "读取 Mach-O 头失败", Some(0)));
        return;
    }
    let u32h = |off: usize| -> u32 {
        let s: [u8; 4] = hdr[off..off + 4].try_into().unwrap();
        if be { u32::from_be_bytes(s) } else { u32::from_le_bytes(s) }
    };
    let ncmds = u32h(16);
    let sizeofcmds = u32h(20);
    if hdr_size + u64::from(sizeofcmds) > size {
        report.push(Finding::corrupt(
            "MACHO_TRUNCATED",
            format!("load commands 声明 {sizeofcmds} 字节，超出文件范围 {size}"),
            Some(20),
        ));
        return;
    }
    if ncmds == 0 {
        report.push(Finding::warning("MACHO_NO_COMMANDS", "ncmds 为 0（没有 load command）", Some(16)));
        return;
    }
    let cmds_end = hdr_size + u64::from(sizeofcmds);
    let mut off = hdr_size;
    for i in 0..ncmds {
        if off + 8 > cmds_end {
            report.push(Finding::corrupt(
                "MACHO_CMDS_OUT_OF_RANGE",
                format!("命令 {i} 处剩余空间不足 8 字节（偏移 {off}）"),
                Some(off),
            ));
            return;
        }
        let mut cb = [0u8; 8];
        if r.seek(SeekFrom::Start(off)).is_err() || r.read_exact(&mut cb).is_err() {
            report.push(Finding::unknown("IO_ERROR", "读取 load command 失败", Some(off)));
            return;
        }
        let cmdsize = if be {
            u32::from_be_bytes(cb[4..8].try_into().unwrap())
        } else {
            u32::from_le_bytes(cb[4..8].try_into().unwrap())
        };
        if cmdsize < 8 {
            report.push(Finding::corrupt(
                "MACHO_BAD_CMD",
                format!("命令 {i} 的 cmdsize 为 {cmdsize}（应 ≥ 8）"),
                Some(off + 4),
            ));
            return;
        }
        if off + u64::from(cmdsize) > cmds_end {
            report.push(Finding::corrupt(
                "MACHO_CMDS_OUT_OF_RANGE",
                format!(
                    "命令 {i}（cmdsize={cmdsize}）超出 load commands 区（结束于 {cmds_end}）"
                ),
                Some(off + 4),
            ));
            return;
        }
        off += u64::from(cmdsize);
    }
    if off < cmds_end {
        report.push(Finding::warning(
            "MACHO_CMDS_TRAILING",
            format!("load commands 区尾部剩余 {} 字节未被命令覆盖", cmds_end - off),
            Some(off),
        ));
    }
}
