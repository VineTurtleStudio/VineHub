use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use vine_curator_lib::db::repo;
use vine_curator_lib::{db, fts, ocr, scan};

struct Env {
    conn: Connection,
    root: PathBuf,
    root_id: i64,
}

fn setup() -> Env {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("vinecurator_ocr_test_{}_{}", std::process::id(), n));
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

fn touch(path: &Path, contents: &[u8]) {
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

fn id_of(env: &Env, like: &str) -> i64 {
    env.conn
        .query_row(
            "SELECT id FROM items WHERE rel_path LIKE ?1",
            rusqlite::params![like],
            |r| r.get(0),
        )
        .expect("item id")
}

fn ocr_status(env: &Env, like: &str) -> String {
    env.conn
        .query_row(
            "SELECT b.ocr_status FROM book_items b JOIN items i ON i.id = b.item_id
             WHERE i.rel_path LIKE ?1",
            rusqlite::params![like],
            |r| r.get(0),
        )
        .expect("ocr_status")
}

/// 假 runner：输入名含 fail 则报错；否则写派生 pdf + sidecar 文本
struct FakeRunner {
    fail_on: HashSet<String>,
}

impl ocr::OcrRunner for FakeRunner {
    fn ocr(&self, input: &Path, out_pdf: &Path, sidecar: &Path, _lang: &str) -> Result<(), String> {
        let name = input
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if self.fail_on.contains(&name) {
            return Err(format!("fake 失败: {name}"));
        }
        fs::write(out_pdf, b"PDFCOPY").map_err(|e| e.to_string())?;
        fs::write(sidecar, format!("OCRTEXT {name}")).map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn run_queue(
    env: &mut Env,
    runner: &dyn ocr::OcrRunner,
    cancel: &AtomicBool,
    derived: &Path,
) -> Vec<ocr::OcrProgress> {
    let roots = roots_map(env);
    let mut emits = Vec::new();
    {
        let mut emit = |p: &ocr::OcrProgress| emits.push(p.clone());
        ocr::run_ocr_queue(
            &env.conn, &roots, derived, runner, "eng", None, cancel, &mut emit,
        )
        .expect("run queue");
    }
    emits
}

#[test]
fn pending_filters_pdf_books_and_states() {
    let mut env = setup();
    touch(&env.root.join("a.pdf"), b"%PDF a");
    touch(&env.root.join("b.pdf"), b"%PDF b");
    touch(&env.root.join("c.pdf"), b"%PDF c");
    touch(&env.root.join("fail.pdf"), b"%PDF d");
    touch(&env.root.join("e.pdf"), b"%PDF e");
    touch(&env.root.join("note.epub"), b"epub");
    touch(&env.root.join("img.png"), b"png");
    scan_all(&mut env);

    // 初始：全部 pdf 书 none → 5 个候选（epub/png 不算）
    let pending = ocr::pending_ocr(&env.conn, None).expect("pending");
    assert_eq!(pending.len(), 5);

    // done + ocr_mtime 失配 → 仍待跑；done + 新鲜 → 排除
    let a = id_of(&env, "%a.pdf");
    env.conn
        .execute(
            "UPDATE book_items SET ocr_status='done', ocr_mtime = (SELECT mtime FROM items WHERE id = ?1) WHERE item_id = ?1",
            rusqlite::params![a],
        )
        .unwrap();
    let b = id_of(&env, "%b.pdf");
    env.conn
        .execute(
            "UPDATE book_items SET ocr_status='done', ocr_mtime = 1 WHERE item_id = ?1",
            rusqlite::params![b],
        )
        .unwrap();
    // missing=1 → 排除
    let e = id_of(&env, "%e.pdf");
    env.conn
        .execute(
            "UPDATE items SET missing = 1 WHERE id = ?1",
            rusqlite::params![e],
        )
        .unwrap();

    let pending = ocr::pending_ocr(&env.conn, None).expect("pending");
    let rels: Vec<String> = pending.iter().map(|p| p.rel_path.clone()).collect();
    assert!(
        rels.contains(&"b.pdf".to_string()),
        "mtime 失配重跑: {rels:?}"
    );
    assert!(
        !rels.contains(&"a.pdf".to_string()),
        "新鲜 done 排除: {rels:?}"
    );
    assert!(
        !rels.contains(&"e.pdf".to_string()),
        "missing 排除: {rels:?}"
    );
    assert_eq!(pending.len(), 3, "b(失配) + c + fail: {rels:?}");

    // root 作用域过滤
    let scoped = ocr::pending_ocr(&env.conn, Some(env.root_id + 999)).expect("scoped");
    assert!(scoped.is_empty());
}

#[test]
fn fake_run_marks_done_fts_and_derived_files() {
    let mut env = setup();
    touch(&env.root.join("a.pdf"), b"%PDF a");
    touch(&env.root.join("b.pdf"), b"%PDF b");
    scan_all(&mut env);

    let dir = std::env::temp_dir().join(format!("vinecurator_ocr_derived_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let runner = FakeRunner {
        fail_on: HashSet::new(),
    };
    let cancel = AtomicBool::new(false);
    let emits = run_queue(&mut env, &runner, &cancel, &dir);
    assert_eq!(emits.last().unwrap().status, "done");
    assert_eq!(emits.last().unwrap().done, 2);
    assert_eq!(emits.last().unwrap().failed, 0);

    // 状态 + 文本 + mtime 保鲜
    assert_eq!(ocr_status(&env, "%a.pdf"), "done");
    let (text, mtime): (String, i64) = env
        .conn
        .query_row(
            "SELECT b.ocr_text, b.ocr_mtime FROM book_items b JOIN items i ON i.id = b.item_id
             WHERE i.rel_path LIKE '%a.pdf'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(text.contains("OCRTEXT a.pdf"), "text={text}");
    assert!(mtime > 0);

    // 派生文件落盘（原始目录只读：root 里无新文件）
    assert!(dir
        .join("ocr")
        .join(format!("{}-{}.txt", id_of(&env, "%a.pdf"), mtime))
        .exists());
    assert!(dir
        .join("ocr")
        .join(format!("{}-{}.pdf", id_of(&env, "%a.pdf"), mtime))
        .exists());

    // FTS 可检索 OCR 文本
    let ids = fts::search_ids(&env.conn, "OCRTEXT", 10).expect("fts");
    assert!(ids.contains(&id_of(&env, "%a.pdf")));
    assert!(ids.contains(&id_of(&env, "%b.pdf")));

    // 二次运行：无候选
    let emits = run_queue(&mut env, &runner, &cancel, &dir);
    assert_eq!(emits.last().unwrap().done, 0);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn failure_marks_failed_and_stays_pending() {
    let mut env = setup();
    touch(&env.root.join("ok.pdf"), b"%PDF ok");
    touch(&env.root.join("fail.pdf"), b"%PDF bad");
    scan_all(&mut env);

    let dir = std::env::temp_dir().join(format!("vinecurator_ocr_fail_{}", std::process::id()));
    let mut fail_on = HashSet::new();
    fail_on.insert("fail.pdf".to_string());
    let runner = FakeRunner { fail_on };
    let cancel = AtomicBool::new(false);
    let emits = run_queue(&mut env, &runner, &cancel, &dir);
    assert_eq!(emits.last().unwrap().done, 1);
    assert_eq!(emits.last().unwrap().failed, 1);

    assert_eq!(ocr_status(&env, "%ok.pdf"), "done");
    assert_eq!(ocr_status(&env, "%fail.pdf"), "failed");
    // failed 仍待跑（可重试）
    let pending = ocr::pending_ocr(&env.conn, None).expect("pending");
    assert_eq!(pending.len(), 1);
    assert!(pending[0].rel_path.ends_with("fail.pdf"));

    // 换成成功的 runner → 重试成功
    let ok_runner = FakeRunner {
        fail_on: HashSet::new(),
    };
    let emits = run_queue(&mut env, &ok_runner, &cancel, &dir);
    assert_eq!(emits.last().unwrap().done, 1);
    assert_eq!(ocr_status(&env, "%fail.pdf"), "done");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cancel_writes_nothing_and_originals_stay_readonly() {
    let mut env = setup();
    touch(&env.root.join("a.pdf"), b"%PDF acontent");
    touch(&env.root.join("b.pdf"), b"%PDF bcontent");
    scan_all(&mut env);
    let dir = std::env::temp_dir().join(format!("vinecurator_ocr_cancel_{}", std::process::id()));
    let runner = FakeRunner {
        fail_on: HashSet::new(),
    };

    // 取消先行 → 零写入（状态保持 none）
    let cancel = AtomicBool::new(true);
    let emits = run_queue(&mut env, &runner, &cancel, &dir);
    assert_eq!(emits.last().unwrap().status, "cancelled");
    assert_eq!(ocr_status(&env, "%a.pdf"), "none");

    // 正常跑：原始文件内容不变（只读不变量）
    let before = fs::read(env.root.join("a.pdf")).unwrap();
    let cancel = AtomicBool::new(false);
    let emits = run_queue(&mut env, &runner, &cancel, &dir);
    assert_eq!(emits.last().unwrap().status, "done");
    assert_eq!(emits.last().unwrap().done, 2);
    let after = fs::read(env.root.join("a.pdf")).unwrap();
    assert_eq!(before, after, "原始媒体必须只读");
    assert!(!env.root.join("a.pdf.pdf").exists(), "原始目录不落派生文件");
    let _ = fs::remove_dir_all(&dir);
}
