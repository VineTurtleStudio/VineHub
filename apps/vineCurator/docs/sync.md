# 多设备同步与工具联动（Syncthing / vinePress）

阶段四第 5 步的设计定稿。核心原则重申：**catalog.sqlite 是单机真相源、原始媒体永远只读、与 vinePress 第一版零代码耦合**。本文档把「已具备的能力」定格为同步介质，把「不做/以后做」写清楚，避免后续误加功能。

## 一、Syncthing 同步矩阵

| 数据 | 位置 | 可否同步 | 理由 |
| --- | --- | --- | --- |
| `catalog.sqlite`（+ `-wal`/`-shm`） | `%LOCALAPPDATA%\com.vineturtle.vinecurator\` | **绝对不行** | WAL 在慢盘/拔盘时损坏；两台设备并发写同一库必坏。库强制本地 AppData（AGENTS 不变量），永不放外置盘/同步盘。 |
| 原始媒体 | `roots` 多根（外置盘） | 由 Syncthing / NAS 负责 | vineCurator 只读扫描，不管同步。多设备间**路径需一致**（同盘符同结构），漂移用 `redirect_root`。 |
| 派生缩略图 / OCR 副本 | `<derived>/thumbs/`、`<derived>/ocr/` | ✅ 可同步 | 纯派生数据：文件名含 mtime（`<id>-<mtime_ms>.jpg`），任一副本可用、过期由 `ensure_thumb` 重生成；同步冲突无害（多余副本闲置，不会被引用）。 |
| 备份快照 | `<derived>/backups/` | ✅ 最适合同步 | 不可变 + 时间戳命名（`catalog-<epoch_ms>.sqlite`，同 ms `-N` 后缀）。注意：Syncthing 冲突副本文件名带 ` sync-conflict-…` 后缀，**不匹配** `prune` 的命名模式 → 不会被自动清理（安全：不会误删正本；代价：会积累，可手动清）。 |
| 标签 / 分类 JSON | `export_port` 任选路径 | ✅ 推荐同步 | 词表交换介质（`PortDoc`，版本化 `version:1`），体积 KB 级。 |

## 二、多设备工作流（v1 的实际做法）

单写者 + 人工交换，**有意不做 CRDT/自动合并**（本地优先工具，简单可靠优先）：

1. **每台设备独立 catalog**：各自扫描各自的 roots；媒体本体由 Syncthing 同步到路径一致。
2. **词表交换**：设备 A `export_port` → 同步文件 → 设备 B `import_port`（幂等 upsert：缺失祖先补建、同名更新 name/sort、冲突别名跳过；重复导入零新增）。**同名冲突 = 后导入者胜**，这是设计而非事故。
3. **新设备冷启动**：全量扫描 → `import_port` 导入词表 → 启用规则后 `start_rule_job`。规则打标（`source='rule'`）可从词表+条件**重建条目关联**（item_tags）。
4. **已知限制（如实）**：`import_port` 只动词表与分类树，**不带逐条目的手工打标**（`item_tags` 中 `source='manual'` 的关联）——冷启动后手工标签需重打，或等待未来「条目清单导出」。整库搬移（非同步）走手动：关应用 → 用 backups 快照替换 `catalog.sqlite`（v1 不做 `restore` 命令，避免连接池运行中换库文件）。

## 三、vinePress 联动（第一版 = 0 代码耦合）

- **边界**（AGENTS 关键不变量）：vinePress = 一次性归档打包，`project.json` 是其真相源；vineCurator = 长期索引，只读。两者**互不读写对方文件、不共享 crate**。
- **已具备的互操作契约**：`PortDoc` JSON（`categories[].path` 物化路径 + `tags[].name/aliases`）是版本化的纯数据格式，任何工具可解析。vinePress `Entry` 有 `tags: Vec<String>` / `category` 字段——人工工作流：导出词表 → 参照其填充 Entry（无需代码对接）。
- **未来联动点（已设计、按需再做，均需新命令 + 审计 + 测试）**：
  1. **条目清单导出**：`rel_path / type / rating / status / tags / category` → JSON，供 vinePress 批量 seed `Entry`（需求真实出现再做）。
  2. vinePress 侧读取 `PortDoc` 预填 Entry 的 tags/category。
  3. 两侧共享的条目级字段无第三版需求前**不**定义正式 schema——`PortDoc.version` 已为此预留版本号演进路径。

## 四、明确不做

- ❌ 同步 `catalog.sqlite`（见矩阵，硬性）。
- ❌ vineCurator 读写 vinePress 的 `project.json`，或反向注入（第一版零耦合）。
- ❌ 双端实时合并标签树（无 CRDT；导入是幂等 upsert + last-import-wins）。
- ❌ `restore` 命令（手动替换快照已覆盖需求，运行中换库风险大于收益）。
