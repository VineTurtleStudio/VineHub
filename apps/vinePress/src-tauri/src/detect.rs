use crate::project::Overrides;
use crate::scan::rules;
use crate::scan::{collect_unit_stats, DirNode, UnitStats};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Unit,
    Ignore,
    Recurse,
}

/// 分类一个目录：单位（最小压缩单位）/ 忽略 / 继续下钻
fn classify(node: &DirNode) -> Class {
    // 1) 直接含有内容文件 -> 单位
    if node.has_content() {
        return Class::Unit;
    }
    // 2) 叶子：有非系统文件 -> 单位；空/纯系统 -> 忽略
    if node.children.is_empty() {
        return if node.has_meaningful_file() {
            Class::Unit
        } else {
            Class::Ignore
        };
    }
    // 3) 分类容器词（画集/漫画/小说/CG/回收站 …）-> 下钻
    if rules::is_category_container(&node.name) {
        return Class::Recurse;
    }
    // 4) 所有子目录都是碟片/分卷名 -> 母文件夹整体作为一个单位
    if node.children.iter().all(|c| rules::is_part_name(&c.name)) {
        return Class::Unit;
    }
    // 5) 其余 -> 容器
    Class::Recurse
}

/// 手动覆盖优先于自动分类：
/// 1. `rel ∈ force_units`      → 单位（停止下钻；子目录整体并入 = 合并/拆分的共同机制）
/// 2. `rel` 是某覆盖路径的祖先 → 继续下钻（保证深层覆盖可达）
/// 3. `rel ∈ force_containers` → 继续下钻
/// 4. 否则走原 classify（overrides 为空时行为与历史完全一致）
fn classify_with(node: &DirNode, ov: &Overrides) -> Class {
    let rel = node.rel_path.as_str();
    if ov.force_units.iter().any(|p| p == rel) {
        return Class::Unit;
    }
    if ov
        .force_units
        .iter()
        .chain(ov.force_containers.iter())
        .any(|p| is_strict_ancestor(rel, p))
    {
        return Class::Recurse;
    }
    if ov.force_containers.iter().any(|p| p == rel) {
        return Class::Recurse;
    }
    classify(node)
}

/// `ancestor` 是否是 `path` 的真祖先（`ancestor/...`）
fn is_strict_ancestor(ancestor: &str, path: &str) -> bool {
    path.len() > ancestor.len()
        && path.starts_with(ancestor)
        && path.as_bytes()[ancestor.len()] == b'/'
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectNode {
    pub name: String,
    pub rel_path: String,
    pub is_unit: bool,
    pub kind: String,
    pub has_notes: bool,
    /// 低置信判定（评分层分差不足/other），需人工复核
    #[serde(default)]
    pub needs_review: bool,
    pub stats: Option<UnitStats>,
    pub children: Vec<DetectNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnitInfo {
    pub rel_path: String,
    pub name: String,
    pub kind: String,
    pub has_notes: bool,
    /// 低置信判定（评分层分差不足/other），需人工复核
    #[serde(default)]
    pub needs_review: bool,
    pub stats: UnitStats,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DetectResult {
    /// 根目录下的子节点树（容器 + 单位混合）
    pub tree: Vec<DetectNode>,
    /// 扁平单位列表（最小压缩单位）
    pub units: Vec<UnitInfo>,
    pub container_count: u32,
    pub ignored_count: u32,
    pub total_size: u64,
    pub total_files: u32,
    /// 扫描根目录直属的内容文件（rel 模型只分类子目录，这些散件不入任何单位——
    /// 仅提示用户，不改变识别边界）
    pub root_loose: Vec<String>,
}

pub fn detect(root: &DirNode, ov: &Overrides) -> DetectResult {
    let mut result = DetectResult {
        total_size: root.total_size,
        total_files: root.total_files,
        root_loose: root
            .files
            .iter()
            .filter(|f| rules::is_content_file(&f.name))
            .map(|f| f.name.clone())
            .collect(),
        ..Default::default()
    };
    result.tree = detect_children(root, ov, &mut result);
    result
}

fn detect_children(parent: &DirNode, ov: &Overrides, result: &mut DetectResult) -> Vec<DetectNode> {
    let mut out = Vec::new();
    for child in &parent.children {
        match classify_with(child, ov) {
            Class::Ignore => {
                result.ignored_count += 1;
            }
            Class::Unit => {
                let stats = collect_unit_stats(child);
                let verdict = rules::kind_verdict(&child.rel_path, &stats);
                let kind = verdict.kind.to_string();
                // 受管说明文件 = `<单位名>.txt` 精确匹配（任意 .txt 会把 readme.txt 误判为已生成）
                let note_name = format!("{}.txt", child.name).to_lowercase();
                let has_notes = child.files.iter().any(|f| f.name.to_lowercase() == note_name);
                result.units.push(UnitInfo {
                    rel_path: child.rel_path.clone(),
                    name: child.name.clone(),
                    kind: kind.clone(),
                    has_notes,
                    needs_review: verdict.review,
                    stats: stats.clone(),
                });
                out.push(DetectNode {
                    name: child.name.clone(),
                    rel_path: child.rel_path.clone(),
                    is_unit: true,
                    kind,
                    has_notes,
                    needs_review: verdict.review,
                    stats: Some(stats),
                    children: Vec::new(),
                });
            }
            Class::Recurse => {
                result.container_count += 1;
                let children = detect_children(child, ov, result);
                out.push(DetectNode {
                    name: child.name.clone(),
                    rel_path: child.rel_path.clone(),
                    is_unit: false,
                    kind: "group".into(),
                    has_notes: false,
                    needs_review: false,
                    stats: None,
                    children,
                });
            }
        }
    }
    out
}
