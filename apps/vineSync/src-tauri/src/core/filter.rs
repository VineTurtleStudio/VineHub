use globset::{Glob, GlobSet, GlobSetBuilder};
use std::path::Path;

use super::E;

/// 包含/排除过滤器。
/// - exclude 同时作用于文件与目录（目录被排除则整棵子树剪枝）
/// - include 仅作用于文件；为空表示全部包含
/// - 模式同时按「完整相对路径」与「文件/目录名」匹配：
///   `node_modules/**` 既能剪掉根级 node_modules，也能剪掉任意层级的 node_modules
pub struct Filter {
    include_path: GlobSet,
    include_name: GlobSet,
    exclude_path: GlobSet,
    exclude_name: GlobSet,
    has_include: bool,
}

fn compile(patterns: &[String], builder: &mut GlobSetBuilder, for_name: bool) -> E<()> {
    for raw in patterns {
        let p = raw.trim();
        if p.is_empty() {
            continue;
        }
        add_pattern(builder, p, for_name)?;
        // `xxx/**` 同时匹配 xxx 目录本身
        if let Some(prefix) = p.strip_suffix("/**") {
            add_pattern(builder, prefix, for_name)?;
        }
    }
    Ok(())
}

fn add_pattern(builder: &mut GlobSetBuilder, p: &str, for_name: bool) -> E<()> {
    if for_name && p.contains('/') {
        // 名称匹配只保留不含路径分隔符的部分
        return Ok(());
    }
    let glob = Glob::new(p)
        .map_err(|e| crate::msg!("过滤器表达式无效 \"{p}\": {e}", "Invalid filter pattern \"{p}\": {e}"))?;
    builder.add(glob);
    Ok(())
}

fn build(patterns: &[String], for_name: bool) -> E<GlobSet> {
    let mut b = GlobSetBuilder::new();
    compile(patterns, &mut b, for_name)?;
    b.build().map_err(|e| crate::msg!("过滤器构建失败: {e}", "Filter build failed: {e}"))
}

fn basename(rel: &str) -> &str {
    rel.rsplit(['/', '\\']).next().unwrap_or(rel)
}

impl Filter {
    pub fn new(include: &[String], exclude: &[String]) -> E<Self> {
        let include: Vec<String> = include
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let has_include = !include.is_empty();
        Ok(Self {
            include_path: build(&include, false)?,
            include_name: build(&include, true)?,
            exclude_path: build(exclude, false)?,
            exclude_name: build(exclude, true)?,
            has_include,
        })
    }

    /// 校验过滤器是否合法
    pub fn validate(include: &[String], exclude: &[String]) -> E<()> {
        Self::new(include, exclude).map(|_| ())
    }

    pub fn allow_dir(&self, rel: &str) -> bool {
        let norm = rel.replace('\\', "/");
        !self.exclude_path.is_match(&norm) && !self.exclude_name.is_match(basename(&norm))
    }

    pub fn allow_file(&self, rel: &str) -> bool {
        let norm = rel.replace('\\', "/");
        if !self.allow_dir(&norm) {
            return false;
        }
        if !self.has_include {
            return true;
        }
        self.include_path.is_match(&norm) || self.include_name.is_match(basename(&norm))
    }
}

/// 判断相对路径的第一段是否为保留目录名：
/// 「版本备份」（程序自己的版本库）及 Windows 系统目录（$RECYCLE.BIN、
/// System Volume Information，大小写不敏感），这些目录从不参与同步
pub fn is_reserved_root(rel: &str) -> bool {
    let norm = rel.replace('\\', "/");
    match norm.split('/').next() {
        Some(first) if first == "版本备份" => true,
        Some(first) => {
            let lower = first.to_ascii_lowercase();
            lower == "$recycle.bin" || lower == "system volume information"
        }
        None => false,
    }
}

pub fn rel_of(root: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(root)
        .ok()
        .map(|p| p.to_string_lossy().replace('/', "\\"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        copy, diff, normalize_key, normalize_path_str, scan, version, ChangeKind,
        DeletePolicy, FileMeta,
    };
    use std::collections::{BTreeMap, HashSet};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "vinesync_test_{}_{}",
            tag,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(path: &Path, content: &str) {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn set_mtime(path: &Path, ago: Duration) {
        let f = fs::File::options().write(true).open(path).unwrap();
        f.set_modified(SystemTime::now() - ago).unwrap();
    }

    fn meta(rel: &str, size: u64, secs: i64) -> FileMeta {
        FileMeta { rel: rel.to_string(), size, mtime_ns: secs * 1_000_000_000 }
    }

    // ---------- 过滤器 ----------

    #[test]
    fn filter_include_and_exclude() {
        let f = Filter::new(
            &["*.docx".into()],
            &["*.tmp".into(), "node_modules/**".into()],
        )
        .unwrap();
        assert!(f.allow_file("a.docx"));
        assert!(f.allow_file("sub\\b.docx"));
        assert!(!f.allow_file("a.txt"));
        assert!(!f.allow_file("x.tmp"));
        assert!(!f.allow_file("sub\\y.tmp"));
        assert!(!f.allow_dir("node_modules"));
        assert!(!f.allow_dir("src\\node_modules"));
        assert!(f.allow_dir("sub"));
        assert!(f.allow_dir("项目A"));
    }

    #[test]
    fn filter_without_include_allows_all() {
        let f = Filter::new(&[], &[]).unwrap();
        assert!(f.allow_file("任意\\路径.txt"));
        assert!(f.allow_dir("任意"));
    }

    #[test]
    fn filter_bad_pattern_is_error() {
        assert!(Filter::new(&["[".into()], &[]).is_err());
    }

    #[test]
    fn reserved_version_dir() {
        assert!(is_reserved_root("版本备份"));
        assert!(is_reserved_root("版本备份\\子目录\\a.txt"));
        assert!(!is_reserved_root("文档\\版本备份"));
        assert!(is_reserved_root("$RECYCLE.BIN"));
        assert!(is_reserved_root("$recycle.bin\\S-1-5-18\\x.txt"));
        assert!(is_reserved_root("System Volume Information"));
        assert!(is_reserved_root("system volume information\\a.txt"));
        assert!(!is_reserved_root("文档\\$RECYCLE.BIN"));
    }

    #[test]
    fn prune_under_removes_covered_keys() {
        let mut map: BTreeMap<String, FileMeta> = [
            ("locked/a.txt", meta("locked/a.txt", 1, 1)),
            ("locked2/b.txt", meta("locked2/b.txt", 1, 1)),
            ("x.txt", meta("x.txt", 1, 1)),
            ("locked_extra/c.txt", meta("locked_extra/c.txt", 1, 1)),
        ]
        .iter()
        .map(|(k, v)| (normalize_key(k), v.clone()))
        .collect();
        scan::prune_under(&mut map, &["locked".into(), "x.txt".into()]);
        assert_eq!(map.len(), 2);
        assert!(map.contains_key("locked2/b.txt"));
        assert!(map.contains_key("locked_extra/c.txt"));
        assert!(!map.contains_key("locked/a.txt"));
        assert!(!map.contains_key("x.txt"));
    }

    // ---------- 扫描 ----------

    #[test]
    fn scan_skips_reserved_and_temp() {
        let root = tmp("scan");
        write(&root.join("a.txt"), "aaa");
        write(&root.join("sub\\b.txt"), "bbb");
        write(&root.join("版本备份\\a_2026-01-01_00-00-00.txt"), "old");
        write(&root.join("$RECYCLE.BIN\\S-1-5-18\\junk.txt"), "junk");
        write(&root.join("System Volume Information\\IndexerVolumeGuid"), "svi");
        write(&root.join("stale.vinesync.tmp"), "junk");

        let filter = Filter::new(&[], &[]).unwrap();
        let out = scan::scan_with_progress(&root, &filter, true, |_| {}).unwrap();
        assert_eq!(out.files.len(), 2);
        assert!(out.files.contains_key("a.txt"));
        assert!(out.files.contains_key("sub/b.txt"));
        assert!(!root.join("stale.vinesync.tmp").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_skips_denied_subtree() {
        let root = tmp("scan_denied");
        write(&root.join("ok.txt"), "x");
        let locked = root.join("locked");
        write(&locked.join("hidden.txt"), "y");

        // 拒绝当前用户读取 locked，模拟权限拒绝场景
        let user = std::env::var("USERNAME").unwrap_or_else(|_| "Everyone".into());
        let domain = std::env::var("USERDOMAIN").unwrap_or_default();
        let who = if domain.is_empty() { user.clone() } else { format!("{domain}\\{user}") };
        let denied = std::process::Command::new("icacls")
            .args([locked.to_str().unwrap(), "/deny", &format!("{who}:(OI)(CI)F")])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !denied {
            let _ = fs::remove_dir_all(&root);
            return; // 环境不支持则跳过
        }

        let filter = Filter::new(&[], &[]).unwrap();
        let out = scan::scan_with_progress(&root, &filter, false, |_| {}).unwrap();
        assert!(out.files.contains_key("ok.txt"));
        assert!(!out.files.contains_key("locked/hidden.txt"));
        assert!(out.skipped.iter().any(|s| s == "locked"));

        let _ = std::process::Command::new("icacls")
            .args([locked.to_str().unwrap(), "/remove:d", &who])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let _ = fs::remove_dir_all(&root);
    }

    /// 实机回归：扫真实回收站目录（内部 S-1-5-18 等 ACL 拒绝访问，
    /// 曾导致整个扫描中断报 os error 5）。需 `VS_LIVE_SCAN=1` 且 F: 存在。
    #[test]
    fn scan_live_recycle_bin() {
        if std::env::var("VS_LIVE_SCAN").is_err() {
            return;
        }
        let root = PathBuf::from("F:\\$RECYCLE.BIN");
        if !root.is_dir() {
            return;
        }
        let filter = Filter::new(&[], &[]).unwrap();
        let out = scan::scan_with_progress(&root, &filter, false, |_| {})
            .expect("real recycle bin scan must not fail");
        // 被跳过的 ACL 拒绝子树（如 S-1-5-18）不得出现在文件清单中
        if let Some(s) = out.skipped.iter().find(|s| s.contains("S-1-5-18")) {
            assert!(!out.files.keys().any(|k| k == s || k.starts_with(&format!("{s}/"))));
        }
    }

    /// 目录联接（junction）/符号链接专项：一律剪枝、不跟随、不进清单。
    /// 风险：junction 带 DIRECTORY 属性，若被当普通目录下降，会把外部目录树
    /// （乃至循环联接 → 无限递归）纳入备份计划。
    #[test]
    fn scan_prunes_junction_and_symlink() {
        let base = tmp("reparse");
        let root = base.join("root");
        let foreign = base.join("foreign");
        write(&root.join("real\\a.txt"), "aaa");
        write(&foreign.join("secret.txt"), "sss");

        let jlink = root.join("jlink");
        let j = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J", jlink.to_str().unwrap(), foreign.to_str().unwrap()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !j {
            let _ = fs::remove_dir_all(&base);
            return; // 环境不支持创建联接（极少见）
        }

        // 快速探针：max_depth 限制下若发现联接自身以外的后代即失败，
        // 先于正式扫描（否则循环联接会导致死循环，测试挂起）
        let descends = walkdir::WalkDir::new(&root)
            .max_depth(5)
            .into_iter()
            .filter_map(|e| e.ok())
            .any(|e| e.path() != jlink.as_path() && e.path().starts_with(&jlink));
        assert!(!descends, "目录联接不得被跟随下降（会纳入外部目录树/循环死递归）");

        // 探针确认不跟随后，再加环形联接做正式扫描（若回归则在此暴露）
        let loop_link = foreign.join("loop");
        let _ = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J", loop_link.to_str().unwrap(), root.to_str().unwrap()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        // 符号链接需开发者模式/管理员，创建失败则跳过其断言
        let slink = root.join("slink");
        let sl = std::os::windows::fs::symlink_dir(&foreign, &slink).is_ok();

        let filter = Filter::new(&[], &[]).unwrap();
        let out = scan::scan_with_progress(&root, &filter, false, |_| {}).unwrap();
        assert!(out.files.contains_key("real/a.txt"));
        assert!(
            !out.files.keys().any(|k| k == "jlink" || k.starts_with("jlink/")),
            "联接内容不得进入清单: {:?}",
            out.files.keys().collect::<Vec<_>>()
        );
        if sl {
            assert!(
                !out.files.keys().any(|k| k == "slink" || k.starts_with("slink/")),
                "符号链接内容不得进入清单"
            );
        }
        assert!(!out.files.keys().any(|k| k.contains("secret")), "外部目录内容不得被纳入");

        // 显式先移除联接点再删目录，避免 remove_dir_all 对联接的边界行为
        let _ = fs::remove_dir(&loop_link);
        let _ = fs::remove_dir(&jlink);
        if sl {
            let _ = fs::remove_dir(&slink);
        }
        let _ = fs::remove_dir_all(&base);
    }

    /// 硬链接专项：按独立普通文件对待（各自出现在清单中，互不影响）。
    #[test]
    fn scan_hardlink_is_regular_file() {
        let root = tmp("hardlink");
        write(&root.join("a.txt"), "same");
        if fs::hard_link(root.join("a.txt"), root.join("b.txt")).is_err() {
            let _ = fs::remove_dir_all(&root);
            return;
        }
        let filter = Filter::new(&[], &[]).unwrap();
        let out = scan::scan_with_progress(&root, &filter, false, |_| {}).unwrap();
        assert!(out.files.contains_key("a.txt"));
        assert!(out.files.contains_key("b.txt"), "硬链接应作为独立普通文件进入清单");
        assert_eq!(out.files["a.txt"].size, out.files["b.txt"].size);
        let _ = fs::remove_dir_all(&root);
    }

    /// 超长路径（>260，MAX_PATH）专项：verbatim 根下扫描与复制均可用。
    #[test]
    fn scan_and_copy_long_paths_over_max_path() {
        fn deep_under(base: &Path) -> PathBuf {
            let mut d = base.to_path_buf();
            let mut i = 0;
            while d.to_string_lossy().len() < 300 && i < 8 {
                d = d.join(format!("lvl{i}_{}", "p".repeat(90)));
                i += 1;
            }
            d
        }

        let base = tmp("longpath");
        let deep = deep_under(&base);
        assert!(deep.to_string_lossy().len() > 260, "构造路径必须超过 260: {}", deep.display());

        let root_v = crate::core::to_long_path(&base);
        let deep_v = crate::core::to_long_path(&deep);
        fs::create_dir_all(&deep_v).expect("verbatim 下应能创建深层目录");
        fs::write(deep_v.join("deep.txt"), "deep").unwrap();

        let filter = Filter::new(&[], &[]).unwrap();
        let out = scan::scan_with_progress(&root_v, &filter, false, |_| {}).unwrap();
        assert!(
            out.files.keys().any(|k| k.ends_with("/deep.txt")),
            "超长路径文件必须能扫出: {:?}",
            out.files.keys().collect::<Vec<_>>()
        );

        // 复制到同样超长的目标
        let base2 = tmp("longpath_dst");
        let deep2_v = crate::core::to_long_path(&deep_under(&base2));
        fs::create_dir_all(&deep2_v).unwrap();
        let src_file = deep_v.join("deep.txt");
        let dst_file = deep2_v.join("deep.txt");
        let mtime_ns =
            crate::core::systemtime_to_ns(fs::metadata(&src_file).unwrap().modified().unwrap());
        copy::safe_copy(&src_file, &dst_file, mtime_ns).unwrap();
        assert_eq!(fs::read_to_string(&dst_file).unwrap(), "deep");

        let _ = fs::remove_dir_all(crate::core::to_long_path(&base));
        let _ = fs::remove_dir_all(crate::core::to_long_path(&base2));
    }

    // ---------- 对比 ----------

    #[test]
    fn diff_three_policies() {
        let src: BTreeMap<String, FileMeta> = [
            ("a.txt", meta("a.txt", 3, 100)),  // 新增
            ("b.txt", meta("b.txt", 9, 100)),  // 内容变化
            ("c.txt", meta("c.txt", 3, 100)),  // 一致
        ]
        .iter()
        .map(|(k, v)| (normalize_key(k), v.clone()))
        .collect();
        let dst: BTreeMap<String, FileMeta> = [
            ("b.txt", meta("b.txt", 3, 50)),
            ("c.txt", meta("c.txt", 3, 100)),
            ("d.txt", meta("d.txt", 4, 50)), // 源已删除
        ]
        .iter()
        .map(|(k, v)| (normalize_key(k), v.clone()))
        .collect();

        let none: HashSet<String> = HashSet::new();

        let m = diff(&src, &dst, DeletePolicy::Mirror, true, &none);
        assert_eq!(m.changes.len(), 3);
        assert!(m.changes.iter().any(|c| c.kind == ChangeKind::Add && c.rel == "a.txt"));
        assert!(m.changes.iter().any(|c| c.kind == ChangeKind::Update && c.rel == "b.txt"));
        assert!(m.changes.iter().any(|c| c.kind == ChangeKind::Delete && c.rel == "d.txt"));
        assert_eq!(m.suppressed, 0);

        let v = diff(&src, &dst, DeletePolicy::Versioned, true, &none);
        assert_eq!(v.changes.len(), 3);
        assert!(v.changes.iter().any(|c| c.kind == ChangeKind::Versioned && c.rel == "d.txt"));

        // 关闭「删除时备份」：策略 A 的源删除退化为直接删除
        let v2 = diff(&src, &dst, DeletePolicy::Versioned, false, &none);
        assert!(v2.changes.iter().any(|c| c.kind == ChangeKind::Delete && c.rel == "d.txt"));
        assert!(!v2.changes.iter().any(|c| c.kind == ChangeKind::Versioned));

        let a = diff(&src, &dst, DeletePolicy::AddOnly, true, &none);
        assert_eq!(a.changes.len(), 2);
        assert!(!a.changes.iter().any(|c| c.kind == ChangeKind::Delete));
        assert_eq!(a.suppressed, 1);
    }

    // ---------- 复制 ----------

    #[test]
    fn safe_copy_content_and_mtime() {
        let dir = tmp("copy");
        let src = dir.join("src.txt");
        let dst = dir.join("nest\\dst.txt");
        write(&src, "hello world");
        set_mtime(&src, Duration::from_secs(5000));

        let mtime_ns = crate::core::systemtime_to_ns(
            fs::metadata(&src).unwrap().modified().unwrap(),
        );
        copy::safe_copy(&src, &dst, mtime_ns).unwrap();
        assert_eq!(fs::read_to_string(&dst).unwrap(), "hello world");
        let dst_mtime = crate::core::systemtime_to_ns(
            fs::metadata(&dst).unwrap().modified().unwrap(),
        );
        assert_eq!(dst_mtime, mtime_ns);

        // 覆盖已有文件
        write(&src, "updated content!");
        copy::safe_copy(&src, &dst, mtime_ns).unwrap();
        assert_eq!(fs::read_to_string(&dst).unwrap(), "updated content!");
        assert!(!dir.join("nest\\dst.txt.vinesync.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn safe_copy_hook_versions_old_only_after_staged() {
        let dir = tmp("copy_hook");
        let src = dir.join("src.txt");
        let dst = dir.join("dst.txt");
        write(&src, "new content");
        write(&dst, "old content");
        let mtime_ns = 1_000_000_000;

        // 钩子成功：旧内容先被移入版本目录，新内容原子覆盖
        let ver_dir = dir.join("版本备份");
        let r = copy::safe_copy_hook(&src, &dst, mtime_ns, || {
            std::fs::create_dir_all(&ver_dir)
                .map_err(|e| crate::msg!("mkdir: {e}", "mkdir: {e}"))?;
            std::fs::rename(&dst, ver_dir.join("old.txt"))
                .map_err(|e| crate::msg!("mv: {e}", "mv: {e}"))?;
            Ok(())
        });
        assert!(r.is_ok());
        assert_eq!(fs::read_to_string(&dst).unwrap(), "new content");
        assert_eq!(fs::read_to_string(ver_dir.join("old.txt")).unwrap(), "old content");
        assert!(!dir.join("dst.txt.vinesync.tmp").exists());

        // 钩子失败：目标原样保留，暂存文件被清理
        write(&src, "newer");
        write(&dst, "current");
        let r2 = copy::safe_copy_hook(&src, &dst, mtime_ns, || {
            Err(crate::msg!("hook failed", "hook failed"))
        });
        assert!(r2.is_err());
        assert_eq!(fs::read_to_string(&dst).unwrap(), "current");
        assert!(!dir.join("dst.txt.vinesync.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    // ---------- 版本备份 ----------

    #[test]
    fn version_move_and_cleanup() {
        let dest = tmp("version");
        write(&dest.join("报告\\old.docx"), "history");

        let rel = version::move_to_version(&dest, "报告\\old.docx", 0, "%Y-%m-%d_%H-%M-%S").unwrap();
        assert!(rel.starts_with("版本备份\\报告\\old_20"));
        assert!(!dest.join("报告\\old.docx").exists());
        assert!(dest.join(&rel).exists());

        // 修改时间为 100 天前 → 按 30 天保留应被清理
        let backed = dest.join(&rel);
        set_mtime(&backed, Duration::from_secs(100 * 24 * 3600));
        let removed = version::cleanup(&dest, 30).unwrap();
        assert_eq!(removed, 1);
        assert!(!dest.join("版本备份").exists());
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn version_collision_gets_suffix() {
        let dest = tmp("version2");
        write(&dest.join("a.txt"), "one");
        let r1 = version::move_to_version(&dest, "a.txt", 0, "%Y-%m-%d_%H-%M-%S").unwrap();
        write(&dest.join("a.txt"), "two");
        let r2 = version::move_to_version(&dest, "a.txt", 0, "%Y-%m-%d_%H-%M-%S").unwrap();
        assert_ne!(r1, r2);
        assert!(dest.join(&r1).exists());
        assert!(dest.join(&r2).exists());
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn version_max_count_trims_oldest() {
        let dest = tmp("version3");
        // 连续三次覆盖备份，max_count=2 → 最旧的一份被裁掉
        write(&dest.join("a.txt"), "v1");
        let r1 = version::move_to_version(&dest, "a.txt", 2, "%Y-%m-%d_%H-%M-%S").unwrap();
        write(&dest.join("a.txt"), "v2");
        let r2 = version::move_to_version(&dest, "a.txt", 2, "%Y-%m-%d_%H-%M-%S").unwrap();
        write(&dest.join("a.txt"), "v3");
        let r3 = version::move_to_version(&dest, "a.txt", 2, "%Y-%m-%d_%H-%M-%S").unwrap();

        assert!(dest.join(&r3).exists());
        assert!(dest.join(&r2).exists());
        assert!(!dest.join(&r1).exists(), "最旧的一份应被份数上限裁掉");

        // 0 = 不限份数
        write(&dest.join("a.txt"), "v4");
        let _ = version::move_to_version(&dest, "a.txt", 0, "%Y-%m-%d_%H-%M-%S").unwrap();
        write(&dest.join("a.txt"), "v5");
        let r5 = version::move_to_version(&dest, "a.txt", 0, "%Y-%m-%d_%H-%M-%S").unwrap();
        assert!(dest.join(&r5).exists());
        let ver_dir = dest.join("版本备份");
        let count = fs::read_dir(&ver_dir).unwrap().count();
        assert_eq!(count, 4, "关闭份数上限后应保留全部历史");
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn version_readonly_backup_gets_fresh_mtime() {
        let dest = tmp("version4");
        let f = dest.join("ro.txt");
        write(&f, "readonly content");
        // 模拟 100 天前的只读文件：若备份后 mtime 不刷新，会被按天清理立刻误删
        set_mtime(&f, Duration::from_secs(100 * 24 * 3600));
        fs::set_permissions(&f, {
            let mut perms = fs::metadata(&f).unwrap().permissions();
            perms.set_readonly(true);
            perms
        })
        .unwrap();

        let rel = version::move_to_version(&dest, "ro.txt", 0, "%Y-%m-%d_%H-%M-%S").unwrap();
        let mtime = crate::core::systemtime_to_ns(fs::metadata(dest.join(&rel)).unwrap().modified().unwrap());
        let now = crate::core::systemtime_to_ns(std::time::SystemTime::now());
        assert!(
            now - mtime < 3_600_000_000_000, // 1 小时内
            "备份文件 mtime 应为当前时间而非源文件旧时间"
        );
        let _ = fs::set_permissions(dest.join(&rel), {
            let mut p = fs::metadata(dest.join(&rel)).unwrap().permissions();
            p.set_readonly(false);
            p
        });
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn version_custom_timestamp_format() {
        let dest = tmp("version5");
        write(&dest.join("a.txt"), "x");
        let rel = version::move_to_version(&dest, "a.txt", 0, "%Y%m%d_%H%M%S").unwrap();
        let name = std::path::Path::new(&rel)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap()
            .to_string();
        // 形如 a_20261009_020000：数字时间戳、无文件名非法字符
        let ts = name.strip_prefix("a_").expect("前缀");
        assert!(ts.starts_with("20"), "ts: {ts}");
        assert!(!name.contains(':') && !name.contains('/'), "name: {name}");

        // 含非法字符的格式：冒号等被替换为 -
        write(&dest.join("a.txt"), "y");
        let rel2 = version::move_to_version(&dest, "a.txt", 0, "%H:%M:%S").unwrap();
        let name2 = std::path::Path::new(&rel2).file_name().unwrap().to_str().unwrap();
        assert!(!name2.contains(':'), "name2: {name2}");
        let _ = fs::remove_dir_all(&dest);
    }

    // ---------- 工具函数 ----------

    #[test]
    fn normalize_key_case_and_separator() {
        assert_eq!(normalize_key("A\\B\\C.TXT"), "a/b/c.txt");
        assert_eq!(normalize_key("a/b/"), "a/b");
    }

    #[test]
    fn normalize_path_str_collapses_slashes() {
        assert_eq!(normalize_path_str("F:/备份//文档/"), "f:\\备份\\文档");
    }
}
