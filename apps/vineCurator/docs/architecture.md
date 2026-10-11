# 架构总览

vineCurator = Tauri 2.x（Rust 后端）+ 零框架 TypeScript（Vite 前端）+ 本地 SQLite。与 vinePress 同骨架，但职责不同：vineCurator 只做**长期索引与检索**，对原始媒体文件永远只读。

## 分层

```
src/                       前端（零框架 TS）
├── index.html             快捷键唯一来源 data-keys
├── main.ts                事件接线 / 启动
├── api.ts                 唯一 invoke 出口（结构体 snake_case，参数 camelCase）
└── styles/                原生 CSS（token 对齐 vinePress 视觉）

src-tauri/src/             Rust 后端
├── main.rs / lib.rs       入口；setup 打开连接池 + 自动迁移 + manage(DbState)
├── commands.rs            薄桥接：#[tauri::command] + emit + JobState 单飞
├── db/                    mod.rs（连接池 WAL）、migrate.rs（迁移器）、repo.rs（仓储）
├── env.rs                 外部工具探测（纯逻辑）
├── scan/ meta/            阶段一第 4-5 步：遍历分派、fingerprint、upsert、ffprobe/exiftool
├── rules.rs smart.rs fts.rs   规则打标 / 白名单 SQL 构建器 / FTS5 同步（纯逻辑）
├── tasks.rs thumbs.rs dupes.rs ocr.rs backup.rs port.rs   后台队列、派生图/OCR/快照/词表导入导出
├── auth.rs audit.rs shell.rs env.rs       本地用户会话、审计、打开/定位、工具探测
└── （阶段五远端协作设计占位在仓库根 collab/）

src-tauri/migrations/      0001_core → … → 0008_item_category_source（只增不改）
src-tauri/tests/           18 个测试文件（migrate/repo/scan/meta/browse/fts/…/port/autocat，全部零外部依赖）
```

## 关键机制

- **连接池**：r2d2 + rusqlite（bundled，含 FTS5），`journal_mode=WAL`；库文件固定本地 AppData，不放外置盘。
- **迁移**：`include_str!` 内嵌 SQL → 每文件独立事务（`BEGIN → execute_batch → INSERT schema_migrations → COMMIT`，失败整体回滚）→ 启动时自动执行；只增不改。
- **存储与检索分离**：原始文件在外置硬盘（`roots` 多根 + `items.root_id+rel_path`，盘符漂移用 `redirect_root`）；元数据与索引在 SQLite。
- **哈希懒计算**：首轮仅 `size+mtime` 指纹（`hash_state='none'`），完整哈希后台按需；查重两级（size 分组 → 补哈希）。
- **事件约定**：进度统一 `scan-progress` / `hash-progress` / `thumb-progress`，前端 `on*Progress` 订阅。
- **多设备与联动**：见 [sync.md](./sync.md)——`catalog.sqlite` 绝不同步；派生目录/备份快照/`PortDoc` 词表可经 Syncthing 交换；与 vinePress 第一版零代码耦合。
