use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use vine_press_lib::manifest::{self, ManifestFile, VerifyReport};

fn fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("vinepress_manifest_test").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn mf(rel: &str, archive: &str, path: &std::path::Path) -> ManifestFile {
    let (size, sha) = manifest::hash_file(path).unwrap();
    ManifestFile {
        rel_path: rel.into(),
        archive: archive.into(),
        size,
        mtime_ms: manifest::mtime_ms(path),
        sha256: sha,
        written_at: manifest::now(),
        fingerprint: String::new(),
    }
}

fn run_verify(out: &std::path::Path) -> VerifyReport {
    let cancel = AtomicBool::new(false);
    let calls = std::cell::Cell::new(0usize);
    let report = manifest::verify(out, &cancel, &|_i, _t, _r| {
        calls.set(calls.get() + 1);
    })
    .unwrap();
    assert!(calls.get() > 0, "应逐文件回调进度");
    report
}

#[test]
fn hash_file_matches_sha256_vector() {
    let d = fixture("hash");
    fs::write(d.join("abc.txt"), b"abc").unwrap();
    let (size, sha) = manifest::hash_file(&d.join("abc.txt")).unwrap();
    assert_eq!(size, 3);
    assert_eq!(
        sha,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn save_load_roundtrip_and_upsert() {
    let out = fixture("roundtrip");
    let a = out.join("音乐/专辑A/专辑A.rar");
    fs::create_dir_all(a.parent().unwrap()).unwrap();
    fs::write(&a, b"archive-bytes-a").unwrap();

    let mut m = manifest::Manifest::default();
    manifest::upsert(&mut m, mf("音乐/专辑A", "音乐/专辑A/专辑A.rar", &a));
    manifest::save(&out, &m).unwrap();

    let loaded = manifest::load(&out).expect("应能加载清单");
    assert_eq!(loaded.files.len(), 1);
    assert_eq!(loaded.files[0].rel_path, "音乐/专辑A");
    assert_eq!(loaded.files[0].size, 15);

    // 同 archive 再 upsert（内容变动）→ 覆盖不追加
    fs::write(&a, b"changed!").unwrap();
    manifest::upsert(&mut m, mf("音乐/专辑A", "音乐/专辑A/专辑A.rar", &a));
    manifest::save(&out, &m).unwrap();
    let loaded2 = manifest::load(&out).unwrap();
    assert_eq!(loaded2.files.len(), 1, "应覆盖同名条目");
    assert_eq!(loaded2.files[0].size, 8);
}

#[test]
fn unchanged_detects_stable_and_modified() {
    let out = fixture("unchanged");
    let a = out.join("x.rar");
    fs::write(&a, b"12345").unwrap();
    let m = manifest::Manifest::default();
    let (size, mtime) = (5u64, manifest::mtime_ms(&a));
    assert!(!manifest::unchanged(&m, "x.rar", size, mtime), "未记录 → 不算 unchanged");
    let mut m2 = m.clone();
    manifest::upsert(
        &mut m2,
        ManifestFile {
            rel_path: "u".into(),
            archive: "x.rar".into(),
            size,
            mtime_ms: mtime,
            sha256: "00".into(),
            written_at: manifest::now(),
            fingerprint: String::new(),
        },
    );
    assert!(manifest::unchanged(&m2, "x.rar", size, mtime));
    assert!(!manifest::unchanged(&m2, "x.rar", size + 1, mtime), "size 变 → 重新哈希");
}

#[test]
fn verify_reports_missing_size_hash_error_and_unlisted() {
    let out = fixture("verify");
    let mut m = manifest::Manifest::default();

    // ok：记录与磁盘一致
    fs::create_dir_all(out.join("单位1")).unwrap();
    let ok_path = out.join("单位1/ok.rar");
    fs::write(&ok_path, b"good-content").unwrap();
    manifest::upsert(&mut m, mf("单位1", "单位1/ok.rar", &ok_path));

    // size：记录 size 与磁盘不符
    let sz_path = out.join("sz.rar");
    fs::write(&sz_path, b"12345").unwrap();
    let mut e = mf("单位2", "sz.rar", &sz_path);
    e.size = 999;
    manifest::upsert(&mut m, e);

    // hash：size 相同但内容（哈希）不符
    let h_path = out.join("h.rar");
    fs::write(&h_path, b"12345").unwrap();
    let mut e = mf("单位3", "h.rar", &h_path);
    e.sha256 = "0".repeat(64);
    manifest::upsert(&mut m, e);

    // missing：记录有但磁盘没有
    manifest::upsert(
        &mut m,
        ManifestFile {
            rel_path: "单位4".into(),
            archive: "单位4/gone.rar".into(),
            size: 10,
            mtime_ms: 0,
            sha256: "ab".repeat(32),
            written_at: manifest::now(),
            fingerprint: String::new(),
        },
    );

    manifest::save(&out, &m).unwrap();

    // 磁盘上额外出现的压缩包 → 未入清单；txt 不算
    fs::create_dir_all(out.join("其他")).unwrap();
    fs::write(out.join("其他/stray.zip"), b"stray").unwrap();
    fs::write(out.join("说明.txt"), b"notes").unwrap();

    let r = run_verify(&out);
    assert_eq!(r.total, 4);
    assert_eq!(r.ok, 1);
    assert_eq!(r.size_bad, 1);
    assert_eq!(r.hash_bad, 1);
    assert_eq!(r.missing, 1);
    assert_eq!(r.error, 0);
    assert_eq!(r.items.len(), 4);
    let status_of = |rel: &str| {
        r.items
            .iter()
            .find(|x| x.rel_path == rel)
            .map(|x| x.status.clone())
            .unwrap()
    };
    assert_eq!(status_of("单位1"), "ok");
    assert_eq!(status_of("单位2"), "size");
    assert_eq!(status_of("单位3"), "hash");
    assert_eq!(status_of("单位4"), "missing");
    assert_eq!(r.unlisted, vec!["其他/stray.zip".to_string()], "txt 不应算未入清单");
}

#[test]
fn verify_without_manifest_errors_and_cancel_stops_early() {
    let out = fixture("nomanifest");
    let cancel = AtomicBool::new(false);
    let err = manifest::verify(&out, &cancel, &|_, _, _| {});
    assert!(err.is_err(), "无清单应报错");

    // 取消置位 → 只检部分
    let mut m = manifest::Manifest::default();
    for i in 0..3 {
        let p = out.join(format!("f{}.rar", i));
        fs::write(&p, b"x").unwrap();
        manifest::upsert(&mut m, mf("u", &format!("f{}.rar", i), &p));
    }
    manifest::save(&out, &m).unwrap();
    let cancel2 = AtomicBool::new(true);
    let r = manifest::verify(&out, &cancel2, &|_, _, _| {}).unwrap();
    assert_eq!(r.items.len(), 0, "取消应立即停止");
    assert_eq!(r.total, 3, "total 仍为清单总数");
}
