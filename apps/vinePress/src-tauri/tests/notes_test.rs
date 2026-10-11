use std::fs;
use std::path::PathBuf;
use vine_press_lib::notes::{note_text, write_note, NoteInput, NOTE_FOOTER};
use vine_press_lib::project::Entry;
use vine_press_lib::scan::UnitStats;

fn fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("vinepress_notes_test").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("本体")).unwrap();
    fs::write(dir.join("本体").join("page01.jpg"), b"\xff\xd8\xff\xe0fake").unwrap();
    fs::write(dir.join("cover.png"), b"\x89PNG fake").unwrap();
    dir
}

fn entry(rel: &str, name: &str, kind: &str) -> Entry {
    Entry {
        rel_path: rel.into(),
        name: name.into(),
        kind: kind.into(),
        title: format!("测试作品 {}", name),
        artist: "测试艺人".into(),
        date: "2024-06-01".into(),
        label: "测试厂牌".into(),
        catalog: "TST-001".into(),
        publisher: "测试出版方".into(),
        rank: "A".into(),
        work: "1999 Kanon（カノン）".into(),
        category: "画集".into(),
        notes: "备注内容".into(),
        tags: vec!["同人".into(), "CG".into()],
        tracklist: vec![],
        password_profile: None,
        selected: true,
        status: "pending".into(),
        has_notes: false,
        needs_review: false,
        stats: UnitStats::default(),
    }
}

#[test]
fn renders_publication_note_with_bom_and_crlf() {
    let dir = fixture("pub");
    let e = entry("001 测试作品/画集/测试单位", "测试单位", "artbook");
    let stats = UnitStats {
        total_size: 3_145_728,
        file_count: 2,
        image_count: 2,
        ..Default::default()
    };

    let input = NoteInput {
        entry: &e,
        stats: &stats,
        unit_path: &dir,
    };

    let text = note_text(&input);
    assert!(text.contains(NOTE_FOOTER), "应包含页脚: {}", text);
    assert!(!text.starts_with('\u{feff}'), "注释正文不应带 BOM");
    assert!(text.contains("测试作品"), "应包含标题: {}", text);
    assert!(text.contains("画集"), "应包含分类: {}", text);
    assert!(text.contains("1999 Kanon"), "应包含所属作品");

    let (path, rendered) = write_note(&input).expect("写入失败");
    assert_eq!(rendered, text, "写入返回的正文应与 note_text 一致");
    assert_eq!(path, dir.join("测试单位.txt"));
    let bytes = fs::read(&path).unwrap();
    assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF], "应带 UTF-8 BOM");
    let body = String::from_utf8(bytes[3..].to_vec()).expect("应为 UTF-8");
    assert!(body.contains("\r\n"), "应使用 CRLF 换行");
    assert!(body.contains(NOTE_FOOTER));

    // 二次生成应覆盖同一文件，不产生多余 txt
    write_note(&input).unwrap();
    let txts: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().map(|x| x == "txt").unwrap_or(false))
        .collect();
    assert_eq!(txts.len(), 1, "只应保留一份说明文件");
}

#[test]
fn album_note_lists_tracklist_when_provided() {
    let dir = fixture("album");
    let mut e = entry("KEY音乐/测试专辑", "测试专辑", "album");
    e.tracklist = vec!["01 序曲".into(), "02 终曲".into()];
    let stats = UnitStats::default();
    let input = NoteInput {
        entry: &e,
        stats: &stats,
        unit_path: &dir,
    };
    let text = note_text(&input);
    assert!(text.contains("01 序曲"), "应包含曲目: {}", text);
    assert!(text.contains("测试艺人"), "应包含艺人");
}
