use crate::archive::{create_rar, create_zip, ArchiveRequest};
use crate::notes::{note_text, write_note_file, NoteInput};
use crate::project::{resolve_password, ArchiveSettings, Entry, PasswordProfile};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Serialize)]
pub struct ExportProgress {
    /// unit | manifest | finished
    pub phase: String,
    pub index: usize,
    pub total: usize,
    pub rel_path: String,
    pub unit_percent: u8,
    pub global_percent: f64,
    /// running | created | skipped | error | cancelled | finished
    pub status: String,
    pub message: String,
    pub bytes: u64,
    pub created: usize,
    pub skipped: usize,
    pub failed: usize,
    pub archives: Vec<String>,
}

pub struct ExportConfig<'a> {
    pub rar_path: &'a str,
    pub root: &'a Path,
    pub output_root: &'a Path,
    pub settings: &'a ArchiveSettings,
    pub profiles: &'a [PasswordProfile],
    pub entries: &'a [Entry],
}

/// 计算某个条目的输出压缩包完整路径（扩展名随 format）
pub fn archive_path(output_root: &Path, entry: &Entry, password: Option<&str>, format: &str) -> PathBuf {
    let mut dir = output_root.to_path_buf();
    for seg in entry.rel_path.split('/') {
        dir.push(seg);
    }
    let label = match password {
        Some(p) if !p.is_empty() => format!(" [{}]", p),
        _ => String::new(),
    };
    let ext = if format == "zip" { "zip" } else { "rar" };
    dir.join(format!("{}{}.{}", entry.name, label, ext))
}

/// 递归统计单位目录（跳过重连点；`exclude` 为受管说明文件路径，不计入）：
/// (文件数, 总字节, 最新 mtime 毫秒)
fn walk_stats(dir: &Path, exclude: Option<&Path>) -> (u64, u64, u64) {
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut max_mtime = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            if let Some(x) = exclude {
                if e.path() == x {
                    continue;
                }
            }
            let Ok(meta) = e.metadata() else { continue };
            if crate::fsops::is_reparse(&meta) {
                continue;
            }
            if meta.is_dir() {
                stack.push(e.path());
                continue;
            }
            files += 1;
            bytes += meta.len();
            if let Some(d) = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            {
                let ms = d.as_millis() as u64;
                if ms > max_mtime {
                    max_mtime = ms;
                }
            }
        }
    }
    (files, bytes, max_mtime)
}

/// 单位增量指纹：源目录（文件数/总字节/最新 mtime）+ 影响包内容的设置 + 密码 + 说明正文。
/// `note_included` = 说明正文会影响压缩包（comment 注释或 write_notes 写入包内）：
/// 此时受管说明文件 `note_path` 不计入目录统计（其状态由正文哈希覆盖，且导出前后会变化）。
/// 只有清单中记录的指纹与现算一致、且磁盘压缩包 size+mtime 未变时才允许增量跳过。
pub fn unit_fingerprint(
    src_dir: &Path,
    settings: &ArchiveSettings,
    password: Option<&str>,
    note_included: bool,
    note: &str,
    note_path: Option<&Path>,
) -> String {
    let (files, bytes, max_mtime) = walk_stats(src_dir, if note_included { note_path } else { None });
    let payload = format!(
        "format={} vol={} enc={} comment={} notes={} pw={} walk={}/{}/{} note={}",
        settings.format,
        settings.volume_mib,
        u8::from(settings.encrypt_names),
        u8::from(settings.comment),
        u8::from(settings.write_notes),
        password.unwrap_or(""),
        files,
        bytes,
        max_mtime,
        if note_included {
            crate::manifest::hash_str(note)
        } else {
            String::new()
        },
    );
    crate::manifest::hash_str(&payload)
}

pub fn run_export(
    cfg: &ExportConfig,
    emit: &dyn Fn(&ExportProgress),
    cancel: &AtomicBool,
) -> ExportProgress {
    let total = cfg.entries.len();
    let mut created = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let mut bytes: u64 = 0;
    // 本次触及的压缩包（条目 rel, 绝对路径列表, 指纹动作）：
    //   Some(fp) = 创建成功，指纹写为 fp（非增量导出时写空串作废旧指纹）
    //   None     = 压缩包未重建（不覆盖跳过），保留清单中的旧指纹
    let mut touched: Vec<(String, Vec<String>, Option<String>)> = Vec::new();
    // 导出开始时的清单快照：增量跳过判定用（含旧清单无指纹字段的降级）
    let manifest = crate::manifest::load(cfg.output_root);
    let mut last = ExportProgress {
        phase: "finished".into(),
        index: total,
        total,
        rel_path: String::new(),
        unit_percent: 100,
        global_percent: 100.0,
        status: "finished".into(),
        message: String::new(),
        bytes: 0,
        created: 0,
        skipped: 0,
        failed: 0,
        archives: vec![],
    };

    for (i, entry) in cfg.entries.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            last.status = "cancelled".into();
            last.message = "已取消".into();
            emit(&last);
            return last;
        }

        let mut progress = ExportProgress {
            phase: "unit".into(),
            index: i + 1,
            total,
            rel_path: entry.rel_path.clone(),
            unit_percent: 0,
            global_percent: (i as f64 / total.max(1) as f64) * 100.0,
            status: "running".into(),
            message: String::new(),
            bytes,
            created,
            skipped,
            failed,
            archives: vec![],
        };
        emit(&progress);

        let src_dir = join_path(cfg.root, &entry.rel_path);
        let out_dir = join_path(cfg.output_root, &entry.rel_path);

        if let Err(e) = std::fs::create_dir_all(&out_dir) {
            progress.status = "error".into();
            progress.message = format!("创建输出目录失败: {}", e);
            failed += 1;
            progress.failed = failed;
            emit(&progress);
            continue;
        }

        // ZIP 不支持加密与分卷：不带密码标签、不分卷（preflight 会提示）
        let is_zip = cfg.settings.format == "zip";
        let password = if is_zip {
            None
        } else {
            resolve_password(cfg.profiles, entry)
        };
        let out_archive =
            archive_path(cfg.output_root, entry, password.as_deref(), &cfg.settings.format);

        // 说明正文：单次渲染（write_note / 压缩注释 / 增量指纹共用），此步不落盘
        let note_input = NoteInput {
            entry,
            stats: &entry.stats,
            unit_path: &src_dir,
        };
        let note_included = cfg.settings.write_notes || cfg.settings.comment;
        let note_rendered = if note_included {
            note_text(&note_input)
        } else {
            String::new()
        };
        let mut note_target = src_dir.clone();
        note_target.push(format!("{}.txt", entry.name));
        let note_path = note_included.then_some(note_target.as_path());

        // 增量导出：清单指纹一致 + 磁盘压缩包未变 → 跳过（不重写说明、不重打包）
        let fingerprint = if cfg.settings.incremental {
            Some(unit_fingerprint(
                &src_dir,
                cfg.settings,
                password.as_deref(),
                note_included,
                &note_rendered,
                note_path,
            ))
        } else {
            None
        };
        if let Some(fp) = &fingerprint {
            let key = crate::manifest::rel_key(cfg.output_root, &out_archive);
            let rec = manifest
                .as_ref()
                .and_then(|m| m.files.iter().find(|f| f.archive == key));
            if let Some(rec) = rec {
                let disk_ok = std::fs::metadata(&out_archive)
                    .map(|d| {
                        d.len() == rec.size
                            && crate::manifest::mtime_ms(&out_archive) == rec.mtime_ms
                    })
                    .unwrap_or(false);
                if !rec.fingerprint.is_empty() && &rec.fingerprint == fp && disk_ok {
                    skipped += 1;
                    progress.status = "skipped".into();
                    progress.message = "增量跳过（未变动）".into();
                    progress.unit_percent = 100;
                    progress.created = created;
                    progress.skipped = skipped;
                    progress.failed = failed;
                    progress.bytes = bytes;
                    progress.global_percent =
                        ((i + 1) as f64 / total.max(1) as f64) * 100.0;
                    emit(&progress);
                    continue;
                }
            }
        }

        // 说明文件（导出前按需生成，压缩包内即包含最新说明）
        if cfg.settings.write_notes {
            if let Err(e) = write_note_file(&note_input, &note_rendered) {
                progress.status = "error".into();
                progress.message = e;
                failed += 1;
                progress.failed = failed;
                emit(&progress);
                continue;
            }
        }

        // 记入清单的指纹按**写盘后**的最终状态重算（说明文件写入/旧说明清理已生效），
        // 下次跳过判定的「写盘前」计算须与其一致（受管说明文件不计入目录统计）
        let fingerprint_stored = fingerprint.as_ref().map(|_| {
            unit_fingerprint(
                &src_dir,
                cfg.settings,
                password.as_deref(),
                note_included,
                &note_rendered,
                note_path,
            )
        });

        let comment = if cfg.settings.comment {
            Some(note_rendered)
        } else {
            None
        };

        let req = ArchiveRequest {
            rar_path: cfg.rar_path.to_string(),
            src_dir: src_dir.clone(),
            out_archive: out_archive.clone(),
            password,
            encrypt_names: cfg.settings.encrypt_names,
            volume_mib: if is_zip { 0 } else { cfg.settings.volume_mib },
            comment,
            overwrite: cfg.settings.overwrite,
        };

        let mut on_progress = |p: u8| {
            progress.unit_percent = p;
            progress.global_percent =
                ((i as f64 + f64::from(p) / 100.0) / total.max(1) as f64) * 100.0;
            emit(&progress);
        };
        let result = if is_zip {
            create_zip(&req, &mut on_progress, cancel)
        } else {
            create_rar(&req, &mut on_progress, cancel)
        };

        match result {
            Ok(out) => match out.status.as_str() {
                "created" => {
                    created += 1;
                    bytes += out.total_bytes;
                    progress.status = "created".into();
                    progress.unit_percent = 100;
                    touched.push((
                        entry.rel_path.clone(),
                        out.archives.clone(),
                        // 重建的包必须记（或作废）指纹；非增量导出写空串，避免旧指纹被误用
                        Some(fingerprint_stored.clone().unwrap_or_default()),
                    ));
                    progress.archives = out.archives;
                    progress.message = if progress.archives.len() > 1 {
                        format!("{} 个分卷", progress.archives.len())
                    } else {
                        String::new()
                    };
                }
                "skipped" => {
                    skipped += 1;
                    progress.status = "skipped".into();
                    // 包未重建 → 清单指纹原样保留
                    touched.push((entry.rel_path.clone(), out.archives.clone(), None));
                    progress.archives = out.archives;
                    progress.message = out.message;
                }
                "cancelled" => {
                    progress.status = "cancelled".into();
                    progress.message = out.message;
                    progress.created = created;
                    progress.skipped = skipped;
                    progress.failed = failed;
                    progress.bytes = bytes;
                    emit(&progress);
                    last = progress;
                    last.phase = "finished".into();
                    return last;
                }
                _ => {
                    failed += 1;
                    progress.status = "error".into();
                    progress.message = out.message;
                }
            },
            Err(e) => {
                failed += 1;
                progress.status = "error".into();
                progress.message = e;
            }
        }

        progress.created = created;
        progress.skipped = skipped;
        progress.failed = failed;
        progress.bytes = bytes;
        progress.global_percent = ((i + 1) as f64 / total.max(1) as f64) * 100.0;
        emit(&progress);
    }

    // 导出清单：size + mtime + SHA-256 记入 <输出根>/vinepress-manifest.json
    //（仅正常走完循环的路径；中途取消的导出不更新清单，校验时会列为「未入清单」）
    write_manifest(cfg, emit, cancel, &touched);

    last = ExportProgress {
        phase: "finished".into(),
        index: total,
        total,
        rel_path: String::new(),
        unit_percent: 100,
        global_percent: 100.0,
        status: "finished".into(),
        message: format!(
            "完成：新建 {}、跳过 {}、失败 {}",
            created, skipped, failed
        ),
        bytes,
        created,
        skipped,
        failed,
        archives: vec![],
    };
    emit(&last);
    last
}

fn join_path(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    for seg in rel.split('/') {
        if !seg.is_empty() {
            p.push(seg);
        }
    }
    p
}

/// 对本次导出触及的压缩包计算校验和并 upsert 进清单；size + mtime 未变的条目跳过重哈希。
/// 每个文件发一次 `phase = "manifest"` 进度（在最终 finished 事件之前）。
fn write_manifest(
    cfg: &ExportConfig,
    emit: &dyn Fn(&ExportProgress),
    cancel: &AtomicBool,
    touched: &[(String, Vec<String>, Option<String>)],
) {
    use crate::manifest as mf;
    if touched.is_empty() {
        return;
    }
    let mut m = mf::load(cfg.output_root).unwrap_or_default();
    let mut files: Vec<(String, String, Option<String>)> = touched
        .iter()
        .flat_map(|(rel, arcs, fp)| arcs.iter().map(move |a| (rel.clone(), a.clone(), fp.clone())))
        .collect();
    files.sort();
    let total = files.len();
    let mk = |index: usize, rel: String, status: &str, message: String| ExportProgress {
        phase: "manifest".into(),
        index,
        total,
        rel_path: rel,
        unit_percent: if total == 0 { 100 } else { (index * 100 / total) as u8 },
        global_percent: 100.0,
        status: status.into(),
        message,
        bytes: 0,
        created: 0,
        skipped: 0,
        failed: 0,
        archives: vec![],
    };
    let mut updated = 0usize;
    for (i, (rel, abs, fp_action)) in files.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let p = Path::new(abs);
        let archive = mf::rel_key(cfg.output_root, p);
        emit(&mk(i, archive.clone(), "running", format!("计算校验和 {}/{}", i + 1, total)));
        if !p.is_file() {
            continue;
        }
        let size = std::fs::metadata(p).map(|x| x.len()).unwrap_or(0);
        let mtime = mf::mtime_ms(p);
        if mf::unchanged(&m, &archive, size, mtime) {
            continue;
        }
        if let Ok((_, sha)) = mf::hash_file(p) {
            let mut rec = mf::ManifestFile {
                rel_path: rel.clone(),
                archive: archive.clone(),
                size,
                mtime_ms: mtime,
                sha256: sha,
                written_at: mf::now(),
                fingerprint: String::new(),
            };
            match fp_action {
                Some(set) => rec.fingerprint = set.clone(),
                None => {
                    // 包未重建：保留旧指纹（与压缩包内容的对应关系不变）
                    rec.fingerprint = m
                        .files
                        .iter()
                        .find(|x| x.archive == archive)
                        .map(|x| x.fingerprint.clone())
                        .unwrap_or_default();
                }
            }
            mf::upsert(&mut m, rec);
            updated += 1;
        }
    }
    match mf::save(cfg.output_root, &m) {
        Ok(()) => emit(&mk(
            total,
            String::new(),
            "running",
            format!("导出清单已更新（本次变动 {} · 累计 {} 个文件）", updated, m.files.len()),
        )),
        Err(e) => emit(&mk(total, String::new(), "error", e)),
    }
}
