//! 临时审计：kind 推断迁移报告（旧算法 vs 新算法）
//! 运行：cargo test --test kind_audit -- --ignored --nocapture
use std::collections::HashMap;
use std::path::Path;
use vine_press_lib::detect::detect;
use vine_press_lib::kindx::enrich_cover_matches;
use vine_press_lib::project::Overrides;
use vine_press_lib::scan::rules;
use vine_press_lib::scan::{scan_tree, DirNode};

const SRC: &str = r"D:\18138";

/// 第二轮基准分布（504 单位；封面 enrich 是新增变量，不一致时人工过样）
const TARGET: &[(&str, u32)] = &[
    ("album", 325),
    ("novel", 4),
    ("artbook", 139),
    ("comic", 23),
    ("score", 6),
    ("concert", 7),
    ("video", 0),
    ("document", 0),
    ("other", 0),
];

const OLD_ARTBOOK: &[&str] = &[
    "画集", "漫画", "图册", "同人誌", "卡片收藏", "壁纸", "设定集", "插画", "CG",
];

/// 旧 infer_kind（修改前逻辑的逐字拷贝，用于迁移对比）
fn old_kind(rel: &str, audio: u32, image: u32, text: u32, doc: u32) -> String {
    if rel.contains("小说") {
        return "novel".into();
    }
    for w in OLD_ARTBOOK {
        if rel.contains(w) {
            return "artbook".into();
        }
    }
    if rel.contains("乐谱") {
        return "other".into();
    }
    if audio > 0 {
        return "album".into();
    }
    if (text > 0 || doc > 0) && image == 0 {
        return "novel".into();
    }
    if image > 0 {
        return "artbook".into();
    }
    "other".into()
}

fn build_index<'a>(node: &'a DirNode, map: &mut HashMap<&'a str, &'a DirNode>) {
    map.insert(&node.rel_path, node);
    for c in &node.children {
        build_index(c, map);
    }
}

fn ext_histogram(node: &DirNode, m: &mut HashMap<String, u32>) {
    for f in &node.files {
        let e = rules::extension_of(&f.name).to_ascii_lowercase();
        if !e.is_empty() {
            *m.entry(e).or_insert(0) += 1;
        }
    }
    for c in &node.children {
        ext_histogram(c, m);
    }
}

fn top_exts(m: &HashMap<String, u32>, n: usize) -> String {
    let mut v: Vec<(&String, &u32)> = m.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1));
    v.into_iter()
        .take(n)
        .map(|(k, c)| format!("{}×{}", k, c))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
#[ignore = "审计工具，需要 D:\\18138"]
fn kind_audit() {
    if !Path::new(SRC).exists() {
        eprintln!("skip: {} 不存在", SRC);
        return;
    }
    let tree = scan_tree(Path::new(SRC)).expect("扫描失败");
    let mut index: HashMap<&str, &DirNode> = HashMap::new();
    build_index(&tree, &mut index);
    let mut result = detect(&tree, &Overrides::default());
    let estat = enrich_cover_matches(Path::new(SRC), &mut result);

    // 分布 + 迁移矩阵
    let mut old_dist: HashMap<String, u32> = HashMap::new();
    let mut new_dist: HashMap<String, u32> = HashMap::new();
    let mut trans: HashMap<(String, String), u32> = HashMap::new();
    let mut moves: HashMap<(String, String), Vec<(String, String)>> = HashMap::new();
    for u in &result.units {
        let old = old_kind(
            &u.rel_path,
            u.stats.audio_count,
            u.stats.image_count,
            u.stats.text_count,
            u.stats.doc_count,
        );
        let new = u.kind.clone();
        *old_dist.entry(old.clone()).or_insert(0) += 1;
        *new_dist.entry(new.clone()).or_insert(0) += 1;
        *trans.entry((old.clone(), new.clone())).or_insert(0) += 1;
        if old != new {
            moves
                .entry((old, new))
                .or_default()
                .push((u.rel_path.clone(), exts_of(&index, &u.rel_path)));
        }
    }

    println!("=== kind 分布（旧 4 类算法 → 新 9 类算法） ===");
    let mut all: Vec<&str> = vec!["album", "novel", "artbook", "comic", "score", "concert", "video", "document", "other"];
    for k in &old_dist {
        if !all.contains(&k.0.as_str()) {
            all.push(k.0);
        }
    }
    for k in &new_dist {
        if !all.contains(&k.0.as_str()) {
            all.push(k.0);
        }
    }
    for k in all {
        println!(
            "  {:10} {} → {}",
            k,
            old_dist.get(k).copied().unwrap_or(0),
            new_dist.get(k).copied().unwrap_or(0)
        );
    }

    println!("\n=== 基准分布核对（第二轮 504 基准；封面 enrich 为新增变量）===");
    for (k, want) in TARGET {
        let got = new_dist.get(*k).copied().unwrap_or(0);
        println!(
            "  {:10} want={} got={} {}",
            k,
            want,
            got,
            if got == *want { "OK" } else { "MISMATCH" }
        );
    }

    println!("\n=== 迁移矩阵 ===");
    let mut tk: Vec<_> = trans.iter().collect();
    tk.sort_by(|a, b| b.1.cmp(a.1));
    for ((o, n), c) in tk {
        println!("  {:10} → {:10} ×{}", o, n, c);
    }

    println!("\n=== 全部迁移明细（旧≠新） ===");
    let mut mk: Vec<_> = moves.iter().collect();
    mk.sort_by_key(|((o, n), _)| format!("{}>{}", o, n));
    for ((o, n), list) in mk {
        println!("  -- {} → {} （{} 个） --", o, n, list.len());
        for (rel, ext) in list {
            println!("    {} | {}", rel, ext);
        }
    }

    println!("\n=== A. 新 kind=artbook 但含音频（应仅剩『画集/…』类分类文件夹） ===");
    let mut a_n = 0;
    for u in &result.units {
        if u.kind == "artbook" && u.stats.audio_count > 0 {
            a_n += 1;
            println!(
                "  audio={} img={} video={} | {}",
                u.stats.audio_count, u.stats.image_count, u.stats.video_count, u.rel_path
            );
        }
    }
    println!("  小计 {}", a_n);

    println!("\n=== B. 新 kind=artbook 的内容构成抽样（含 B 段直方图） ===");
    let mut b_n = 0;
    for u in &result.units {
        if u.kind == "artbook" {
            b_n += 1;
            if b_n <= 40 {
                let hist = index
                    .get(u.rel_path.as_str())
                    .map(|n| {
                        let mut m = HashMap::new();
                        ext_histogram(n, &mut m);
                        top_exts(&m, 8)
                    })
                    .unwrap_or_default();
                println!(
                    "  img={} doc={} text={} audio={} | {} | {}",
                    u.stats.image_count,
                    u.stats.doc_count,
                    u.stats.text_count,
                    u.stats.audio_count,
                    u.rel_path,
                    hist
                );
            }
        }
    }
    println!("  小计 {}（只打印前 40）", b_n);

    println!("\n=== C. 新类型样例（comic/score/concert/video/document 各抽 8 条） ===");
    for kind in ["comic", "score", "concert", "video", "document"] {
        let list: Vec<_> = result.units.iter().filter(|u| u.kind == kind).collect();
        println!("  -- {} ×{} --", kind, list.len());
        for u in list.iter().take(8) {
            println!(
                "    audio={} img={} video={} doc={} | {}",
                u.stats.audio_count, u.stats.image_count, u.stats.video_count, u.stats.doc_count, u.rel_path
            );
        }
    }

    println!("\n=== D. 新 kind=other（应为空或仅剩真正的杂项） ===");
    for u in &result.units {
        if u.kind == "other" {
            println!(
                "  img={} doc={} text={} video={} audio={} | {}",
                u.stats.image_count,
                u.stats.doc_count,
                u.stats.text_count,
                u.stats.video_count,
                u.stats.audio_count,
                u.rel_path
            );
        }
    }

    println!("\n=== E. 新 kind=novel 全量（应为真正的小说/文本作品） ===");
    for u in &result.units {
        if u.kind == "novel" {
            println!(
                "  img={} doc={} text={} audio={} | {}",
                u.stats.image_count, u.stats.doc_count, u.stats.text_count, u.stats.audio_count, u.rel_path
            );
        }
    }

    println!("\n=== F. 低置信复核清单（needs_review，评分层分差不足或 other）===");
    let mut rv: Vec<_> = result.units.iter().filter(|u| u.needs_review).collect();
    rv.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    for u in &rv {
        let v = rules::kind_verdict(&u.rel_path, &u.stats);
        let top: String = rules::kind_scores(&u.stats)
            .iter()
            .take(3)
            .map(|(k, s)| format!("{}={:.2}", k, s))
            .collect::<Vec<_>>()
            .join(" ");
        println!(
            "  kind={} conf={:.2} cover={:?} [{}] | {} | {}",
            u.kind,
            v.confidence,
            u.stats.cover_match,
            top,
            u.rel_path,
            exts_of(&index, &u.rel_path)
        );
    }
    println!("  小计 {} / {}", rv.len(), result.units.len());

    println!("\n=== G. 封面比对执行统计（音频+散图单位）===");
    println!(
        "  candidates={} gated={} computed={}",
        estat.candidates, estat.gated, estat.computed
    );
    for u in &result.units {
        if let Some(r) = u.stats.cover_match {
            println!(
                "  match={:.2} kind={} review={} | {}",
                r, u.kind, u.needs_review, u.rel_path
            );
        }
    }
}

fn exts_of(index: &HashMap<&str, &DirNode>, rel: &str) -> String {
    index
        .get(rel)
        .map(|n| {
            let mut m = HashMap::new();
            ext_histogram(n, &mut m);
            top_exts(&m, 8)
        })
        .unwrap_or_default()
}
