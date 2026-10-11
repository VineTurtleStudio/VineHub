use rusqlite::Connection;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use vine_curator_lib::auth;
use vine_curator_lib::db;

fn setup() -> Connection {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "vinecurator_auth_test_{}_{}",
        std::process::id(),
        n
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("临时目录");
    let mut conn = Connection::open(dir.join("test.sqlite")).expect("打开");
    conn.execute_batch("PRAGMA foreign_keys=ON;")
        .expect("pragma");
    db::migrate::migrate(&mut conn).expect("迁移");
    conn
}

#[test]
fn hash_and_verify_roundtrip() {
    let hash = auth::hash_password("s3cret-pw").expect("哈希");
    assert!(hash.starts_with("$argon2"));
    assert!(auth::verify_password("s3cret-pw", &hash));
    assert!(!auth::verify_password("wrong-pw", &hash));
    // 损坏的哈希串不 panic，直接判失败
    assert!(!auth::verify_password("x", "not-a-hash"));
    // 弱密码拒绝
    assert!(auth::hash_password("123").is_err());
}

#[test]
fn setup_admin_only_when_empty() {
    let conn = setup();
    assert_eq!(auth::count_users(&conn).unwrap(), 0);
    let id = auth::setup_admin(&conn, "admin", "admin-pw-1").expect("首个管理员");
    assert_eq!(auth::count_users(&conn).unwrap(), 1);
    // 再次调用被拒
    assert!(auth::setup_admin(&conn, "admin2", "admin-pw-2").is_err());
    assert_eq!(auth::count_users(&conn).unwrap(), 1);
    let users = auth::list_users(&conn).unwrap();
    assert_eq!(users[0].id, id);
    assert_eq!(users[0].role, "admin");
}

#[test]
fn create_user_validations_and_duplicate() {
    let conn = setup();
    auth::setup_admin(&conn, "admin", "admin-pw-1").unwrap();
    // 用户名
    assert!(auth::create_user(&conn, "   ", "valid-pw", "viewer").is_err());
    assert!(auth::create_user(&conn, "has space", "valid-pw", "viewer").is_err());
    // 密码
    assert!(auth::create_user(&conn, "u1", "123", "viewer").is_err());
    // 角色
    assert!(auth::create_user(&conn, "u1", "valid-pw", "root").is_err());
    // 正常创建
    auth::create_user(&conn, "u1", "valid-pw", "viewer").expect("创建");
    // 重名
    assert!(auth::create_user(&conn, "u1", "valid-pw", "editor").is_err());
}

#[test]
fn authenticate_success_failure_and_unknown() {
    let conn = setup();
    auth::create_user(&conn, "alice", "alice-pw", "editor").unwrap();
    let user = auth::authenticate(&conn, "alice", "alice-pw")
        .expect("查询不报错")
        .expect("应登录成功");
    assert_eq!(user.username, "alice");
    assert_eq!(user.role, "editor");
    // 密码错误
    assert!(auth::authenticate(&conn, "alice", "bad-pw")
        .expect("查询不报错")
        .is_none());
    // 用户不存在
    assert!(auth::authenticate(&conn, "nobody", "whatever")
        .expect("查询不报错")
        .is_none());
    // 用户名两侧空白可容忍
    assert!(auth::authenticate(&conn, "  alice  ", "alice-pw")
        .expect("查询不报错")
        .is_some());
}

#[test]
fn keep_at_least_one_admin() {
    let conn = setup();
    let admin_id = auth::setup_admin(&conn, "admin", "admin-pw-1").unwrap();
    // 唯一管理员：不能降级、不能删除
    assert!(auth::set_role(&conn, admin_id, "viewer").is_err());
    assert!(auth::delete_user(&conn, admin_id).is_err());
    // 第二个管理员就位后可降级
    let admin2 = auth::create_user(&conn, "admin2", "admin-pw-2", "admin").unwrap();
    auth::set_role(&conn, admin_id, "editor").expect("两个管理员时可降级");
    // 此时 admin2 是唯一管理员：不可删、不可降级
    assert!(auth::delete_user(&conn, admin2).is_err());
    assert!(auth::set_role(&conn, admin2, "viewer").is_err());
    // 恢复双管理员后删除 admin2
    auth::set_role(&conn, admin_id, "admin").expect("升回 admin");
    auth::delete_user(&conn, admin2).expect("删除");
    assert_eq!(auth::count_users(&conn).unwrap(), 1);
    // admin_id 又是唯一管理员 → 不允许再删
    assert!(auth::delete_user(&conn, admin_id).is_err());
    // 不存在的用户
    assert!(auth::delete_user(&conn, 999).is_err());
    assert!(auth::set_role(&conn, 999, "viewer").is_err());
}

#[test]
fn change_password_flow() {
    let conn = setup();
    let id = auth::create_user(&conn, "bob", "old-pw-1", "viewer").unwrap();
    // 旧密码错误
    assert!(auth::change_password(&conn, id, "wrong-pw", "new-pw-1").is_err());
    // 新密码太弱
    assert!(auth::change_password(&conn, id, "old-pw-1", "123").is_err());
    // 正常改密
    auth::change_password(&conn, id, "old-pw-1", "new-pw-1").expect("改密");
    assert!(auth::authenticate(&conn, "bob", "new-pw-1")
        .expect("查询")
        .is_some());
    assert!(auth::authenticate(&conn, "bob", "old-pw-1")
        .expect("查询")
        .is_none());
}

#[test]
fn auth_state_session_lifecycle() {
    let st = auth::AuthState::default();
    assert!(st.current().is_none());
    assert!(st.user_id().is_none());
    st.login(auth::UserRow {
        id: 7,
        username: "u".into(),
        role: "admin".into(),
        created_at: String::new(),
    });
    assert_eq!(st.user_id(), Some(7));
    assert_eq!(st.current().unwrap().username, "u");
    st.logout();
    assert!(st.current().is_none());
}
