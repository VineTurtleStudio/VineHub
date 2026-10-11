# 迁移清单

规则：`migrations/*.sql` 按文件名升序、每文件事务内执行、`schema_migrations(version, name, applied_at)` 记账；首次启动自动跑；**只增不改**（要改就新增下一个编号）。SQL 文件内不得写 `BEGIN`/`COMMIT`（由迁移器事务包裹）。

| 版本 | 文件 | 内容 |
|---|---|---|
| 1 | `0001_core.sql` | `settings`、`roots`、`items`、`categories`、`item_categories`、`tags`、`tag_aliases`、`item_tags`、`smart_collections`、`rules`、`jobs` + 主要索引 |
| 2 | `0002_type_ext.sql` | 类型扩展（全部 1:1 → items）：`music_tracks`、`video_items`、`book_items`（含 `ocr_status`）、`comic_items` |
| 3 | `0003_fts.sql` | `fts_items` FTS5 虚拟表（`unicode61 remove_diacritics 2`；列 `item_id UNINDEXED/title/body/tags_text`；无 trigger，Rust 侧显式同步） |
| 4 | `0004_collab.sql` | `users`、`audit_log`（阶段五协作占位，第一版就建表） |
| 5 | `0005_meta_state.sql` | `items.meta_state`（none|done|failed 元数据状态机；扫描指纹变化时同步重置） |
| 6 | `0006_dupes.sql` | `items.partial_hash`（重复检测部分哈希）+ `idx_items_size` 索引 |
| 7 | `0007_ocr.sql` | `book_items.ocr_text`（OCR 文本入 FTS，200_000 字符截断）+ `book_items.ocr_mtime`（mtime 失配自动重跑） |
| 8 | `0008_item_category_source.sql` | `item_categories.source`（manual\|auto\|rule，默认 manual；自动分类重跑只动 auto 行） |

## 核心表要点（0001）

- `items`：`root_id + rel_path` 唯一；`type CHECK IN (music|video|book|comic|image|other)`；`status CHECK IN (inbox|unwatched|reading|done|dropped)`；`rating 0..5`；`hash_state IN (none|pending|done)` 懒哈希；`missing` 标记（文件消失不直接删）；`year/language/has_cover` 供智能集合白名单；`metadata` JSON。
- `categories`：物化路径 `path` 唯一（根级 `音乐`、子级 `音乐/爵士`，无前导 `/`），子树枚举 `path LIKE ? || '/%'`。
- `item_categories`：多对多归属，`source IN (manual|auto|rule)`（0008）——目录镜像自动分类 / 规则动作 / 用户手挂互不覆盖。
- `tag_aliases`：同义词 → `tag_id`；`item_tags.source IN (manual|rule|import)`。
- `settings`：键值配置（工具路径、派生目录等），列名 `"key"/"value"` 已加引号。

## 验证

`src-tauri/tests/migrate_test.rs` 锁死：

1. 首轮按序应用全部迁移、二次迁移幂等；
2. 19 张表齐全（含 schema_migrations）；
3. `items` 的 CHECK 约束（非法 type/rating 拒绝）；
4. FTS5 `MATCH` 往返（bundled SQLite 含 FTS5 实测）；
5. 连接池 `PRAGMA journal_mode=wal` 生效 + `settings` upsert 往返。
