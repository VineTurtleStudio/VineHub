use rusqlite::Connection;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use vine_curator_lib::db::repo;
use vine_curator_lib::{db, dupes, scan};

struct Env {
    conn: Connection,
    root: PathBuf,
    root_id: i64,
}

fn setup() -> Env {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "vinecurator_dupe_test_{}_{}",
        std::process::id(),
        n
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("临时目录");
    let mut conn = Connection::open(dir.join("test.sqlite")).expect("打开");
    conn.execute_batch("PRAGMA foreign_keys=ON;")
        .expect("pragma");
    db::migrate::migrate(&mut conn).expect("迁移");
    let root = dir.join("root");
    fs::create_dir_all(&root).expect("根目录");
    let root_id = repo::add_root(&mut conn, &root.to_string_lossy(), "test")
        .expect("加根目录")
        .id;
    Env {
        conn,
        root,
        root_id,
    }
}

fn touch(path: &std::path::Path, contents: &[u8]) {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).expect("目录");
    }
    fs::write(path, contents).expect("写文件");
}

fn scan_all(env: &mut Env) {
    let cancel = AtomicBool::new(false);
    scan::scan_root(&mut env.conn, env.root_id, &env.root, &cancel, &mut |_| {}).expect("扫描");
}

fn roots_map(env: &Env) -> HashMap<i64, PathBuf> {
    repo::list_roots(&env.conn)
        .expect("roots")
        .into_iter()
        .map(|r| (r.id, PathBuf::from(r.path)))
        .collect()
}

#[test]
fn compute_partial_hash_properties() {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "vinecurator_partial_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    let a = dir.join("a.bin");
    let b = dir.join("b.bin");
    let c = dir.join("c.bin");
    touch(&a, b"hello world");
    touch(&b, b"hello world");
    touch(&c, b"HELLO WORLD");

    let ha = dupes::compute_partial_hash(&a, 11).unwrap();
    let hb = dupes::compute_partial_hash(&b, 11).unwrap();
    let hc = dupes::compute_partial_hash(&c, 11).unwrap();
    // 同内容同长 → 同哈希（确定性）
    assert_eq!(ha, hb);
    assert_eq!(ha.len(), 64);
    // 同长不同内容 → 不同
    assert_ne!(ha, hc);
    // 长度必须与实际文件一致（size 过期/失真 → 报错可重试，不写脏数据）
    assert!(dupes::compute_partial_hash(&c, 99).is_err());
    // 空文件拒绝 / 缺失文件报错
    assert!(dupes::compute_partial_hash(&a, 0).is_err());
    assert!(dupes::compute_partial_hash(&dir.join("missing.bin"), 5).is_err());
    // 大于头尾窗口的文件：跳过中段仍稳定
    let big1 = dir.join("big1.bin");
    let big2 = dir.join("big2.bin");
    let mut payload = vec![7u8; 3 * 1024 * 1024 + 123];
    touch(&big1, &payload);
    let h1 = dupes::compute_partial_hash(&big1, payload.len() as i64).unwrap();
    payload[2 * 1024 * 1024] = 9; // 只改中段（在头尾窗口之外）
    touch(&big2, &payload);
    let h2 = dupes::compute_partial_hash(&big2, payload.len() as i64).unwrap();
    assert_eq!(h1, h2, "中段窗口外的差异不影响部分哈希（设计取舍）");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn pending_requires_size_groups_and_skips_missing_or_done() {
    let mut env = setup();
    touch(&env.root.join("a.bin"), b"XX");
    touch(&env.root.join("b.bin"), b"YY");
    touch(&env.root.join("c.bin"), b"ZZZ");
    touch(&env.root.join("d.bin"), b"QQQQ");
    scan_all(&mut env);

    let pending = dupes::pending_partial(&env.conn).unwrap();
    // 只有 size=2 的 a/b（c、d 各自唯一）
    assert_eq!(pending.len(), 2);
    assert!(pending.iter().all(|p| p.size == 2));

    // 已算过的不再入列
    for p in &pending {
        dupes::store_partial(&env.conn, p.id, "aaaa").unwrap();
    }
    assert!(dupes::pending_partial(&env.conn).unwrap().is_empty());

    // missing=1 整组排除（先把 a 置缺失，b 单独不成组）
    env.conn
        .execute(
            "UPDATE items SET missing = 1 WHERE rel_path LIKE '%a.bin'",
            [],
        )
        .unwrap();
    dupes::store_partial(&env.conn, pending[1].id, "").unwrap_or(());
    env.conn
        .execute("UPDATE items SET partial_hash = NULL", [])
        .unwrap();
    assert!(dupes::pending_partial(&env.conn).unwrap().is_empty());
}

#[test]
fn report_splits_by_full_hash_and_confirms() {
    let mut env = setup();
    touch(&env.root.join("a.bin"), b"XX");
    touch(&env.root.join("b.bin"), b"YY");
    touch(&env.root.join("c.bin"), b"YY");
    scan_all(&mut env);

    // 补部分哈希（同 size 同内容长度，内容不同 → a 与 b/c 可能不同桶）
    let roots = roots_map(&env);
    let cancel = AtomicBool::new(false);
    let mut emits = Vec::new();
    {
        let mut emit = |p: &dupes::DupeProgress| emits.push(p.clone());
        dupes::run_partial_hashes(&env.conn, &roots, &cancel, &mut emit).unwrap();
    }
    // 最后一条是 finished
    assert_eq!(emits.last().unwrap().status, "done");
    assert_eq!(emits.last().unwrap().hashed, 3);

    // b、c 同内容 → 同部分哈希同完整哈希；a 内容不同 → 不同部分哈希
    env.conn
        .execute(
            "UPDATE items SET hash_state = 'done', hash = 'h_bc' WHERE rel_path LIKE '%b.bin' OR rel_path LIKE '%c.bin'",
            [],
        )
        .unwrap();
    let groups = dupes::report(&env.conn).unwrap();
    assert_eq!(groups.len(), 1);
    assert!(groups[0].confirmed);
    assert_eq!(groups[0].count, 2);
    assert_eq!(groups[0].full_hash.as_deref(), Some("h_bc"));

    // 完整哈希分桶不同 → 撞车剔除（b 与 c 改成不同完整哈希）
    env.conn
        .execute(
            "UPDATE items SET hash = 'h_b' WHERE rel_path LIKE '%b.bin'",
            [],
        )
        .unwrap();
    env.conn
        .execute(
            "UPDATE items SET hash = 'h_c' WHERE rel_path LIKE '%c.bin'",
            [],
        )
        .unwrap();
    assert!(dupes::report(&env.conn).unwrap().is_empty());

    // 清掉完整哈希 → 未确认组（size + 部分哈希相同）
    env.conn
        .execute("UPDATE items SET hash = NULL, hash_state = 'none'", [])
        .unwrap();
    let groups = dupes::report(&env.conn).unwrap();
    assert_eq!(groups.len(), 1);
    assert!(!groups[0].confirmed);
    assert_eq!(groups[0].count, 2);
}

#[test]
fn run_is_idempotent_and_cancel_writes_nothing() {
    let mut env = setup();
    touch(&env.root.join("a.bin"), b"XX");
    touch(&env.root.join("b.bin"), b"YY");
    scan_all(&mut env);
    let roots = roots_map(&env);

    // 取消先行 → 零写入
    let cancel = AtomicBool::new(true);
    let mut emits = Vec::new();
    {
        let mut emit = |p: &dupes::DupeProgress| emits.push(p.clone());
        dupes::run_partial_hashes(&env.conn, &roots, &cancel, &mut emit).unwrap();
    }
    assert_eq!(emits.last().unwrap().status, "cancelled");
    let n: i64 = env
        .conn
        .query_row(
            "SELECT COUNT(*) FROM items WHERE partial_hash IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 0);

    // 正常跑 → 全部写入
    let cancel = AtomicBool::new(false);
    let mut emits = Vec::new();
    {
        let mut emit = |p: &dupes::DupeProgress| emits.push(p.clone());
        dupes::run_partial_hashes(&env.conn, &roots, &cancel, &mut emit).unwrap();
    }
    let fin = emits.last().unwrap();
    assert_eq!(fin.status, "done");
    assert_eq!(fin.hashed, 2);
    assert_eq!(fin.failed, 0);

    // 幂等：候选为空，二次运行 hashed=0
    let mut emits = Vec::new();
    {
        let mut emit = |p: &dupes::DupeProgress| emits.push(p.clone());
        dupes::run_partial_hashes(&env.conn, &roots, &cancel, &mut emit).unwrap();
    }
    assert_eq!(emits.last().unwrap().hashed, 0);
}
