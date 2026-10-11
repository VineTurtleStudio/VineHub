use std::collections::HashSet;
use std::fs;
use std::path::Path;
use vine_press_lib::detect::detect;
use vine_press_lib::project::Overrides;
use vine_press_lib::scan::scan_tree;

const SRC: &str = r"D:\18138";
const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/ground_truth_units.txt"
);

/// 与参考输出 `D:\18138-分享压缩包`（504 个单位目录）比对识别结果。
///
/// 参考工具在输出时会改写少量单位目录名（去掉 `／艺人`、`（备注）` 等），
/// 因此先做精确匹配，剩余部分按「公共前缀 ≥ 40 字符」配对为改名命中。
///
/// 运行：`cargo test --test ground_truth -- --ignored --nocapture`
#[test]
#[ignore = "需要参考数据集 D:\\18138"]
fn ground_truth_units() {
    if !Path::new(SRC).exists() {
        eprintln!("skip: {} 不存在", SRC);
        return;
    }
    let expected: Vec<String> = fs::read_to_string(FIXTURE)
        .expect("读取 fixture")
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    let tree = scan_tree(Path::new(SRC)).expect("扫描失败");
    let result = detect(&tree, &Overrides::default());
    let actual: Vec<String> = result.units.iter().map(|u| u.rel_path.clone()).collect();

    let actual_set: HashSet<&String> = actual.iter().collect();
    let mut unmatched_expected: Vec<String> = Vec::new();
    let mut renamed = 0usize;
    for e in &expected {
        if actual_set.contains(e) {
            continue;
        }
        unmatched_expected.push(e.clone());
    }
    let expected_set: HashSet<&String> = expected.iter().collect();
    let mut unmatched_actual: Vec<String> = actual
        .iter()
        .filter(|a| !expected_set.contains(a))
        .cloned()
        .collect();

    // 改名配对
    let mut pairs: Vec<(String, String, usize)> = Vec::new();
    let mut remaining_actual = Vec::new();
    for e in &unmatched_expected {
        let mut best: Option<(usize, usize)> = None; // (common_len, index)
        for (i, a) in unmatched_actual.iter().enumerate() {
            let common = common_prefix_len(e, a);
            if common >= 40 {
                if best.map(|(l, _)| common > l).unwrap_or(true) {
                    best = Some((common, i));
                }
            }
        }
        match best {
            Some((common, i)) => {
                let a = unmatched_actual.remove(i);
                pairs.push((e.clone(), a, common));
                renamed += 1;
            }
            None => remaining_actual.push(e.clone()),
        }
    }

    println!(
        "expected={} actual={} exact={} renamed={} missing={} extra={} containers={} ignored={}",
        expected.len(),
        actual.len(),
        expected.len() - unmatched_expected.len(),
        renamed,
        remaining_actual.len(),
        unmatched_actual.len(),
        result.container_count,
        result.ignored_count,
    );

    let dump = std::env::temp_dir().join("vinepress_ground_truth");
    let _ = fs::create_dir_all(&dump);
    let _ = fs::write(dump.join("missing.txt"), remaining_actual.join("\n"));
    let _ = fs::write(dump.join("extra.txt"), unmatched_actual.join("\n"));
    let _ = fs::write(dump.join("expected.txt"), expected.join("\n"));
    let _ = fs::write(dump.join("actual.txt"), actual.join("\n"));
    let _ = fs::write(
        dump.join("renamed.txt"),
        pairs
            .iter()
            .map(|(e, a, c)| format!("common={}\n  ref : {}\n  ours: {}\n", c, e, a))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    println!("dump -> {}", dump.display());

    if !remaining_actual.is_empty() {
        println!("--- 真漏检 (基准有 / 我们没有) ---");
        for p in &remaining_actual {
            println!("  {}", p);
        }
    }
    if !unmatched_actual.is_empty() {
        println!("--- 真多检 (我们有 / 基准没有) ---");
        for p in &unmatched_actual {
            println!("  {}", p);
        }
    }

    assert!(
        remaining_actual.is_empty() && unmatched_actual.is_empty(),
        "识别结果与基准不一致: missing={}, extra={}",
        remaining_actual.len(),
        unmatched_actual.len()
    );
}

fn common_prefix_len(a: &str, b: &str) -> usize {
    let mut n = 0usize;
    for (x, y) in a.chars().zip(b.chars()) {
        if x != y {
            break;
        }
        n += 1;
    }
    n
}
