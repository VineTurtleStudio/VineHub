use std::fs;
use std::path::PathBuf;
use vine_press_lib::meta::{extract, parse_dir_name, propose_all};
use vine_press_lib::project::Entry;
use vine_press_lib::scan::UnitStats;

fn entry_of(rel: &str, name: &str) -> Entry {
    let mut e = Entry {
        rel_path: rel.into(),
        name: name.into(),
        kind: "album".into(),
        title: String::new(),
        artist: String::new(),
        date: String::new(),
        label: String::new(),
        catalog: String::new(),
        publisher: String::new(),
        rank: String::new(),
        work: String::new(),
        category: String::new(),
        notes: String::new(),
        tags: vec![],
        tracklist: vec![],
        password_profile: None,
        selected: true,
        status: "pending".into(),
        has_notes: false,
        needs_review: false,
        stats: UnitStats::default(),
    };
    e.work = "1999 Kanon（カノン）".into();
    e.category = "乐谱".into();
    e
}

fn fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("vinepress_meta_test").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn parses_date_catalog_artist_from_dir_name() {
    let d = parse_dir_name(
        "[2001.08.10][GMLS-0004] AIRness -BEST SOUND-／茶太（GameMusicLibrary） [cd-flac+cue+log+img]",
    );
    assert_eq!(d.date, "2001-08-10");
    assert_eq!(d.catalog, "GMLS-0004");
    assert_eq!(d.artist, "茶太（GameMusicLibrary）");
    assert!(
        d.title.starts_with("AIRness"),
        "标题应去掉日期/品番/格式标签: {}",
        d.title
    );
    assert!(!d.title.contains("[2001"), "标题不应含日期标签: {}", d.title);
    assert!(!d.title.contains("cd-flac"), "标题不应含格式标签: {}", d.title);
}

#[test]
fn strips_index_prefix_and_keeps_parens() {
    let d = parse_dir_name("002 1999 Kanon（カノン）");
    assert_eq!(d.title, "1999 Kanon（カノン）");
    let d2 = parse_dir_name("KSLA [2014.12.28][KSLA-0098] Girls Dead Monster『rare tracks』／Girls Dead Monster");
    assert_eq!(d2.catalog, "KSLA-0098");
    assert_eq!(d2.artist, "Girls Dead Monster");
    assert!(d2.date == "2014-12-28");
}

#[test]
fn extracts_from_cue_and_readme() {
    let dir = fixture("cue_unit");
    fs::write(dir.join("disc.cue"), "TRACK 01\n  TITLE \"序曲\"\nTRACK 02\n  TITLE \"終曲\"\n").unwrap();
    fs::write(dir.join("说明.txt"), "这是一张 2003 年的同人专辑。").unwrap();
    fs::write(dir.join("a.flac"), b"fLaC-not-really").unwrap();

    let e = entry_of("KEY音乐/测试单位", "测试单位");
    let fields: Vec<String> = ["title", "artist", "date", "catalog", "tracklist", "notes", "tags"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let found = extract(&e, &dir, &fields);

    let tracks = found.get("tracklist").expect("应有曲目提案");
    assert!(tracks.0.contains("序曲"), "{}", tracks.0);
    assert!(tracks.0.contains("終曲"), "{}", tracks.0);

    let notes = found.get("notes").expect("应有备注提案");
    assert!(notes.0.contains("同人专辑"), "{}", notes.0);
    assert_eq!(notes.1, "目录内说明文本");

    let tags = found.get("tags").expect("应有标签提案");
    assert!(tags.0.contains("cue"), "{}", tags.0);
    assert!(tags.0.contains("flac"), "{}", tags.0);
}

#[test]
fn parses_existing_note_fields() {
    let dir = fixture("note_unit");
    fs::create_dir_all(dir.join("本体")).unwrap();
    fs::write(dir.join("本体").join("p.png"), b"png").unwrap();
    let note = "\
==========================================================
  资料信息 （Publication Info）
==========================================================

【作品名称】Kissa monochrome (Various)
【所属作品】画师作品集
【分类】同人誌
【出版方／发行商】非官方同人出版物（本地资料未记录原发行方）
【发行日期】2016-04-29
【文件格式】PNG（11）
";
    fs::write(dir.join("测试单位.txt"), note).unwrap();

    let e = entry_of("A/B/测试单位", "测试单位");
    let fields: Vec<String> = ["title", "date", "publisher", "rank"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let found = extract(&e, &dir, &fields);
    assert_eq!(
        found.get("title").map(|x| x.0.as_str()),
        Some("Kissa monochrome (Various)"),
        "{:?}",
        found
    );
    assert_eq!(found.get("date").map(|x| x.0.as_str()), Some("2016-04-29"));
    // 占位值不应成为提案
    assert!(found.get("publisher").is_none(), "占位出版方应被忽略");
    assert!(found.get("rank").is_none());
}

#[test]
fn propose_all_skips_unchanged_and_missing_dirs() {
    let root = fixture("root");
    let unit = root.join("分类").join("单位A");
    fs::create_dir_all(&unit).unwrap();
    fs::write(unit.join("x.txt"), b"hi").unwrap();

    let mut e = entry_of("分类/单位A", "单位A");
    e.title = "单位A".into(); // 与目录名清洗结果相同 -> 不提案
    e.category = "分类".into();
    e.work = "分类".into();

    let fields: Vec<String> = ["title", "category", "work", "notes"]
        .iter()
        .map(|s| s.to_string())
        .collect();

    // 缺失目录 -> 无提案
    let ghost = entry_of("分类/不存在", "不存在");
    let out = propose_all(&root, &[ghost], &fields);
    assert!(out.is_empty(), "{:?}", out);

    let out = propose_all(&root, &[e], &fields);
    let fields_found: Vec<&str> = out.iter().map(|p| p.field.as_str()).collect();
    assert!(
        !fields_found.contains(&"title"),
        "相同标题不应提案: {:?}",
        out
    );
    assert!(
        !fields_found.contains(&"category"),
        "相同分类不应提案: {:?}",
        out
    );
}

#[test]
fn parses_flac_vorbis_comment_block() {
    // 手工构造 FLAC：magic + 最后一个 type=4 (VORBIS_COMMENT) 块
    let vendor = b"vinepress";
    let entries: Vec<Vec<u8>> = vec![
        "TITLE=测试专辑".as_bytes().to_vec(),
        "ARTIST=测试艺人".as_bytes().to_vec(),
        "DATE=2020-05-01".as_bytes().to_vec(),
        "CATALOGNUMBER=KSLA-0098".as_bytes().to_vec(),
        "LABEL=KSLA".as_bytes().to_vec(),
    ];
    let mut body = Vec::new();
    body.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    body.extend_from_slice(vendor);
    body.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for e in &entries {
        body.extend_from_slice(&(e.len() as u32).to_le_bytes());
        body.extend_from_slice(e);
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"fLaC");
    bytes.push(0x84); // last + type 4
    let len = body.len() as u32;
    bytes.push(((len >> 16) & 0xFF) as u8);
    bytes.push(((len >> 8) & 0xFF) as u8);
    bytes.push((len & 0xFF) as u8);
    bytes.extend_from_slice(&body);

    let dir = fixture("flac_unit");
    fs::write(dir.join("t.flac"), &bytes).unwrap();
    let tags = vine_press_lib::meta::read_tags(&dir);
    assert_eq!(tags.title, "测试专辑");
    assert_eq!(tags.artist, "测试艺人");
    assert_eq!(tags.date, "2020-05-01");
    assert_eq!(tags.catalog, "KSLA-0098");
    assert_eq!(tags.label, "KSLA");
}

#[test]
fn parses_id3v2_frames() {
    fn syncsafe(v: usize) -> [u8; 4] {
        [
            ((v >> 21) & 0x7F) as u8,
            ((v >> 14) & 0x7F) as u8,
            ((v >> 7) & 0x7F) as u8,
            (v & 0x7F) as u8,
        ]
    }
    fn text_frame(id: &str, text: &str) -> Vec<u8> {
        let mut body = vec![3u8]; // UTF-8
        body.extend_from_slice(text.as_bytes());
        let mut f = id.as_bytes().to_vec();
        f.extend_from_slice(&syncsafe(body.len()));
        f.extend_from_slice(&[0, 0]);
        f.extend_from_slice(&body);
        f
    }
    let mut frames = text_frame("TIT2", "ID3标题");
    frames.extend(text_frame("TPE1", "ID3艺人"));
    frames.extend(text_frame("TALB", "ID3专辑"));
    frames.extend(text_frame("TDRC", "1999-11-05"));

    let mut tag = b"ID3".to_vec();
    tag.push(3); // v2.3
    tag.push(0);
    tag.push(0);
    tag.extend_from_slice(&syncsafe(frames.len()));
    tag.extend_from_slice(&frames);

    let dir = fixture("mp3_unit");
    fs::write(dir.join("t.mp3"), &tag).unwrap();
    let tags = vine_press_lib::meta::read_tags(&dir);
    assert_eq!(tags.title, "ID3标题");
    assert_eq!(tags.artist, "ID3艺人");
    assert_eq!(tags.album, "ID3专辑");
    assert_eq!(tags.date, "1999-11-05");
}

#[test]
fn parses_note_blocks_with_multiline_value() {
    use vine_press_lib::meta::parse_note_blocks;
    let text = concat!(
        "==========================================================\n",
        "  专辑信息 （Album Info）\n",
        "==========================================================\n",
        "\n",
        "【专辑名称】旧标题\n",
        "【艺人】某艺人\n",
        "\n",
        "【曲目列表 / Tracklist】\n",
        " 01. First\n",
        " 02. Second\n",
        "\n",
        "----------------------------------------------------------\n",
        "  本信息文件由本地曲库自动整理生成\n",
        "==========================================================\n",
    );
    let blocks = parse_note_blocks(text);
    let keys: Vec<&str> = blocks.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, ["专辑名称", "艺人", "曲目列表 / Tracklist"]);
    assert_eq!(blocks[0].1, "旧标题");
    assert_eq!(blocks[2].1, "01. First\n 02. Second");
}

#[test]
fn diffs_note_fields_by_status() {
    use vine_press_lib::meta::diff_note_text;
    let old = "【专辑名称】旧标题\n【艺人】某艺人\n【唱片公司】旧厂牌\n【备注】仅旧有\n";
    let new = "【专辑名称】新标题\n【艺人】某艺人\n【规格品番】KSLA-0098\n";
    let diff = diff_note_text(old, new);
    let by_field = |f: &str| diff.iter().find(|d| d.field == f).unwrap().clone();

    assert_eq!(by_field("专辑名称").status, "changed");
    assert_eq!(by_field("专辑名称").old, "旧标题");
    assert_eq!(by_field("专辑名称").new, "新标题");
    assert_eq!(by_field("艺人").status, "same");
    assert_eq!(by_field("规格品番").status, "added");
    assert_eq!(by_field("唱片公司").status, "removed");
    assert_eq!(by_field("备注").status, "removed");
    // 主序 = 新渲染顺序，旧文件独有字段在末尾
    let order: Vec<&str> = diff.iter().map(|d| d.field.as_str()).collect();
    assert_eq!(order, ["专辑名称", "艺人", "规格品番", "唱片公司", "备注"]);

    // 没有现有说明文件（old 为空）→ 全部 added
    let all_added = diff_note_text("", new);
    assert!(all_added.iter().all(|d| d.status == "added"));
}

#[test]
fn audits_missing_changed_and_stale_fields() {
    use vine_press_lib::meta::audit_note;
    let existing = "【专辑名称】旧标题\n【艺人】某艺人\n【备注】仅旧有\n";
    let rendered = "【专辑名称】新标题\n【艺人】某艺人\n【规格品番】KSLA-0098\n";
    let a = audit_note(Some(existing), rendered);
    assert!(a.exists);
    assert_eq!(a.missing, ["规格品番"]);
    assert_eq!(a.added, 1);
    assert_eq!(a.changed, 1); // 专辑名称
    assert_eq!(a.same, 1); // 艺人
    assert_eq!(a.removed, 1); // 备注（新模板已不含）
}

#[test]
fn audits_note_absent_and_identical() {
    use vine_press_lib::meta::audit_note;
    // 文件不存在 → 只标记缺失，不做字段计数
    let absent = audit_note(None, "【专辑名称】X\n");
    assert!(!absent.exists);
    assert!(absent.missing.is_empty());
    assert_eq!((absent.changed, absent.added, absent.removed, absent.same), (0, 0, 0, 0));

    // 完全一致 → 零缺失、零变更
    let full = "【专辑名称】X\n【艺人】Y\n";
    let ok = audit_note(Some(full), "【专辑名称】X\n【艺人】Y\n");
    assert!(ok.exists);
    assert!(ok.missing.is_empty());
    assert_eq!((ok.changed, ok.removed, ok.same), (0, 0, 2));
}
