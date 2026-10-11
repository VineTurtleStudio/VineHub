# vineCurator — 本地媒体资产管理器

本地优先的媒体库索引与策展工具：统一管理约 **1.1TB** 本地媒体（500G+ 音乐、400G+ 视频、200G+ 电子书/图册/扫描本），支持层级分类、标签、智能集合与全文检索。

> 架构与 `vinePress` 一致：**Tauri 2.x + Rust + 零框架 TypeScript + Vite**。完整开发约定见 [AGENTS.md](./AGENTS.md)（开工时创建，照抄 vinePress 骨架）。

---

## 1. 项目定位与命名

| 项 | 值 |
|---|---|
| productName / 目录名 | `vineCurator`（camelCase，对齐 `vinePress`） |
| package.json name | `vine-curator` |
| identifier | `com.vineturtle.vinecurator` |
| dev 端口 | **5175**（5173 VinePalmaCursor、5174 vinePress，strictPort） |
| localStorage 键前缀 | `vinecurator.prefs` / `vinecurator.layout` |
| 数据库 | `catalog.sqlite`（本地 AppData，**不放外置盘**：WAL 在慢盘/拔盘时会损坏） |

### 与 vinePress 的分工边界（关键不变量）

- **vinePress** = 一次性归档打包（项目 JSON 为真相源、生成说明文件、RAR/ZIP 导出）——不动。
- **vineCurator** = 长期索引与检索（1.1TB **只读**扫描、SQLite 索引、打标、智能集合）。
- vineCurator 对原始文件**永远只读**：没有任何修改/删除媒体文件的命令，仅有 `open_path` / `reveal_path`；删除走回收站且默认禁用。
- 第一版不耦合 vinePress：`scan/rules.rs` 9 类判定、`meta.rs` 封面提取、`dupes.rs` 两级查重以“抄实现/抄测试”方式移植，不共享 crate。

---

## 2. 核心设计理念

- **存储与检索分离**：原始文件在外置硬盘，元数据与索引在 SQLite。
- **层级分类 + 扁平标签 + 智能集合**：稳定导航与多维发现互补。
- **本地优先**：桌面端即时响应，一切以本地 SQLite 为真相源，离线可用。
- **不重复造轮子**：集成 ffprobe、ExifTool、beets、Calibre、OCRmyPDF 等成熟工具。
  - ffprobe / exiftool 走 subprocess（`-json`）；**beets、Calibre 直接只读打开其库文件**（`file:...?mode=ro`），不 shell 调用，更快更稳。

## 3. 技术栈

| 层 | 选型 |
|---|---|
| 桌面框架 | Tauri 2.x（Rust 后端） |
| 前端 | **零框架 TypeScript + Vite**（不引 React/Tailwind/shadcn，分层对齐 vinePress） |
| 数据库 | SQLite 3 + FTS5（rusqlite + r2d2 连接池，WAL） |
| 任务队列 | 后台线程 + JobState 单飞 + 进度事件（`scan-progress` / `hash-progress` / …） |
| 协作（阶段五，暂缓） | axum + Tower，Tailscale 绑定，角色门禁（见 `collab/README.md`，不在本期） |
| 备份 | 元数据 `VACUUM INTO` 快照（`backup.rs`）+ 可选 restic 推送 |
| 外部工具 | ffprobe、ffmpeg、ExifTool、beets、Calibre、OCRmyPDF + Tesseract |

---

## 4. 对原始需求的修正与补强

1. **多根目录**：`roots` 表（外置硬盘可多个、可换盘符），`items` 存 `root_id + rel_path`；盘符变了在设置里重定向 root，索引不废。
2. **补 item↔category 关联**：原 items 表无 category_id → 新增 `item_categories` 多对多。
3. **哈希懒计算**：1.1TB 全量 SHA-256 首轮会跑几小时 → 首轮只存 `size+mtime` 指纹（`hash_state='none'`），完整哈希进后台按需补算；查重用“size 分组 → 再哈希”两级策略。
4. **FTS5 无 trigger**：tag/title 常变、trigger 链拖慢写入 → 独立 FTS 表 + Rust 侧显式同步（单条增删 / 导入后整表重建）。
5. **智能集合防注入**：`query_json` → SQL 走字段白名单 + 占位符绑定，绝不拼接用户值。
6. **派生数据隔离**：缩略图写 `<derived_dir>/thumbs/<item_id>-<mtime_ms>.jpg`（OCR 副本 `<derived_dir>/ocr/`），可整目录删除重建，不污染媒体库。
7. **schema 占位前置**：`users` / `audit_log` / `jobs` 第一版就建表（迁移只增不改），协作阶段零迁移成本。
8. **Windows 约定继承**：`npm.cmd`、PowerShell 显式 `-Encoding UTF8`、`cargo fmt` + `npm run lint`。

---

## 5. 目录结构（目标形态）

```
vineCurator/
├── src-tauri/
│   ├── src/
│   │   ├── main.rs / lib.rs
│   │   ├── commands.rs          # 薄桥接：#[tauri::command] + emit + JobState 单飞
│   │   ├── db/                  # 连接池、migrate.rs（顺序迁移）、repo.rs（仓储）
│   │   ├── scan/                # 纯逻辑：遍历分派、fingerprint、upsert
│   │   ├── meta/                # 纯逻辑：ffprobe / exiftool / beets / calibre
│   │   ├── categories.rs / tags.rs / smart.rs / rules.rs   # 分类、标签、白名单 SQL、规则打标
│   │   ├── browse.rs / fts.rs / shell.rs                   # 查询、FTS5 同步、打开/定位（只读）
│   │   ├── tasks.rs / thumbs.rs / dupes.rs / ocr.rs / backup.rs / port.rs   # 后台任务与派生
│   │   ├── auth.rs / audit.rs / env.rs / autocat.rs           # 本地用户会话、审计、工具探测、目录镜像自动分类
│   ├── migrations/              # 0001_core … 0008_item_category_source（只增不改）
│   ├── tests/                   # 18 个测试文件（migrate/repo/scan/meta/browse/fts/…/port/autocat）
│   ├── Cargo.toml
│   └── tauri.conf.json          # devUrl 5175
├── src/
│   ├── index.html               # 快捷键唯一来源 data-keys
│   ├── main.ts                  # 事件接线
│   ├── api.ts                   # 唯一 invoke 出口
│   ├── state.ts / ui.ts / layout.ts / menu.ts / settings.ts
│   ├── features/                # tree（分类树）/ collections（集合与打标）
│   └── styles/                  # 原生 CSS（对齐 vinePress 视觉）
├── collab/                      # 阶段五远端协作设计占位（README）
├── docs/                        # architecture.md、migrations.md、sync.md
├── package.json                 # dev/build/tauri/lint/fmt
├── vite.config.ts               # port 5175 strictPort
├── tsconfig.json
└── README.md
```

---

## 6. 数据库设计

迁移规则：`migrations/*.sql` 按文件名升序、每文件事务内执行、`schema_migrations(version, applied_at)` 记账；首次启动自动跑；**只增不改**（要改就新增下一个编号，当前 `0001`–`0007`）。

### 核心表（0001_core.sql 摘要）

- `settings` — 键值配置
- `roots` — 媒体根目录（可多个，`path` 唯一，支持盘符重定向）
- `items` — 统一条目：`root_id + rel_path` 唯一；`type/music|video|book|comic|image|other`；`size+mtime` 指纹；`hash + hash_state(none|pending|done)` 懒哈希；`status`（inbox/unwatched/reading/done/dropped）、`rating 0..5`、`review` 复核标记、`metadata JSON`
- `categories` — 层级分类，物化路径 `path`（`/` 分隔，根级 `音乐`、子级 `音乐/爵士`，无前导 `/`），子树枚举 `LIKE path||'/%'`
- `item_categories` — 条目 ↔ 分类 多对多
- `tags` / `tag_aliases`（同义词）/ `item_tags`（`source: manual|rule|import`）
- `smart_collections` — `query_json`（`{filter, sort, limit}`）
- `rules` — 条件 → 动作的自动打标规则（`conditions`/`actions` JSON，含 priority）
- `jobs` — 后台任务队列（`kind/status/progress/error`）

### 类型扩展表（0002_type_ext.sql，全部 1:1 → items）

- `music_tracks`：artist、albumartist、album、disc、track、duration、format、bitrate、mbid（专辑用 `albumartist+album` 分组查询，暂不建 albums 表）
- `video_items`：series、season、episode、resolution、codec、duration、tmdb_id
- `book_items`：author、series、volume、isbn、publisher、pages、format、`ocr_status(none|queued|done|failed)`
- `comic_items`：series、volume、page_count、scan_quality

### 检索与协作（0003 / 0004）

```sql
-- 0003_fts.sql
CREATE VIRTUAL TABLE fts_items USING fts5(
  item_id UNINDEXED, title, body, tags_text,
  tokenize = 'unicode61 remove_diacritics 2'
);

-- 0004_collab.sql（阶段五占位）
users(id, username UNIQUE, password_hash/*Argon2id*/, role admin|editor|viewer)
audit_log(id, user_id, action, entity, entity_id, detail, created_at)
```

### 后续状态迁移（0005–0008）

- `0005_meta_state.sql` — `items.meta_state(none|done|failed)`，扫描指纹变化时同步重置。
- `0006_dupes.sql` — `items.partial_hash`（重复检测部分哈希）+ `idx_items_size`。
- `0007_ocr.sql` — `book_items.ocr_text`（OCR 文本入 FTS，200k 字符截断）+ `ocr_mtime`（保鲜）。
- `0008_item_category_source.sql` — `item_categories.source`（manual|auto|rule），自动分类重跑只动 auto 行。

### 智能集合 query_json 示例

```json
{
  "filter": {
    "all": [
      { "field": "type", "op": "eq", "value": "video" },
      { "field": "tag",  "op": "eq", "value": "4K" },
      { "field": "status", "op": "neq", "value": "done" }
    ]
  },
  "sort": [{ "field": "mtime", "dir": "desc" }],
  "limit": 200
}
```
字段白名单：`type/status/rating/year/language/size/mtime/tag/category/has_cover/hash_state/ocr_status`——不在白名单直接报错，测试锁死。

---

## 7. 关键实现要点

- **迁移执行**（`db/migrate.rs`）：`include_str!` 内嵌 SQL，事务内 `BEGIN → execute_batch → INSERT schema_migrations → COMMIT`，失败整体回滚；`migrate_test` 验证全量跑通 + 幂等。
- **扫描 upsert**：`INSERT ... ON CONFLICT(root_id, rel_path) DO UPDATE`；`size+mtime` 未变 → 跳过元数据提取（增量扫描核心）；文件消失 → 标 `missing` / 移入待确认列表，**不直接删**。
- **命令层模式**：`commands.rs` 只做桥接（`#[tauri::command]` + 事件 emit + JobState 单飞），纯逻辑全在 `scan/meta/smart/rules`，`tests/` 直接调用。
- **前端约定**：`src/api.ts` 唯一 invoke 出口；结构体 snake_case ↔ JS 同名，命令参数 camelCase（Rust `root_id` ↔ JS `rootId`）；进度事件统一 `on*Progress` 订阅。

### 第一版命令清单

**权威清单见 `AGENTS.md`「当前 command 清单」**（59 项，随功能持续维护，本 README 不重复以免漂移）。分类速览：环境/设置、根目录、任务（扫描/哈希/元数据/规则/自动分类/重复/OCR）、浏览、分类、标签、智能集合、缩略图、备份、导入导出、用户/审计。

---

## 8. 功能模块与路线图

### 阶段一：本地索引与浏览（体量最大）
1. 脚手架：端口 5175、identifier/productName、`AGENTS.md` 照抄 vinePress 骨架
2. `db/`：连接池 + WAL + 迁移器 + `migrate_test`
3. 设置页：多根目录 / 派生目录 / 数据库路径
4. 扫描器：四类分派、upsert、增量跳过、进度事件，`scan_test`（fixture）
5. 元数据：ffprobe/exiftool JSON → 扩展表；beets/Calibre 只读库导入
6. 基础 UI：三栏（分类树 / 虚拟滚动列表 / 详情）、列宽与面板持久化、Ctrl+K 命令面板、Ctrl+Shift+F 全局搜索
7. FTS5 同步与搜索高亮

**验收**：1.1TB 首轮扫描可断点续扫；重启后二次扫描分钟级以内；搜索专辑名秒出。

### 阶段二：分类与标签
1. 分类树 CRUD + 拖拽移动
2. 标签：受控词表、同义词、批量打标、`merge_tags`
3. 智能集合：表单式查询构建器 + `smart_test` 注入锁死
4. 规则自动打标（扫描/导入后触发，`source='rule'`，动作含 `add_tag` / `set_category`）
5. 缩略图：图片缩放、视频抽帧、封面落派生目录
6. 树/列表/集合三视图联动
7. 自动分类：目录结构镜像（前 N 层文件夹 → 分类树，`source='auto'` 幂等重跑）+ 批量归类

**验收**：3 个示例集合（未读且=科幻 / 4K 未看 / 待 OCR）可用；规则跑全库不卡 UI。

### 阶段三：本地协作数据就绪（原远程协作降级）
1. `users` 本地管理（admin 改密，Argon2）
2. 所有写操作记 `audit_log`
3. axum / Tailscale / 协作界面 → **移入阶段五**，仅保留 `collab/` 空壳与设计文档

**验收**：本地写操作 100% 有审计记录。

### 阶段四：高级能力
1. 重复检测：size 分组 → 可疑组补哈希 → 只读报告（绝不自动删）
2. OCR 队列：OCRmyPDF + Tesseract，`ocr_status` 状态机
3. 备份：元数据定时快照 + restic 集成钩子
4. 标签/分类 JSON 导入导出；Meilisearch 可选（FTS5 够用则不做）
5. 视需要：与 vinePress 联动、Syncthing 多设备

### 贯穿约定
- 每阶段收尾：`cargo fmt` + `cargo clippy` + `npm.cmd run lint` + 新增测试
- 纯逻辑模块零 Tauri 依赖；新功能补 `docs/` + 更新 `AGENTS.md` command 清单
- **不变量**：原始媒体文件只读；本地 SQLite 为唯一真相源，离线可用；迁移只增不改

---

## 9. 风险与对策

| 风险 | 对策 |
|---|---|
| 1.1TB 哈希耗时 | 首轮只存 size+mtime；哈希后台按需；两级查重 |
| 外置盘盘符漂移 | `redirect_root`，条目存 rel_path |
| SQLite 放慢盘损坏 | catalog.sqlite 强制本地 AppData；派生数据可迁外置 |
| FTS5 写放大 | 无 trigger，Rust 侧显式同步，批量导入后一次 rebuild |
| 外部工具缺失 | `probe_tools` 探测 + 设置页手动指定，缺工具降级不崩溃 |
| 与 vinePress 行为混淆 | AGENTS.md 首条不变量写明“只读、无导出打包” |

---

## 10. 开发状态

- [x] 规划完成（本文档）
- [x] 阶段一第 1-2 步：脚手架（5175 / identifier / AGENTS.md / docs）+ db 连接池 + 迁移器 + 0001~0004 + `migrate_test` 全绿（2026-10-09）
- [x] 阶段一第 3 步：设置页（多根目录 / 派生目录 / 数据库路径 + 工具路径手动指定）+ `repo_test`（2026-10-09）
- [x] 阶段一第 4 步：扫描器（四类分派、指纹增量、UPSERT、missing 标记、`scan-progress` 进度）+ 后台哈希任务（`start_hash_job` / `cancel_job`，`hash-progress`）+ `scan_test` 5 项（2026-10-09）
- [x] 阶段一第 5 步：元数据（`meta/` ffprobe/exiftool JSON → 扩展表 + beets/Calibre 只读库按绝对路径匹配导入 + `meta_state` 状态机 + `start_meta_job` / `meta-progress` + 设置页元数据面板）+ `meta_test` 7 项（2026-10-09）
- [x] 阶段一第 6 步：基础 UI（三栏树/列表/详情、分块虚拟滚动、列排序/列宽/面板持久化、`browse.rs` 白名单查询 + `list_items`/`list_categories`/`get_overview`/`get_item`/`search_items`/`open_path`/`reveal_path`、Ctrl+K 命令面板 + Ctrl+Shift+F 搜索 + `browse_test` 6 项）（2026-10-09）
- [x] 阶段一第 7 步：FTS5 同步与搜索高亮（`fts.rs` 显式同步（扫描/元数据/删根钩子）+ 启动一致性检查 + `search_items` FTS5 优先 BM25、LIKE 回落 + 列表/详情命中标记高亮 + `fts_test` 7 项）（2026-10-09）
- [x] 阶段二第 1-5 步（分类 / 标签 / 智能集合 / 规则 / 缩略图）：
  - [x] 分类树 CRUD + 拖拽移动（物化路径防环/防交错，`category_test` 7 项）
  - [x] 标签：受控词表、同义词别名、批量打标、`merge_tags`（`tag_test` 6 项）
  - [x] 智能集合：保存/预览 + 注入锁死（`smart_test` 6 项）+ 三个示例集合（未读且=科幻 / 4K 未看 / 待 OCR）首次播种
  - [x] 规则自动打标：`rules.rs` 白名单校验 + `start_rule_job` 单事务后台执行 + `rule-progress` + 扫描/导入后自动触发（`source='rule'`，不降级 manual）+ `rule_test` 6 项
  - [x] 缩略图：图片缩放 320 / 视频 ffmpeg 抽帧、`<id>-<mtime>.jpg` 派生目录落盘、asset 协议动态白名单 + 详情面板展示（`thumbs_test` 4 项）
- [x] 阶段二第 6 步：三视图联动收尾（搜索/集合/树模式优先级 + 集合/搜索本地排序 + 扫描/规则后列表失效刷新 + 详情状态/评分可编辑联动树计数 + `browse_test` 排序回归，cargo 92 项全绿）
- [x] 阶段三：本地协作数据就绪
  - [x] `users` 多用户 + 本地 admin 登录（Argon2：`auth.rs` 首启建管理员 / 登录会话 / 改密 / 用户 CRUD 保管理员守卫）
  - [x] 所有写操作进 `audit_log`（`audit.rs` 动作白名单 + `audit_test` 命令覆盖率守护：39 写命令全插桩、20 只读豁免、lib.rs 注册一一对应；登录失败也入账、密码绝不记录；任务按汇总入账）
  - [x] `collab/` 设计占位（axum/Tailscale/角色门禁草案，远端不在本期）
  - [x] 设置页「用户与登录」「审计日志」面板；cargo 104 项全绿（2026-10-09）
- [x] 阶段四：高级能力
  - [x] 重复检测：`0006_dupes`（partial_hash：sha256 长度+头尾 1MiB）+ size 分组后台 job + `list_dupes` 只读报告（完整哈希确认/待确认切分、撞车剔除）+ 设置页面板，`dupe_test` 4 项，cargo 108 项全绿（2026-10-09）
  - [x] OCR 队列：`0007_ocr`（`ocr_text` 入库 + `ocr_mtime` 保鲜）+ `ocr.rs`（OcrRunner 注入、none→queued→done|failed 状态机、mtime 失配重跑）+ OCRmyPDF 派生副本（原始只读）+ FTS 全文可检索 + 设置页面板/徽标，`ocr_test` 4 项，cargo 112 项全绿（2026-10-09）
  - [x] 备份：`backup.rs`（`VACUUM INTO` 元数据快照 + keep 清理 + restic 集成钩子 `ResticRunner` 注入、未配置/失败降级保留本地快照）+ `last_backup_at` + 应用内定时器（间隔小时、15 分钟巡检）+ 设置页面板，`backup_test` 3 项，cargo 115 项全绿（2026-10-09）
  - [x] 标签/分类 JSON 导入导出：`port.rs`（物化路径分类 + 标签/别名快照，增量 upsert、祖先补建、冲突跳过）+ `export_port`/`import_port` + 设置页面板；Meilisearch 不做（FTS5 已覆盖检索），`port_test` 3 项，cargo 118 项全绿（2026-10-09）
  - [x] 视需要：与 vinePress 联动、Syncthing 多设备 → 落为设计定稿 `docs/sync.md`（同步矩阵：catalog 绝不同步、派生/备份快照/`PortDoc` 词表可同步；多设备词表交换 = `export_port`→同步→`import_port`；与 vinePress **第一版零代码耦合**，条目清单导出列为未来按需联动点）（2026-10-09）
  - [x] UI 完善批次：列表键盘导航（↑↓/PgUp/PgDn/Home/End/Enter、Ctrl/Shift 扩展多选）+ 行右键菜单（打开/定位/复制路径）+ 首次无根目录引导 CTA + 批量设状态 + 顶栏账号 chip + 设置页分区导航；纯前端，npm 三关全绿（2026-10-09）
  - [x] UI 现代化批次：Shift 范围连选（锚点，鼠标 + 键盘 Shift）+ 行内 hover 快捷操作（打开/定位）+ 详情复制路径 + CSS 现代化（硬编码色全 token 化、细滚动条/焦点环/微动效/选中标记/进出场动画、`prefers-reduced-motion`）+ 深浅主题切换（`#theme-toggle`，`html[data-theme]` 变量组覆盖，localStorage `vinecurator.prefs.theme` 持久化、首次跟随系统）；cargo 118 项 + npm 三关全绿（2026-10-09）
  - [x] 任务体验批次：顶栏全局任务指示器 `#job-chip`（脉冲圆点 + 文案 + 微进度条，完成转绿 3 秒后隐藏，点击跳任务面板——浏览页跑任务可见）+ 不定进度流光动画（total 未知）+ 扫描真实百分比 + 进度文案收口 `updateJob`；cargo 118 项 + npm 三关全绿（2026-10-09）
  - [x] 封面网格视图：toolbar 列表⇄网格切换 + 网格紧凑排序（与列头同源）+ 虚拟滚动封面卡（懒加载 `ensure_thumb`、`id:mtime` 缓存）+ Shift 范围/Ctrl 多选/右键/批量条全复用 + 网格键盘导航 + 视图偏好持久化；**修复 `ensureChunk` 潜在微任务死循环**（仅新拉数据才重渲染 + in-flight 等待图）；cargo 118 项 + npm 三关全绿（2026-10-09）
  - [x] 拖放与预览：拖放文件夹直加根目录（Tauri `onDragDropEvent` + 全屏提示遮罩，批量/重复/文件混拖容错）+ 媒体预览灯箱（详情缩略图点击 / 右键「预览」：图片原片、视频原片 + 封面兜底，Esc/遮罩/✕ 关闭）+ 根目录加入 asset 协议白名单（启动全量 + add/redirect 命令追加，灯箱只读播原片）；cargo 118 项 + npm 三关全绿（2026-10-09）
  - [x] 事件链修复：跨模块 `vc:*` CustomEvent 统一 **window↔window**（ui.ts 9 处 dispatch + `vc:selection-changed` 监听——修复 CTA「添加根目录…」、toast、视图持久化不生效死链；`bubbles:false` 导致 document→window 永不送达）+ 根表单空路径自动弹目录选择器 + `pickDir` 统一错误兜底 + 拖放 `getCurrentWebview` 同步 try/catch；cargo 118 项 + npm 三关全绿（2026-10-09）
  - [x] 定位与自动分类：`reveal_path` 反斜杠归一化（`explorer /select,` 对正斜杠路径不选中）+ 目录结构镜像自动分类（`0008` 加 `item_categories.source`、`autocat.rs` 前 N 层文件夹建树归类、扫描后**自动分类→规则**链式触发、重跑只动 auto 不碰 manual/rule）+ 规则动作 `set_category` + 多选批量归类 + 设置页「自动分类」面板；`autocat_test` 4 项等，cargo **127 项** + npm 三关全绿（2026-10-09）
