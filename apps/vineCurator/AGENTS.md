# vineCurator — 开发约定

本地优先的媒体库索引与策展工具：统一管理约 1.1TB 本地媒体（500G+ 音乐、400G+ 视频、200G+ 电子书/图册/扫描本），层级分类 + 扁平标签 + 智能集合 + FTS5 全文检索。
架构与 `vinePress` 一致：**Tauri 2.x + Rust + 零框架 TypeScript + Vite**。

## 常用命令（Windows / PowerShell）

```powershell
npm.cmd run dev            # 仅前端，端口 5175（5173 VinePalmaCursor、5174 vinePress，strictPort）
npm.cmd run tauri dev      # 联调（前端热更新 + cargo watch）
npm.cmd run build          # tsc && vite build
npm.cmd run lint           # tsc --noEmit
npm.cmd run fmt            # prettier --write src
cargo fmt --check          # Rust 格式检查（先 cargo fmt 修复）
cargo clippy --all-targets -- -D warnings
cargo test                 # 全量单测（migrate/repo/scan/meta/browse/fts/category/tag/smart/rule/dupe/ocr/backup/port/autocat，不需本机数据/外部工具）
```

- `npm` 必须写 `npm.cmd`（npm.ps1 被执行策略禁用）。
- PowerShell 写文件必须显式 `-Encoding UTF8`，否则 UTF-8 会被 GBK 损坏；**读改源码一律禁用 `Get-Content`/`Set-Content`/`-replace` 管道**（PS5.1 把无 BOM UTF-8 按 GBK 解码 → 中文乱码、换行被吞），批量替换用 Node 或编辑工具（2026-10-09 ui.ts 事故即此因，靠 opencode.db part 表重放 59 个 edit 恢复）。
- esbuild 安装脚本由 `package.json.allowScripts` 放行（对齐 vinePress）。
- **杀软误杀**：Defender 会间歇性删 `src-tauri\target\debug` 下刚编译的 exe（`os error 5` / `never executed`，build-script 与测试 exe 都可能中招，无管理员权限无法加排除项）。遇到时改用独立目标目录：`$env:CARGO_TARGET_DIR = "$env:TEMP\vinecurator_target"` 再跑 cargo 命令（实测可绕过）；`Cargo.toml` 已加 `[profile.dev] strip = "debuginfo"` 降低误报面。

## 分层约定

- 纯逻辑（**零 Tauri 依赖**，`tests/` 直接调用）：`src-tauri/src/scan/`、`meta/`、`categories.rs`、`tags.rs`、`rules.rs`、`smart.rs`、`fts.rs`、`tasks.rs`、`thumbs.rs`、`dupes.rs`（重复检测部分哈希/分组报告）、`ocr.rs`（OCR 队列：`OcrRunner` trait 注入 + 状态机）、`backup.rs`（元数据快照 `VACUUM INTO` + `ResticRunner` 注入）、`port.rs`（标签/分类 JSON 导入导出）、`browse.rs`（过滤/排序/分页/详情/搜索白名单）、`shell.rs`（open/reveal 只读）、`env.rs`（外部工具探测）、`auth.rs`（Argon2 用户/角色/会话 `AuthState`）、`audit.rs`（审计动作白名单 + 写入/查询）、`autocat.rs`（目录结构镜像自动分类，source=auto 幂等重跑）。
- `src-tauri/src/db/`：`mod.rs` 连接池（r2d2 + WAL + busy_timeout）、`migrate.rs` 迁移器（`include_str!` 内嵌 SQL、逐文件事务、`schema_migrations` 记账）、`repo.rs` 仓储（settings 等）。
- `src-tauri/src/commands.rs`：薄桥接（`#[tauri::command]` + 事件 emit + `JobState` 单任务互斥），不写业务逻辑。
- `src/api.ts`：前端唯一 invoke 出口；**结构体字段**与 Rust `Serialize/Deserialize` 一一对应（snake_case），**命令参数**按 Tauri 默认 camelCase 传（Rust `root_id` ↔ JS `rootId`）。进度事件统一 `on*Progress` 订阅。
- 前端：`index.html` 为快捷键唯一来源（`data-keys`，菜单与命令面板同源）；`main.ts` 事件接线、`state.ts` 状态、`ui.ts` 渲染、`layout.ts` 面板/列宽持久化、`menu.ts` 菜单、`settings.ts` 设置；`features/` 按功能域拆分。
- 测试：`src-tauri/tests/` 放 `migrate_test`（全量迁移 + 幂等 + 表清单 + FTS5 + WAL）、`repo_test`、`scan_test`（fixture）、`meta_test`（FakeRunner + 只读库 fixture）、`browse_test`（过滤/排序/分页/详情/搜索）、`fts_test`（同步/注入中和/回落/孤儿）、`category_test`（物化路径 CRUD/防环/防交错）、`tag_test`（词表/别名/合并/批量打标）、`smart_test`（白名单注入锁死）、`rule_test`（规则校验/打标/幂等/取消）、`thumbs_test`（缩放/mtime 新鲜度/类型与缺源报错/ffmpeg 抽帧）、`auth_test`（Argon2 往返/首管/校验/保管理员/改密/会话）、`audit_test`（顺序过滤/动作白名单/JOIN 用户名 + **命令覆盖率守护**）、`dupe_test`（部分哈希稳定性/size 分组/完整哈希切分/幂等/取消零写入）、`ocr_test`（FakeRunner 状态机/FTS 可检索/派生文件/取消零写入/原始只读）、`backup_test`（快照可查/keep 清理/restic 降级/last_backup_at/目录与钳制）、`port_test`（往返一致/祖先补建/别名冲突跳过/坏 JSON 与版本）、`autocat_test`（目录镜像层级/重跑只换 auto 不动 manual/取消与 depth=0 零写入）。

## 关键不变量

- **对原始媒体文件永远只读**：没有任何修改/删除媒体文件的命令，仅 `open_path` / `reveal_path`；删除走回收站且默认禁用。
- **本地 SQLite 为唯一真相源**，离线可用；`catalog.sqlite` 强制本地 AppData（`%LOCALAPPDATA%\com.vineturtle.vinecurator\catalog.sqlite`），**不放外置盘**（WAL 在慢盘/拔盘时会损坏）；派生数据（封面/缩略图）可迁外置。
- **迁移只增不改**：`migrations/*.sql` 按文件名升序、每文件事务内执行、失败整体回滚；要改 schema 就新增 `0005_*.sql`。
- **FTS5 无 trigger**：Rust 侧显式同步（单条增删 / 导入后整表重建），避免 trigger 链拖慢写入。
- **智能集合防注入**：`query_json` → SQL 走字段白名单 + 占位符绑定，绝不拼接用户值。
- **与 vinePress 分工边界**：vinePress = 一次性归档打包（项目 JSON 真相源、RAR/ZIP 导出）；vineCurator = 长期索引与检索（只读扫描、SQLite 索引、打标、智能集合）。第一版不耦合、不共享 crate。
- 端口 **5175**（strictPort）；identifier `com.vineturtle.vinecurator`；localStorage 键前缀 `vinecurator.prefs` / `vinecurator.layout`。
- 外部工具（ffprobe/exiftool/beets/Calibre/OCRmyPDF）缺失时降级不崩溃：`probe_tools` 探测 + 设置页手动指定。

## 当前 command 清单（`commands.rs`，全部经 `src/api.ts` 暴露）

- 环境/设置：`probe_tools`（ffprobe / ffmpeg / exiftool / beets / ebook-meta / ocrmypdf / tesseract / restic；优先读 settings 的 `*_path` 覆盖键，`ebook-meta` → `ebook_meta_path`，空值回落 PATH）、`get_settings` / `save_settings`（`settings` 表键值）、`get_db_info`（catalog.sqlite 绝对路径）
- 根目录：`list_roots`（含 item_count）、`add_root`（存在性 + 互斥/嵌套拒绝）、`remove_root`（**只删索引**：级联清 items/扩展表，磁盘文件不动）、`redirect_root`（盘符漂移重定向，条目按 rel_path 保留；路径比较走 `repo::paths_conflict` 归一化，大小写/尾分隔符等价）
- 任务：`start_scan`（后台线程扫描，emit `scan-progress`）、`start_hash_job`（后台线程懒哈希，emit `hash-progress`）、`start_meta_job`（`source = probe|beets|calibre`，后台线程元数据，emit `meta-progress`）、`cancel_job`（置位取消标志，单任务互斥 `JobState`，扫描/哈希/元数据/规则/自动分类/重复/OCR 共用）、`start_dupe_job` / `start_ocr_job` / `start_autocat_job`（见各专节）
- 浏览：`list_items`（分页 + root/type/status/category 过滤 + 排序白名单）、`list_categories`（物化路径建树 + 计数）、`get_overview`（by_type/by_status 计数）、`get_item`（条目详情：扩展表 + tags + category_ids + metadata + root_path）、`search_items`（**FTS5 MATCH 优先**（BM25 排序），无结果或语法失败回落 LIKE 通配符转义）、`set_item_fields`（status 白名单 / rating 0..=5 / json_each 批量 UPDATE + FTS 同步）、`open_path` / `reveal_path`（`cmd /C start`、`explorer /select,`，隐藏控制台窗口）
- 分类：`create_category`（parent 链接 + 物化 path + 同级重名拒绝）、`rename_category`（物化路径整段重写，含子树）、`move_category`（防环 + 新旧前缀不交错 + 冲突预检）、`delete_category`（级联清 `item_categories`，条目不动）、`set_item_categories`（整体替换归属，source='manual'）、`add_item_categories`（批量增量归类：ensure 祖先 + OR IGNORE）、`categories::ensure_path` / `validate_path_str`（物化路径补建/校验，供自动分类与规则复用）
- 标签：`list_tags`（含 item_count + aliases）、`create_tag` / `rename_tag` / `add_tag_alias`、`merge_tags`（源打标 + 同义词并入目标，源删除，受影响条目 FTS 同步）、`set_item_tags`（增量加标，先整体校验再创建，防孤儿标签）/ `remove_item_tags`（只动关联）
- 智能集合：`list_collections` / `save_collection`（保存前编译校验 query_json、名称唯一）/ `preview_collection`（返回 `(total, Vec<ItemRow>)`，白名单编译 + 占位符绑定）
- 规则：`list_rules` / `save_rule`（conditions 走 smart 白名单编译、actions 白名单 `add_tag` / `set_category`（物化路径，ensure 祖先）、名称唯一）/ `delete_rule`、`start_rule_job`（后台线程单事务跑全部启用规则，emit `rule-progress`，与其他任务共用 `JobState` 互斥；扫描/元数据导入完成后前端自动触发）
- 缩略图：`ensure_thumb`（图片内部缩放 / 视频 ffmpeg 抽帧，返回派生目录绝对路径；文件名 `<id>-<mtime_ms>.jpg`，mtime 变化即重生成并清旧文件；成功置 `has_cover=1`）
- 重复检测：`start_dupe_job`（后台线程给 size 相同的可疑条目补部分哈希，emit `dupe-progress`，与其他任务共用 `JobState` 互斥）、`list_dupes`（只读报告：`(size, partial_hash)` 相同且 ≥2 的组，组内按完整哈希切分 confirmed/待确认，部分哈希撞车剔除）
- OCR：`start_ocr_job`（`root_id: Option<i64>`，后台线程 book PDF → OCRmyPDF 派生副本 + 文本入库 + FTS 重建，emit `ocr-progress`，`JobState` 互斥；`ocr_lang` 设置键默认 eng）、`ocr_pending_count`（待识别数徽标）
- 备份：`backup_now`（`VACUUM INTO` 快照 + 可选 `restic backup` 推送，写 `last_backup_at`，同步命令 + `audit_now`）
- 导入导出：`export_port`（分类树 + 标签词表/别名 → JSON 文件写盘 + `audit_now`）、`import_port`（JSON 文件 → 增量 upsert，返回 `ImportReport` + `audit_now`）
- 用户/审计：`setup_admin`（仅 users 空时建初始管理员，成功即登录）、`session_user` / `login`（失败也入审计，不记密码）/ `logout` / `change_password`、`list_users` / `create_user` / `delete_user`（至少留一个 admin、不可删自己）/ `set_user_role`、`list_audit`（limit 钳 1..=500，可按 entity 过滤）

## 数据库与迁移

- 连接池：r2d2（max 8）+ `PRAGMA journal_mode=WAL; synchronous=NORMAL; foreign_keys=ON; busy_timeout=5000`，启动 `setup` 里打开并自动迁移。
- `0001_core.sql`：`settings`、`roots`、`items`（`root_id+rel_path` 唯一、type/status/rating CHECK、`hash_state` 懒哈希、`missing` 标记）、`categories`（物化路径）、`item_categories`、`tags`/`tag_aliases`/`item_tags`（source: manual|rule|import）、`smart_collections`、`rules`、`jobs`。
- `0002_type_ext.sql`：`music_tracks` / `video_items` / `book_items`（含 `ocr_status`）/ `comic_items`，全部 1:1 → `items`。
- `0003_fts.sql`：`fts_items` FTS5（`unicode61 remove_diacritics 2`，列 `item_id/title/body/tags_text`）。
- `0004_collab.sql`：`users` / `audit_log`（阶段五占位，第一版就建表）。
- `0005_meta_state.sql`：`items.meta_state`（none|done|failed 元数据状态机，阶段一第 5 步）。
- `0006_dupes.sql`：`items.partial_hash`（重复检测部分哈希）+ `idx_items_size` 索引（阶段四第 1 步）。
- `0007_ocr.sql`：`book_items.ocr_text`（OCR 文本入库供 FTS，写入时按 200_000 字符截断）+ `book_items.ocr_mtime`（保鲜，mtime 失配自动重跑）（阶段四第 2 步）。
- `0008_item_category_source.sql`：`item_categories.source`（manual|auto|rule，默认 manual）——自动分类重跑只动 auto 行，不降级手动/规则归类。
- `migrate_test` 锁死：全量按序应用、二次迁移幂等、表清单齐全、CHECK 约束生效、FTS5 MATCH 往返、WAL 生效 + settings upsert。

## 扫描器与哈希（阶段一第 4 步）

- `scan/mod.rs`：迭代遍历（深度上限 24，跳过 `$RECYCLE.BIN`/`System Volume Information`/Thumbs.db/desktop.ini/点文件/reparse 点）；每 500 文件一事务批量 `UPSERT ... RETURNING id`；指纹 = `size + mtime(ms)`，未变零写入，变化时 `hash=NULL, hash_state='none'`；四类（music/video/book/comic）Changed 时 `INSERT OR IGNORE` 扩展表占位行；临时表 `scan_seen` 走完整结束才标 `missing=1`（取消/中断不标）；进度 `scan-progress`（walk/finalize/finished）。
- `scan/classify.rs`：`classify_type(ext)` 白名单（flac/mp3/…→music；mp4/mkv/…→video；epub/pdf/…→book；cbz/cbr/…→comic；jpg/png/…→image；其余含 txt→other）；`ext_table(type)` 四类 → `music_tracks/video_items/book_items/comic_items`。
- `tasks.rs`：`mark_pending` 把 `hash_state='none'` 转 `pending`；`run_pending_hashes` 流式 SHA-256（64KB 分块，sha2），成功置 `hash='hex', hash_state='done'`，文件缺失回落 `none`（可重试）；每 10 条节流 `hash-progress`；已知向量 `b"hello"` → `2cf24dba…`。
- `scan_test` 5 项：四类分派 + ext 归一化 + 扩展表占位 + 增量零写入 + 缺失标记 + 取消不标 missing + rel_path 正斜杠 + 深度嵌套；哈希向量 + 缺失文件回落 none。
- `repo::get_root_path(conn, id)`：读取根目录磁盘路径（worker 用）。

## 元数据（阶段一第 5 步）

- `meta/mod.rs`：`pending_probe`（meta_state='none' + missing=0 + 四类，可选限定 root）→ `probe_root`（Runner trait 注入，缺失文件跳过保持 none，失败置 failed，取消不改状态）→ `apply_probe_json`（ffprobe/exiftool JSON → 扩展表 `COALESCE(excluded.x, 表.x)` 保留非空旧值 + `items.metadata` 落原始 JSON + `year` COALESCE 回填 + `meta_state='done'`）；进度 `meta-progress`（probe|import|finished，stats{probed,skipped,failed,imported}）。
- `meta/runner.rs`：`Runner` trait（测试用 Fake）；`ToolRunner::from_overrides` 读 settings `ffprobe_path`/`exiftool_path`；video/music → ffprobe（`-print_format json -show_format -show_streams`），book/comic → exiftool（`-j -G1`）。
- `meta/beets.rs` / `meta/calibre.rs`：只读 SQLite（`OpenFlags::SQLITE_OPEN_READ_ONLY|SQLITE_OPEN_URI`，`file:...?mode=ro`，绝不写对方库）；settings 键 `beets_db_path` / `calibre_library_path`；按 `normalize_key`（反斜杠→斜杠、折叠重复、小写，保留盘符冒号）绝对路径匹配回填 + `meta_state='done'`；Calibre 文件路径 = `library_dir/books.path/data.name.<format 小写>`，`series_index` 在 `books` 表。
- `0005` 契约：扫描指纹变化时同步重置 `meta_state='none'`。
- `meta_test` 7 项：FakeRunner 全流程（四类扩展表 + metadata JSON + year + 幂等）、失败置 failed/缺失保持 none、取消不改状态、限定 root、beets 只读导入 fixture、Calibre 只读导入 fixture、normalize_key + 扫描重置 meta_state。

## 浏览与基础 UI（阶段一第 6 步）

- `browse.rs`（纯逻辑，零 Tauri）：`list_items`（`ItemFilter` → 白名单排序列 `file_name|type|size|year|rating|status|mtime`，其余输入一律回落 `file_name`，分页 offset/limit）、`list_categories`（按物化路径建 `CategoryNode` 树 + item_count）、`get_overview`（by_type/by_status，root 作用域用参数绑定）、`get_item`（`SELECT *` 扩展表 → 动态 `ExtField` + tags + metadata JSON + root_path）、`search_items`（`like_pattern` 转义 `\ % _` + `ESCAPE '\'`）。
- `shell.rs`：`open_path` / `reveal_path`，`CREATE_NO_WINDOW` 隐藏控制台；`win_join_normalize` 先归一化——前端 `/` 拼接 root（`F:\` 结尾）+ rel 会产生盘符后双分隔符 `F:\\18138\...`，explorer `/select,` 对此回落打开「此电脑」（实测复现），折叠连续 `\` 并保留 UNC 开头；路径缺失直接报错（shell 单测 `open_rejects_missing_path` + `win_join_normalize_collapses_separators`）。
- 前端三栏：`#tree-panel`（根目录/类型/分类树，`state.sel: TreeSel`）+ `#list-panel`（`list-header` 列头点击排序、`#list-scroll` 虚拟滚动、`#list-spacer` 高度 = total×28）+ `#detail-panel`（`get_item` 详情 + 打开/定位）。
- 分块虚拟滚动：`state.ts` `CHUNK = 500`，`chunks: Map<number, ItemRow[]>`，可见区 ±8 行预取相邻块；行高 28px；搜索模式一次性取 `searchItems(query, 1000)` 后本地切块。
- 持久化：`layout.ts` 读写 localStorage `vinecurator.layout`（treeWidth/detailWidth/detailOpen/columns）；splitter 拖拽 + 双击折叠，`mouseup` 时 `saveLayout`。
- 快捷键/命令面板：`menu.ts`（`registerCommand` + `bindDeclaredShortcuts` 解析 `data-keys` + `installShortcutListener` 输入框内只放行 Ctrl+K/Ctrl+Shift+F + `togglePalette`）；`index.html` 顶栏 `data-cmd` 导航（Ctrl+1 浏览 / Ctrl+2 设置 / Ctrl+Shift+F 搜索 / Ctrl+K 命令面板），`#command-source` 存额外命令（refresh-view/cancel-job/reprobe/goto-top/reload-list）。
- `browse_test` 6 项：过滤/排序/分页、排序白名单注入安全、overview 计数、分类树嵌套计数、get_item 详情、搜索通配符转义。

## FTS5 同步与搜索高亮（阶段一第 7 步）

- `fts.rs`（纯逻辑，零 Tauri）：`sync_item`（按 `rowid = item_id` 删除重建单行；body = rel_path + year + 扩展表文本字段，tags_text = 标签名拼接）、`sync_items`（批量）、`rebuild_all`（事务内清空 + 整表重建）、`prune_orphans`（items 已删但 fts 行残留）、`ensure_consistent`（items 计数 ≠ fts 计数 → 重建，返回是否发生）、`build_match`（**每个词加引号 + 前缀 `*`，`"` 转义剥离，AND/OR/NEAR/* 全部成为字面量，绝不注入 FTS 语法**）、`search_ids`（`MATCH ?1 ORDER BY rank`，BM25 排序）。
- 同步钩子（**无 trigger，Rust 侧显式**）：`scan::flush_batch`（同事务内 changed 条目）、`meta::apply_probe_json`、`meta/beets.rs::apply_beets_row`、`meta/calibre.rs::apply_calibre_row`、`commands::remove_root`（删根后 `prune_orphans`）。
- 启动：`lib.rs setup` 迁移后 `fts::ensure_consistent`（计数不一致自动重建，日志提示）。
- `browse::search_items`：FTS5 MATCH（BM25）优先 → 无结果或语法失败回落 LIKE `ESCAPE '\'`；`rows_by_ids` 按 rank 顺序回表。
- 高亮：前端 `ui.ts::appendHighlight` 按查询词（空白切分、大小写不敏感）把命中片段包 `<mark>`（纯 text node 构建防注入）；列表 file_name 列 + 详情标题；CSS `mark` 金色底。
- `fts_test` 7 项：扫描后索引同步（文件名/多词 AND/路径分词）、操作符中和（OR/AND/NEAR/引号/col: 不报错不注入）、扩展表 + 标签可检索、rebuild/ensure_consistent 幂等、孤儿 prune、search_items FTS 优先 LIKE 回落 + 通配符转义 + 注入安全、rank 相关性排序。

## 分类、标签、集合与规则（阶段二）

- `categories.rs`（纯逻辑）：物化路径 `path`（`/` 分隔，全局唯一）；`create_category`（同级同名拒绝）、`rename_category`（子树 path 整段重写）、`move_category`（parent 链防环 + 新旧前缀不交错 + 目标冲突预检，因此可直接查唯一性）、`delete_category`（级联清 `item_categories`）、`like_prefix`（子树查询前缀）。
- `tags.rs`（纯逻辑）：`validate_tag_name`（trim/非空/禁逗号/≤60）；`resolve_tag`（精确名 → `tag_aliases` → 按需创建，`pub(crate)`）；`add_tags_to_items`（**先整体校验名称 + 条目存在，再解析创建**，避免失败调用留孤儿标签；`ON CONFLICT DO NOTHING`）、`remove_tags_from_items`（只动关联）、`merge_tags`（源打标改指目标 + `UPDATE tag_aliases` 一条语句并入同义词 + 删源）。打标后由调用方 `fts::sync_item`。
- `smart.rs`（纯逻辑，防注入锁死）：`compile_query(query_json)` → `(where_sql, args, order, limit)`；字段白名单 `type/status/rating/year/language/size/mtime/tag/category/has_cover/hash_state/ocr_status`（排序另含 `file_name/name/rel_path`）；`tag`/`category`/`ocr_status` 走 `compile_relation` EXISTS 子查询（ocr_status 在 `book_items`）；值全部 `SqlValue` 占位符绑定；limit 钳制 1..=1000。
- `rules.rs`（纯逻辑）：`validate_conditions`（复用 smart 白名单编译，数组归一化为 `{"all":[…]}` 兼容默认 `'[]'`）+ `validate_actions`（白名单 op 仅 `add_tag`，标签名走 `validate_tag_name`）；`run_rules(conn, root_id, cancel, emit)` 单事务按 `priority DESC` 跑启用规则 → `INSERT OR IGNORE ... source='rule'`（**不降级已有 manual 打标**）→ 触及条目 `fts::sync_item`；cancel 在规则/条目边界生效；进度 `rule-progress`（rule|finished）。
- 前端：`dialog.ts` 自绘对话框（`promptText`/`chooseFrom`/`confirmBox`，WebView2 无 `window.prompt`）；`features/tree.ts` 分类增删改移 + HTML5 拖拽（`text/vine-cat`，客户端防环）；`features/collections.ts` 当前筛选另存为集合（`currentQueryJson`）、详情标签增删、`mergeTagsDialog`；树区「智能集合」点击 → `state.collection` 集合预览模式（`previewCollection` 全量取回本地切块）；列表 Ctrl/Shift 多选 → `#batch-bar` 批量打标；详情面板标签 ×/输入框 + 分类 chip 归属（`setItemCategories`）；`vc:toast` / `vc:roots-changed` / `vc:selection-changed` CustomEvent 解耦模块刷新。
- 设置页「规则自动打标」卡片：表格 + 内联表单（条件/动作 JSON 文本框）、立即运行（`start_rule_job` + job 面板）；`main.ts::scheduleAutoRules` 在扫描/元数据 `done` 后延迟 600ms 自动触发（JobState 互斥则重试一次，无启用规则则跳过）。
- 测试：`category_test` 7 项、`tag_test` 6 项、`smart_test` 6 项（含 `values_never_interpolate_into_sql` + 示例集合播种幂等）、`rule_test` 6 项（白名单校验注入拒绝 / 打标 source + FTS / 幂等 + manual 优先 / 停用与 root 范围 / 取消零写入 / 无规则 finished）。**注意**：并行测试必须用 `AtomicU64` 唯一临时目录，否则共享 DB 报 `database is locked`。

## 缩略图（阶段二第 5 步）

- `thumbs.rs`（纯逻辑，零 Tauri，依赖 `image` crate）：`derived_root`（settings `derived_dir`，空则回落 `<AppData>/derived`，建 `thumbs/`）→ `ensure_thumb`（文件名 `<item_id>-<mtime_ms>.jpg`：存在即复用；变了重生成 + `cleanup_stale` 清同条目旧 mtime 文件）；`image` → `thumbnail(320,320)` 存 JPEG；`video` → ffmpeg `-ss 3`（失败回落 `-ss 0`）抽帧 `scale=320:-2`，`CREATE_NO_WINDOW`；其余类型报「暂不支持」；成功置 `items.has_cover=1`；`ffmpeg_program`（settings `ffmpeg_path` 覆盖，空回落 PATH `ffmpeg`）；`thumb_exists` / `existing_thumb` 只查不生成。
- 资产协议：`tauri.conf.json` `assetProtocol.enable = true`（scope 空，动态追加）；`tauri` feature `protocol-asset`；`lib.rs setup` 与 `save_settings`（`derived_dir` 变更时）调 `app.asset_protocol_scope().allow_directory(thumbs_dir, true)`；前端 `convertFileSrc(path)` → `<img>`。
- 前端：详情面板 image/video 类型显示 `.thumb-holder`（首见「生成缩略图…」→ `ensure_thumb` → 图片；失败显示错误文本；`detailBody.dataset.itemId` 防切换条目后回填）。
- `thumbs_test` 4 项：缩放 ≤320 + has_cover + 复用不重生成 + ffmpeg override 键；mtime 变化重生成 + 清旧 + exists/existing 查询；epub 不支持 / 缺源 / 无条目报错；本机有 ffmpeg 时生成 2 秒测试片抽帧（造不出则跳过不误报）。

## 三视图联动（阶段二第 6 步）

- 模式优先级（`ensureChunk`）：**搜索 > 智能集合预览 > 树筛选**；搜索/集合为一次性取回客户端切块（`sliceIntoChunks`），表头点击走 `applyLocalSort` 本地排序；树节点点击会清空搜索框（否则搜索优先导致树点击看似无效）。
- `state.ts` 拆分：`resetChunks()` 只失效数据块（保留多选），`resetList()` 额外清选中；`refreshAll` 先 `resetChunks`（否则 chunks 命中早退，扫描/规则/打标后列表不更新），批量打标后同样失效重拉 + 刷详情（智能集合成员可能变化）。
- 详情面板状态/评分可编辑：下拉改状态、点 1-5 星改评分（再点当前星清零）→ `set_item_fields` → 本地修补列表行 + `renderViewport`；状态变更额外刷 `getOverview`（树区 by_status 计数联动）。
- `browse.rs sort_column` 白名单含 `year`/`rating`（与 `smart.rs` 排序白名单一致）；`browse_test::sort_by_year_and_rating_columns` 锁死。

## 用户、登录与审计（阶段三）

- `auth.rs`（纯逻辑）：Argon2id 哈希（`hash_password`/`verify_password`，PHC 格式随机盐）；`setup_admin`（**仅 users 为空**可建初始管理员）/ `create_user`（用户名 trim ≤64 禁空白、密码 ≥6 位、role 白名单 `admin|editor|viewer`、重名拒绝）/ `authenticate`（失败不区分用户不存在/密码错误）/ `delete_user` + `set_role`（**至少保留一个管理员**）/ `change_password`（校验旧密码）；`AuthState`（`Mutex<Option<UserRow>>` 进程内存会话，`login/logout/current/user_id`，poison 自行恢复）。**角色门禁本期不接入命令**，留给远端接口阶段（见 `collab/README.md`）。
- `audit.rs`（纯逻辑）：`ACTIONS` 白名单 `create|update|delete|run|cancel|login|logout|setup|password`；`record`（action 白名单 + 非空 entity，detail 必须 JSON 字符串且**绝不含密码**）；`recent`（LEFT JOIN users 取用户名、按 entity 过滤、limit 钳 1..=500、id 倒序）。
- 插桩约定：同步命令用 `commands.rs::audit_now(conn, auth, action, entity, entity_id, json!())?`；后台任务启动时捕获 `auth.user_id()`，线程收尾用 `audit_job`（`action='run'`, entity=`job`, detail `{kind, root_id, source, status: done|cancelled|failed}`），**按任务汇总不逐文件刷屏**；`cancel_job` 记 `action='cancel'`。登录顺序注意：先 `auth.login` 再 `audit_now`（否则 user_id 落 NULL）。
- **覆盖率守护**（`audit_test::every_command_is_classified_and_write_commands_audit`）：解析 `commands.rs` 全部 `#[tauri::command]`，每个必须归入 `MUTATING`（39 个，函数体含 `audit_now|audit::record|audit_job`）或 `READONLY`（20 个，不得写审计）——**新增命令必须显式分类**；`all_commands_registered_in_handler` 保证 lib.rs invoke_handler 与命令一一对应。
- 前端：设置页「用户与登录」面板（首启 `setup-admin-form`、登录/退出/改密、建用户、行内改角色、删用户走 `confirm`）+「审计日志」面板（500 条倒序、entity 下拉过滤、手动刷新）；`main.ts setView('settings')` 时 `refreshCollabView()`；`api.ts` 封装 `setupAdmin/sessionUser/login/logout/changePassword/listUsers/createUser/deleteUser/setUserRole/listAudit`。
- `collab/README.md`：远端接口（axum/Tailscale/角色门禁/JWT/同步策略）设计占位，阶段三不实现。
- Cargo：`argon2 = "0.5"` + `password-hash = { version = "0.5", features = ["getrandom"] }`（开 `rand_core::OsRng` 供 `SaltString::generate`，与 argon2 共用同一实例做 feature 统一）。
- `auth_test` 7 项、`audit_test` 5 项（含覆盖率守护）；**迁移与示例集合播种属启动引导，不计审计**（其余全部写操作落账）。

## 重复检测（阶段四第 1 步）

- `dupes.rs`（纯逻辑，零 Tauri）：`compute_partial(path, size)` = sha256(size.to_le_bytes() ‖ 头 1MiB ‖ 尾 ≤1MiB)（小文件顺序读全量、头尾不重叠；同长同头尾不同中间会撞 → 只报告，确认仍靠完整哈希；size 必须为实际大小，失真报错可重试）；`pending_partial`（size 重复且 missing=0 且 partial_hash IS NULL，`idx_items_size` 加速）；`run_partial_hashes`（逐条写 partial_hash，文件缺失/根缺失 failed++ 不写，取消在条目边界生效，每 10 条节流 emit `dupe-progress`）；`report` → `Vec<DupeGroup>`（`(size, partial_hash)` 分组 count≥2 → 组内按 `hash` 分桶：完整哈希一致桶 = confirmed、无哈希桶 = 待确认、单成员桶剔除，按 size 降序）。
- 0006 契约：只增列不动旧数据；`migrate.rs` `MIGRATIONS` 数组须同步加条目（硬编码，include_str 不会自动发现）。
- 命令：`start_dupe_job`（JobState 互斥 + 线程收尾 `audit_job(kind=dupe)` + `DupeProgress::failure` 兜底崩溃）、`list_dupes`（READONLY）。
- 前端：设置页「重复检测」面板（补部分哈希 → `vc:begin-job`、刷新报告、组卡片显示 已确认/待确认 + 定位按钮走 `list_roots` 拼根路径）；`main.ts onDupeProgressEvent`（done/cancelled → `vc:dupes-done` → settings 刷报告）；样式 `.dupe-group/.dupe-badge/.dupe-item`。
- `dupe_test` 4 项：`compute_partial_hash_properties`（确定性/内容区分/size 失真报错/大文件跳中段稳定）、`pending_requires_size_groups_and_skips_missing_or_done`（仅 size 组、算过不重复、missing 整组排除）、`report_splits_by_full_hash_and_confirms`（confirmed 切分/撞车剔除/待确认组）、`run_is_idempotent_and_cancel_writes_nothing`（取消零写入/跑完幂等）。

## OCR 队列（阶段四第 2 步）

- `ocr.rs`（纯逻辑，零 Tauri）：`OcrRunner` trait（测试 Fake 注入）；生产 `OcrmypdfRunner::from_overrides`（settings `ocrmypdf_path` 覆盖，`--quiet -l <lang> --sidecar`，`CREATE_NO_WINDOW`）；`pending_ocr`（book + `ext='pdf'` + missing=0 + 状态 `none|failed`，或 `done` 但 `ocr_mtime ≠ items.mtime` 指纹失配重跑，可选 root 作用域）；`run_ocr_queue`（逐条 claim `queued` → runner 只写派生目录 `<derived>/ocr/<id>-<mtime_ms>.{pdf,txt}` → sidecar 读回截断 200_000 字符入 `ocr_text` + `ocr_mtime` + `ocr_status='done'` → `fts::sync_item`；失败置 `failed` 可重试；取消在条目边界生效、claim 前检查 → 取消零写入；进度 `ocr-progress` phase queue|finished）。
- **原始媒体只读**：ocrmypdf 只写输出路径，不碰输入；`ocr_test::cancel_writes_nothing_and_originals_stay_readonly` 断言原始文件字节不变、原始目录无派生文件。
- FTS：`fts.rs::sync_select` book 分支 body 追加 `COALESCE(b.ocr_text,'')`——扫描后 OCR 过的书全文可检索（`fts_test` 不受影响：ocr_text 为 NULL 时行为不变）。
- `thumbs::derived_base`（新增）从 `derived_root` 拆出：settings `derived_dir` 回落 AppData，各派生子系统（thumbs/ocr）各自 join 子目录。
- 状态机（0002 CHECK）：`none → queued → done | failed`；`queued` 为运行中瞬态。
- 命令：`start_ocr_job`（JobState 互斥 + `audit_job(kind=ocr)` + `OcrProgress::failure` 兜底崩溃）、`ocr_pending_count`（READONLY）。前端：设置页「OCR 队列」面板（待识别徽标 + 开始/刷新）；`main.ts onOcrProgressEvent`（done/cancelled → `vc:ocr-done` → 刷徽标）；工具缺失由 ocrmypdf 启动失败计入 failed 不崩溃。
- `ocr_test` 4 项：`pending_filters_pdf_books_and_states`（仅 pdf 书/新鲜 done 排除/mtime 失配重跑/missing 排除/root 作用域）、`fake_run_marks_done_fts_and_derived_files`（done + ocr_text + mtime + 派生文件 + FTS 命中 + 幂等）、`failure_marks_failed_and_stays_pending`（failed 可重试）、`cancel_writes_nothing_and_originals_stay_readonly`。

## 自动分类（目录结构镜像）

- `autocat.rs`（纯逻辑，零 Tauri）：`desired_path(rel_path, depth)` = 父目录取前 N 层文件夹（根下文件 → None；depth=0 → 关闭）；`run(conn, root_id, depth, cancel, emit)` 单事务逐条对齐——`categories::ensure_path` 建/复用物化路径 → desired 写 `item_categories source='auto'`（OR IGNORE），desired 之外的旧 `auto` 关联删除，**manual/rule 关联永不触碰**；取消在条目边界生效、每 200 条节流 emit `autocat-progress`（phase queue|finished）。
- depth 来源：settings `auto_cat_depth`（`start_autocat_job` 线程内读取，缺省 1，`parse` 失败回落 1，`.min(5)` 钳制；0 = 关闭立即 finished）。执行顺序约定：**扫描 done → 自动分类 → 规则**（`main.ts scheduleAutoClassify`：先 `start_autocatJob`，`onAutocatProgressEvent` 终态后 `scheduleAutoRules`；互斥占用则直接跳规则），规则条件可用 `category` 过滤形成组合能力。
- 命令：`start_autocat_job`（JobState 互斥 + `audit_job(kind=autocat)` + `AutoCatProgress` 崩溃兜底）、`add_item_categories`（批量增量归类：整体校验条目存在 → ensure 祖先 → OR IGNORE，source='manual'，`audit_now` entity=item_categories）。
- 前端：设置页「自动分类」面板（层级输入 0..5 + 保存 `auto_cat_depth` + 立即整理）；列表多选批量条 `#batch-cat` + 归类按钮（`addItemCategories` → toast + `vc:roots-changed` 刷树）；`main.ts onAutocatProgressEvent`（job-chip 文案、终态 refreshAll + 链式触发规则）。
- **`item_categories.source`**（0008）三态语义：`manual` 用户手挂 / `auto` 目录镜像 / `rule` 规则动作；重跑各自只动自己来源的行，`set_item_categories` 整体替换后一律记 manual。
- `autocat_test` 4 项 + `rule_test set_category` + `categories::ensure_path` 单测 + `migrate_test` 0008 断言。

## 备份（阶段四第 3 步）

- `backup.rs`（纯逻辑，零 Tauri）：`snapshot(conn, dir, keep)` = `VACUUM INTO '<dir>/catalog-<epoch_ms>.sqlite'` 在线一致副本（含 FTS；路径单引号转义；同 ms 碰撞追加 `-N`）→ `prune` 按 keep 清最旧（key = (ms 字符串, 后缀序号) 排序，keep 钳 ≥1，单个删除失败不阻断）；`run_backup(conn, default_derived, restic: Option<&dyn ResticRunner>)` → `BackupReport { snapshot_path, kept, restic, restic_skipped }` + 写 `last_backup_at`（epoch ms）。
- 配置（settings 键）：`backup_dir`（空 → `<derived>/backups`，复用 `thumbs::derived_base`）、`backup_keep`（默认 7，非法回 7、0 钳 1）、`backup_interval_hours`（0=仅手动）、`restic_repo`（空 → 跳过且不调 runner）、`restic_password_file`；`CliRestic::from_overrides` 读 `restic_path` 覆盖。
- `ResticRunner` trait（测试 Fake 注入）；生产 `restic --quiet backup <dir> --repo <url> [--password-file f]`（`CREATE_NO_WINDOW`）；**降级链**：未配置 → 跳过；restic 缺失/失败 → `restic_skipped` 带原因，本地快照始终保留。
- 命令：`backup_now`（同步 + `audit_now` action=create entity=backup，detail 含 snapshot/kept/restic）。前端设置页「备份」面板（间隔/保留数/快照目录/restic 仓库与密码文件 + 立即备份 + 上次备份徽标）；`settings.ts applyBackupSchedule`（interval>0 时启动即查 + 每 15 分钟巡检 `last_backup_at` 到点自动跑，`runBackup` 单飞防重入，silent 模式仅 restic 失败告警）。
- `backup_test` 3 项：`snapshot_creates_queryable_copy_and_prunes`（副本可独立打开 + 数据在 + keep 清理）、`run_backup_restic_fallback_and_last_backup_at`（成功/失败降级/未配置不调 runner + last_backup_at）、`default_dir_resolution_and_keep_clamping`（默认目录回退 + keep 非法/0 钳制）。
- 注意：`prune` 排序对同 ms 后缀必须 base 在前（`-1` 在后），否则会把最新份当最旧删掉——`snapshot_files` 的 key 已锁死；测试在 Windows 上需及时 drop 打开的快照连接（文件占用会跳过删除）。

## 标签 / 分类导入导出（阶段四第 4 步）

- `port.rs`（纯逻辑，零 Tauri）：`PortDoc { version=1, exported_at, categories[{path,name,sort_order}], tags[{name,aliases[]}] }`；`export_doc`（`categories` 按 path 序 + `tags` LEFT JOIN 别名收拢 BTreeMap）；`import_doc`（单事务：分类按段数升序 → `upsert_category` 同 path 查在场（在场不动名/序，缺失按 `split_path` 递归补建祖先，path 段逐个 `validate_name`）、标签 `validate_tag_name` 按名 upsert、别名归属校验（已属他标签 → 跳过），非法项进 `ImportReport.skipped` 不中断整体）；`import_json`（serde 解析 + `version > 1` 拒绝）。
- **不触碰 `item_tags` 关联**（词表/树增删不影响已有条目 FTS 的 tags_text）→ 无需 FTS 重建；全部参数绑定，`a//b`、空 path、带逗号标签名等恶意/非法输入被校验跳过。
- 命令：`export_port(path)`（JSON pretty 写盘 + `audit_now` create/export）、`import_port(path)`（读盘导入 + `audit_now` create/import，detail = ImportReport）。
- 前端：设置页「标签 / 分类导入导出」面板（`plugin-dialog` `save()`/`open()` 选路径，导入完成 toast 报告 + `vc:roots-changed` 刷树）。
- `port_test` 3 项：`roundtrip_export_import_into_fresh_db`（往返一致（忽略 exported_at）/幂等零新增）、`import_creates_missing_ancestors_and_skips_invalid`（a/b/c 仅叶条目 → 祖先补建 + parent 正确 + sort 更新计数 + 非法路径/标签名/占用别名跳过 + 注入安全）、`json_parse_and_version_errors`。

## 开发状态

- [x] 阶段一第 1-2 步：脚手架（5175 / identifier / AGENTS.md）+ db 连接池 + 迁移器 + 0001~0004 + 测试全绿（2026-10-09）
- [x] 阶段一第 3 步：设置页（多根目录 CRUD/重定向/删除只清索引、派生目录、数据库路径展示、工具路径手动指定）+ `repo_test` 6 项（2026-10-09）
- [x] 阶段一第 4 步：扫描器（四类分派、upsert、增量跳过、进度事件、`scan_test` 5 项）+ 后台哈希任务（`start_hash_job` + `hash-progress`）（2026-10-09）
- [x] 阶段一第 5 步：元数据（`meta/` ffprobe/exiftool 探测 + beets/Calibre 只读导入 + `meta_state` + `start_meta_job`/`meta-progress` + 设置页元数据面板）+ `meta_test` 7 项（2026-10-09）
- [x] 阶段一第 6 步：基础 UI（三栏树/列表/详情、分块虚拟滚动、列头排序、layout.ts 持久化、Ctrl+K 命令面板 + Ctrl+Shift+F 搜索、`browse.rs` + `shell.rs` + `browse_test` 6 项）（2026-10-09）
- [x] 阶段一第 7 步：FTS5 同步与搜索高亮（`fts.rs` 显式同步 + 启动一致性检查 + `search_items` FTS 优先 + `appendHighlight` + `fts_test` 7 项）（2026-10-09）
- [x] 阶段二第 1-3 步：分类树 CRUD/拖拽 + 标签（词表/别名/合并/批量打标）+ 智能集合（保存/预览/多选批量条/三个示例集合首次播种）+ `category_test` 7 / `tag_test` 6 / `smart_test` 6 + 前端 dialog/tree/collections（2026-10-09）
- [x] 阶段二第 4 步：规则自动打标（`rules.rs` 白名单校验 + `start_rule_job` 单事务后台执行 + `rule-progress` + 设置页规则卡片 + 扫描/导入后自动触发）+ `rule_test` 6 项（2026-10-09）
- [x] 阶段二第 5 步：缩略图（`thumbs.rs` 图片缩放 320 / ffmpeg 抽帧 + mtime 新鲜度 + `ensure_thumb` 命令 + asset 协议白名单 + 详情面板展示 + `thumbs_test` 4 项）（2026-10-09）
- [x] 阶段二第 6 步：三视图联动收尾（搜索/集合/树优先级 + 本地排序 + refreshAll 失效列表 + 状态/评分可编辑联动树计数 + `browse_test` 92 项全绿）（2026-10-09）
- [x] 阶段三：本地协作数据就绪（`auth.rs` Argon2 用户/角色/会话 + `audit.rs` 全写操作审计 + 覆盖率守护测试 + 设置页用户/审计面板 + `collab/` 设计占位 + `auth_test` 7 / `audit_test` 5，cargo 104 项全绿）（2026-10-09）
- [x] 阶段四第 1 步：重复检测（`0006_dupes` partial_hash + size 索引、`dupes.rs` 部分哈希/分组报告、`start_dupe_job`/`list_dupes` + `dupe-progress`、设置页面板 + 定位/确认徽标、`dupe_test` 4 项，cargo 108 项全绿）（2026-10-09）
- [x] 阶段四第 2 步：OCR 队列（`0007_ocr` ocr_text/ocr_mtime、`ocr.rs` OcrRunner 注入 + 状态机 none→queued→done|failed + mtime 失配重跑、`start_ocr_job`/`ocr_pending_count` + `ocr-progress`、FTS body 并入 ocr_text、设置页面板 + 待识别徽标、`ocr_test` 4 项，cargo 112 项全绿）（2026-10-09）
- [x] 阶段四第 3 步：备份（`backup.rs` `VACUUM INTO` 快照 + keep 清理 + `ResticRunner` 降级链 + `last_backup_at`、`backup_now` 命令、设置页面板 + 应用内定时器（间隔小时/15 分钟巡检）、restic 探测入 `probe_tools`、`backup_test` 3 项，cargo 115 项全绿）（2026-10-09）
- [x] 阶段四第 4 步：标签/分类 JSON 导入导出（`port.rs` PortDoc 往返 + 祖先补建 upsert + 别名冲突跳过 + 版本校验、`export_port`/`import_port` 命令、设置页面板走 dialog save/open、Meilisearch 不做——FTS5 够用、`port_test` 3 项，cargo 118 项全绿）（2026-10-09）
- [x] 阶段四第 5 步：Syncthing 多设备 / vinePress 联动 → 设计定稿 `docs/sync.md`（catalog.sqlite 绝不同步、派生/备份/PortDoc 词表可同步、词表交换 = export→同步→import（last-import-wins）、vinePress 第一版零代码耦合；0 新代码、门槛沿用 118 项全绿）（2026-10-09）
- [x] UI 完善批次：列表键盘导航（↑↓/PgUp/PgDn/Home/End 移动选中 + Ctrl/Shift 扩展多选 + Enter 打开，焦点在 `#list-scroll` 时生效）+ 行右键菜单（打开/定位/复制路径，`ctx-menu` fixed 定位、点外/Esc/滚动关闭）+ 首次无根目录空态 CTA（`vc:goto-roots` → 设置页根目录面板）+ 批量设状态（多选条 + 行本地修补 + 树 by_status 刷新）+ 顶栏账号 chip（`vc:user-changed` 由登录/退出/建管理员广播）+ 设置页 sticky 分区导航；纯前端改动，cargo 118 项不变、npm 三关全绿（2026-10-09）
- [x] UI 现代化批次：Shift 范围连选（`state.anchorId` 锚点，鼠标 Shift 点击 + Shift 方向键，Ctrl 仍单条切换）+ 行 hover 快捷操作（打开/定位 SVG 按钮 `.row-actions`）+ 详情「复制路径」+ CSS 现代化（散落硬编码色全部 token 化——新增 `--accent-soft/--accent-mid/--line-3/--overlay/--warn/--mark-*` 等变量；细滚动条、`:focus-visible` 焦点环、交互微动效、药丸导航选中态、列表选中 accent 竖条标记、toast 滑入/菜单弹出动画、`prefers-reduced-motion` 无障碍）+ 深浅主题切换（顶栏 `#theme-toggle` → `html[data-theme='light']` 覆盖 `:root` 变量组，localStorage 键 `vinecurator.prefs.theme`，首次跟随系统 light 偏好，`color-scheme` 同步）；cargo 118 项 + npm 三关全绿（2026-10-09）
- [x] 任务体验批次：顶栏全局任务指示器 `#job-chip`（`beginJob`/`updateJob`/`endJob` 三态同步——脉冲圆点 + 截断文案 + 56px 微进度条，完成转绿保留 3 秒，点击跳设置页任务面板；浏览页跑任务不再无反馈）+ 不定进度流光（`updateJob(text, pct?)`：total 未知时 indeterminate 扫光动画，总未知不再死在 0%）+ 扫描 walk 阶段真实百分比 + 五类进度 handler 直写 `jobStatus` 的 5 处全部收口到 `updateJob`（chip 与面板永远一致）；纯前端，cargo 118 项 + npm 三关全绿（2026-10-09）
- [x] 封面网格视图批次：toolbar 分段切换 `#view-toggle`（列表⇄网格）+ 网格模式紧凑排序（`#grid-sort`/`#grid-sort-dir`，与列头同源 `COLUMNS`，`applySort` 抽出共用，列头隐藏）+ 虚拟滚动网格（列数按容器宽度算、`.gcard` 200px = 封面 120（type 占位 → image/video 封面 `object-fit:contain`）+ 文件名两行 + 类型·年份·评分·大小 meta）+ 封面懒加载（`ensureThumb` 按 `id:mtime` 缓存 + inflight 去重，仅 image/video）+ 交互全复用（`wireRow` 单选/Ctrl/Shift 范围/双击/右键、`buildRowActions` 悬浮打开/定位、批量条/详情联动）+ 网格键盘（↑↓ 跨列 ±cols、←→ 逐卡、Page 按行）+ `state.viewMode` 持久化（`LayoutPrefs.viewMode`，`vc:view-mode` → `saveLayout`）；**顺带修复潜在冻结死循环**：`ensureChunk` 改 `Promise<boolean>`（是否新拉数据）+ `chunkLoads` in-flight 等待图——旧 `.then(() => renderViewport())` 对已缓存块会无限重排微任务，且预取进行中 `selectIndex` 早退丢行；cargo 118 项 + npm 三关全绿（2026-10-09）
- [x] 拖放与预览批次：拖放文件夹 → 直接添加根目录（main.ts `initDropTargets` 订阅 `getCurrentWebview().onDragDropEvent`：enter/over 显示 `#drop-overlay` 提示、leave 隐藏、drop 逐个 `addRoot`——重复/非目录/混拖容错，按成功数 toast + `vc:roots-changed`；浏览器 dev 环境无事件静默）+ 媒体预览灯箱 `#lightbox`（详情缩略图点击 / 行右键「预览」，仅 image/video：图片走**原片**、视频 `<video controls>` 原片 + `ensureThumb` 封面兜底，mkv 等 WebView2 解不了的编码回落封面提示；Esc/点击遮罩/✕ 关闭，文案单行省略号）+ **asset 协议放开根目录**（`lib.rs setup` 启动时全量 `allow_directory`，`add_root`/`redirect_root` 命令成功后追加——灯箱只读播原片的前提；`--overlay-strong` 新 token，`#drop-overlay/#lightbox` 显式 `[hidden]{display:none}` 防 `display:grid/flex` 盖过 UA 默认）；cargo 118 项 + npm 三关全绿（2026-10-09）
- [x] 事件链修复批次：跨模块 `vc:*` CustomEvent 统一 **window↔window**（ui.ts 9 处 `document.dispatchEvent` → window + main.ts `vc:selection-changed` 监听 document → window；`CustomEvent` `bubbles:false`，document→window 永不送达——修复 CTA「添加根目录…」/`vc:toast`/`vc:view-mode` 死链）+ 根表单空路径自动弹 `pickDir` 选择器（用户报「点击添加根目录没动静」）  + `pickDir` 内部 try/catch 统一兜底 4 调用点 + `initDropTargets` 对 `getCurrentWebview()` 同步 try/catch（浏览器 dev 防抛）；cargo 118 项 + npm 三关全绿（2026-10-09）
- [x] 定位与自动分类批次：**定位修复**（`shell.rs reveal_path` 统一反斜杠——`explorer /select,` 对前端正斜杠路径不选中，路径/打开不受影响）+ **目录结构镜像自动分类**（`0008_item_category_source` 加 `item_categories.source`；`autocat.rs` desired_path 前 N 层文件夹 → ensure_path 建树 + `source='auto'` 幂等对齐，重跑只动 auto 不碰 manual/rule；扫描后 **自动分类 → 规则** 链式触发）+ 规则新动作 `set_category`（物化路径 ensure 祖先，`RuleStats.categorized`）+ 批量归类（`#batch-cat` → `add_item_categories`）+ 设置页「自动分类」面板（`auto_cat_depth` 0..5，缺省 1）；`autocat_test` 4 项 + rule/migrate/categories/shell 补测，cargo **127 项** + npm 三关全绿（2026-10-09）
