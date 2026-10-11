//! 规则自动打标/归类（纯逻辑，零 Tauri）：conditions 复用 `smart::compile_query` 白名单编译，
//! actions 白名单 `add_tag` / `set_category`；打标写 `source='rule'`（ON CONFLICT 不降级已有
//! manual），归类同理走 `item_categories.source='rule'`；打标写入后按触及条目同步 FTS。
//! 进度事件由调用方 emit（`rule-progress`）。

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::types::Value as SqlValue;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{categories, fts, smart, tags};

// ---------------------------------------------------------------- 结构

#[derive(Debug, Clone, Serialize)]
pub struct RuleRow {
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    pub priority: i64,
    /// smart 过滤节点 JSON（对象；兼容旧数组形式）
    pub conditions: String,
    /// `[{"op":"add_tag","value":"未读"},{"op":"set_category","value":"音乐/摇滚"}]`
    pub actions: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleAction {
    pub op: String,
    pub value: Value,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RuleStats {
    pub rules_run: usize,
    pub matched: usize,
    pub tagged: usize,
    pub categorized: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuleProgress {
    /// rule | finished
    pub phase: String,
    pub index: usize,
    pub total: usize,
    pub rule_id: i64,
    pub rule_name: String,
    /// running | done | cancelled | error
    pub status: String,
    pub message: String,
    pub stats: RuleStats,
}

impl RuleProgress {
    pub fn failure(message: impl Into<String>) -> Self {
        RuleProgress {
            phase: "finished".into(),
            index: 0,
            total: 0,
            rule_id: 0,
            rule_name: String::new(),
            status: "error".into(),
            message: message.into(),
            stats: RuleStats::default(),
        }
    }
}

// ---------------------------------------------------------------- 校验

/// conditions 归一化：数组按 `all` 处理（兼容默认 `'[]'`），对象原样
pub fn normalize_conditions(raw: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(raw).map_err(|e| format!("条件 JSON 解析失败: {e}"))?;
    match v {
        Value::Array(arr) => Ok(Value::Object(
            [("all".to_string(), Value::Array(arr))]
                .into_iter()
                .collect(),
        )),
        o @ Value::Object(_) => Ok(o),
        _ => Err("条件必须是对象（all/any/not/叶子）或数组".into()),
    }
}

/// 保存前校验 conditions：走 smart 白名单编译，返回归一化后的 JSON 字符串
pub fn validate_conditions(raw: &str) -> Result<String, String> {
    let node = normalize_conditions(raw)?;
    let query = format!(
        "{{\"filter\":{}}}",
        serde_json::to_string(&node).map_err(|e| format!("条件序列化失败: {e}"))?
    );
    smart::compile_query(&query).map_err(|e| format!("条件非法: {e}"))?;
    Ok(node.to_string())
}

/// 保存前校验 actions：白名单 op（add_tag / set_category）+ 值规则
pub fn validate_actions(raw: &str) -> Result<Vec<RuleAction>, String> {
    let arr: Vec<Value> =
        serde_json::from_str(raw).map_err(|e| format!("动作 JSON 解析失败: {e}"))?;
    if arr.is_empty() {
        return Err("至少需要一个动作".into());
    }
    let mut out = Vec::with_capacity(arr.len());
    for v in arr {
        let o = v
            .as_object()
            .ok_or_else(|| "动作必须是对象 {op, value}".to_string())?;
        let op = o
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| "动作缺少 op".to_string())?;
        let value = o.get("value").ok_or_else(|| "动作缺少 value".to_string())?;
        match op {
            "add_tag" => {
                let s = value
                    .as_str()
                    .ok_or_else(|| "add_tag 的 value 必须是字符串".to_string())?;
                tags::validate_tag_name(s)?;
            }
            "set_category" => {
                let s = value
                    .as_str()
                    .ok_or_else(|| "set_category 的 value 必须是分类路径字符串".to_string())?;
                categories::validate_path_str(s)?;
            }
            other => {
                return Err(format!(
                    "不支持的动作: {other}（支持 add_tag / set_category）"
                ))
            }
        }
        out.push(RuleAction {
            op: op.to_string(),
            value: value.clone(),
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------- CRUD

pub fn list_rules(conn: &Connection) -> Result<Vec<RuleRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, enabled, priority, conditions, actions
             FROM rules ORDER BY priority DESC, id ASC",
        )
        .map_err(|e| format!("读取规则失败: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(RuleRow {
                id: r.get(0)?,
                name: r.get(1)?,
                enabled: r.get::<_, i64>(2)? != 0,
                priority: r.get(3)?,
                conditions: r.get(4)?,
                actions: r.get(5)?,
            })
        })
        .map_err(|e| format!("读取规则失败: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("读取规则失败: {e}"))?);
    }
    Ok(out)
}

/// 保存（id 缺省 = 新建）：conditions/actions 先过白名单校验，再落库；名称唯一
pub fn save_rule(
    conn: &Connection,
    id: Option<i64>,
    name: &str,
    enabled: bool,
    priority: i64,
    conditions: &str,
    actions: &str,
) -> Result<i64, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("规则名不能为空".into());
    }
    let cond = validate_conditions(conditions)?;
    validate_actions(actions)?;
    match id {
        Some(id) => {
            let n = conn
                .execute(
                    "UPDATE rules SET name = ?1, enabled = ?2, priority = ?3,
                     conditions = ?4, actions = ?5, updated_at = datetime('now')
                     WHERE id = ?6",
                    params![name, enabled as i64, priority, cond, actions, id],
                )
                .map_err(|e| {
                    if e.to_string().contains("UNIQUE") {
                        "已存在同名规则".to_string()
                    } else {
                        format!("保存规则失败: {e}")
                    }
                })?;
            if n == 0 {
                return Err(format!("规则不存在: {id}"));
            }
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO rules (name, enabled, priority, conditions, actions)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![name, enabled as i64, priority, cond, actions],
            )
            .map_err(|e| {
                if e.to_string().contains("UNIQUE") {
                    "已存在同名规则".to_string()
                } else {
                    format!("保存规则失败: {e}")
                }
            })?;
            Ok(conn.last_insert_rowid())
        }
    }
}

pub fn delete_rule(conn: &Connection, id: i64) -> Result<(), String> {
    let n = conn
        .execute("DELETE FROM rules WHERE id = ?1", [id])
        .map_err(|e| format!("删除规则失败: {e}"))?;
    if n == 0 {
        return Err(format!("规则不存在: {id}"));
    }
    Ok(())
}

// ---------------------------------------------------------------- 执行

/// 单事务跑完全部启用规则：按 priority DESC 扫描命中 → 打标 source='rule'
/// → 同步 FTS；cancel 置位后在规则边界停止（已写入不回滚整批前提交）。
pub fn run_rules(
    conn: &mut Connection,
    root_id: Option<i64>,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(&RuleProgress),
) -> Result<RuleStats, String> {
    let rules = list_rules(conn)?
        .into_iter()
        .filter(|r| r.enabled)
        .collect::<Vec<_>>();
    let total = rules.len();
    let mut stats = RuleStats::default();

    if total == 0 {
        emit(&RuleProgress {
            phase: "finished".into(),
            index: 0,
            total: 0,
            rule_id: 0,
            rule_name: String::new(),
            status: "done".into(),
            message: "无启用规则".into(),
            stats: stats.clone(),
        });
        return Ok(stats);
    }

    let tx = conn.transaction().map_err(|e| format!("开事务失败: {e}"))?;
    let mut cancelled = false;
    let mut touched: Vec<i64> = Vec::new();

    for (i, rule) in rules.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        emit(&RuleProgress {
            phase: "rule".into(),
            index: i + 1,
            total,
            rule_id: rule.id,
            rule_name: rule.name.clone(),
            status: "running".into(),
            message: format!("规则「{}」", rule.name),
            stats: stats.clone(),
        });

        // 动作为空/非法：跳过该规则（兼容默认 '[]' 行）
        let Ok(actions) = validate_actions(&rule.actions) else {
            continue;
        };
        let node = normalize_conditions(&rule.conditions)?;
        let query = format!(
            "{{\"filter\":{}}}",
            serde_json::to_string(&node).map_err(|e| format!("条件序列化失败: {e}"))?
        );
        let (where_sql, mut args) = match smart::compile_query(&query) {
            Ok((w, a, _, _)) => (w, a),
            Err(e) => return Err(format!("规则「{}」条件非法: {e}", rule.name)),
        };
        let mut sql = format!("SELECT i.id FROM items i WHERE ({where_sql}) AND i.missing = 0");
        if let Some(rid) = root_id {
            sql.push_str(" AND i.root_id = ?");
            args.push(SqlValue::Integer(rid));
        }

        let mut stmt = tx.prepare(&sql).map_err(|e| format!("查询命中失败: {e}"))?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                r.get::<_, i64>(0)
            })
            .map_err(|e| format!("查询命中失败: {e}"))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row.map_err(|e| format!("读取命中失败: {e}"))?);
        }
        drop(stmt);
        stats.matched += ids.len();
        stats.rules_run += 1;

        // 预解析标签 / 分类 id（缺失则创建；set_category 走 ensure_path 建祖先）
        let mut tag_ids: Vec<i64> = Vec::new();
        let mut cat_ids: Vec<i64> = Vec::new();
        for a in &actions {
            match (a.op.as_str(), &a.value) {
                ("add_tag", Value::String(name)) => tag_ids.push(tags::create_tag(&tx, name)?),
                ("set_category", Value::String(path)) => {
                    cat_ids.push(categories::ensure_path(&tx, path)?)
                }
                _ => {}
            }
        }
        tag_ids.sort_unstable();
        tag_ids.dedup();
        cat_ids.sort_unstable();
        cat_ids.dedup();

        for &item_id in &ids {
            if cancel.load(Ordering::Relaxed) {
                cancelled = true;
                break;
            }
            for &tag_id in &tag_ids {
                let n = tx
                    .execute(
                        "INSERT INTO item_tags (item_id, tag_id, source)
                         VALUES (?1, ?2, 'rule')
                         ON CONFLICT(item_id, tag_id) DO NOTHING",
                        params![item_id, tag_id],
                    )
                    .map_err(|e| format!("打标失败: {e}"))?;
                if n > 0 {
                    stats.tagged += 1;
                    touched.push(item_id);
                }
            }
            for &cat_id in &cat_ids {
                let n = tx
                    .execute(
                        "INSERT INTO item_categories (item_id, category_id, source)
                         VALUES (?1, ?2, 'rule')
                         ON CONFLICT(item_id, category_id) DO NOTHING",
                        params![item_id, cat_id],
                    )
                    .map_err(|e| format!("归类失败: {e}"))?;
                if n > 0 {
                    stats.categorized += 1;
                }
            }
        }
        if cancelled {
            break;
        }
    }

    let touched_unique: BTreeSet<i64> = touched.into_iter().collect();
    for id in &touched_unique {
        let _ = fts::sync_item(&tx, *id);
    }

    tx.commit().map_err(|e| format!("提交失败: {e}"))?;

    emit(&RuleProgress {
        phase: "finished".into(),
        index: total,
        total,
        rule_id: 0,
        rule_name: String::new(),
        status: if cancelled { "cancelled" } else { "done" }.into(),
        message: if cancelled { "已取消" } else { "完成" }.into(),
        stats: stats.clone(),
    });
    Ok(stats)
}
