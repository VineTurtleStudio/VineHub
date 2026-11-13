# vineSeal（藤印）

> 文件复制/移动 + 转移后哈希校验的极简工具。
> vineFerry 的后继项目：**只做「转移即校验」，不做文件管理器。**

## 1. 定位与核心承诺

vineSeal 是一个单窗口的小工具，围绕一条安全不变量构建：

- **写入 `目标.seal.tmp` → 写后复读比对 → 原子改名**：绝不直接写最终文件名，断电/中断不会留下半截正式文件。
- **源哈希与写入共享同一次读取**（边写边算），写完再复读目标文件比对；可选「完美校验」额外复读源文件。
- **移动 = 复制 + 校验，校验通过才删源**（跨盘）；同盘走 `rename` 快路径（数据未搬动，无需哈希）。
- 校验不过 / 出错时：**源文件原封不动**，删除 tmp，逐文件报告，单文件失败不中断整批。

三种运行形态共存于同一个 exe（独立运行能力 100% 保留）：

| 形态 | 触发方式 | 说明 |
|---|---|---|
| GUI | 无参数双击/运行 | 单窗口：选源/目标 → 模式与算法 → 进度与结果表，无任何浏览功能 |
| CLI | `vine-seal copy\|move\|verify\|hash` | 退出码 0=全部成功 / 1=有失败或校验不符 / 2=参数错误 |
| Sidecar | `--rpc` | 行分隔 JSON-RPC v1，宿主 VineHub 拉起（`plugins/vine.seal`） |

## 2. 与现有子项目的分工（明确不重叠）

| | vineSeal | vineFerry（已封存） | vinePulse |
|---|---|---|---|
| 职责 | **转移过程中校验**（边写边哈希 + 写后复读比对） | 资源管理器式文件管理 GUI + 转移引擎（已封存，见 `apps/vineFerry/ARCHIVED.md`） | 事后批量损坏检测/审计（熵值、深度解码、格式校验） |
| GUI | 单窗口，仅源/目标/选项/进度/结果 | 双栏多标签浏览、缩略图、剪贴板、拖放（约 4900 行 UI） | 扫描配置 + 报告面板 |
| verify 子命令 | 轻量快速复核（对自己刚转移的文件 / 单文件期望哈希） | 有（含目录对比、复检清单导出） | 主业：全量审计与报告 |
| 清单格式 | `.sha256/.md5/.sfv` 互通 | 同左 | 同左 |

一句话：**vineSeal 管「搬得对」，vinePulse 管「查得深」**；vineFerry 的文件管理能力已封存，需要时按 `ARCHIVED.md` 恢复。

## 3. 明确不做（范围护栏）

vineFerry 就是做「完美文件管理器」才膨胀到约 8800 行。以下功能被刻意排除，**未来也不计划做**：

- 双栏/多标签文件浏览、面包屑地址栏、右键菜单、内联重命名
- 缩略图、系统 Shell 图标、预览面板
- 拖放、系统剪贴板互操作
- SQLite 历史库、预设定时任务、完成后关机/休眠
- 断点续传（tmp + journal）——失败就重跑该文件
- CSV/JSON 报告导出、i18n、便携模式
- 永久删除 / 回收站操作（本项目只在「校验通过后删源」这一处删除）

## 4. 命令行用法

```
vine-seal copy <源>... <目标目录> [-c skip|overwrite|rename] [-a sha256|xxh3|crc32|md5] [--perfect] [--no-skip-identical]
vine-seal move <源>... <目标目录> [同上]
vine-seal verify <路径> --hash <期望哈希> [-a 算法]      # 单文件按期望哈希校验
vine-seal verify -m <清单文件> <基目录> [-a 算法]        # 按清单校验目录
vine-seal hash <路径> [-a 算法]                         # 只计算并打印哈希
```

- 冲突策略默认 `rename`（`name (2).ext` 去重）；`skip` 跳过同名；`overwrite` 覆盖。
- PowerShell 提示：release 版为 GUI 子系统（与 vineFerry 同款，避免双击时闪控制台），裸调用 `& vine-seal ...` 不等待；需要退出码/输出时用管道（`& vine-seal hash f | Out-String; $LASTEXITCODE`）或 `cmd /c "vine-seal ..."`。cmd/批处理/宿主拉起（Rust `Command`）均正常等待。
- 默认算法 **sha256**（标准、与 `sha256sum` 互通）；大流量场景 `-a xxh3` 提速。
- 清单格式兼容 `sha256sum`（`hash *path`）、SFV（`HASH  path`）与通用单空白分隔，可与 vinePulse 互通。

## 5. RPC 面（`plugins/vine.seal`）

- `plugin.info` → capabilities `["transfer", "verify", "cancel"]`
- `ping`
- `transfer.start`（sources/dest/mode，复用 `vine-plugin-api::TransferStartParams`）→ ack taskId → `task.progress` 通知 → 同 id 终态响应（ok/bad/skipped/bytes/outcomes）
- `verify.start`（manifest/base/algo，复用 `VerifyStartParams`）→ ack → 进度 → 终态（ok/missing/mismatch/readError/items）
- `task.cancel`（通知或请求）；stdin 关闭后等待在途任务 ≤10s

v1 的 RPC 转移固定使用默认选项（sha256 + 常规校验 + rename 冲突策略）；算法/策略定制走 CLI/GUI。

## 6. 技术选型与结构

Rust 2024，全部依赖走根 `workspace.dependencies`；eframe/egui 0.36（GUI）、rfd（文件对话框）、sha2/md-5/xxhash-rust/crc32fast（哈希）、walkdir/filetime/windows-sys、vine-plugin-api。

```
apps/vineSeal/
├── Cargo.toml              package vine-seal，[lib] vine_seal + [[bin]] vine-seal
├── README.md / AGENTS.md
├── src/
│   ├── main.rs             argv 最早分流：--rpc → rpc；copy/move/verify/hash → cli；否则 GUI
│   ├── cli.rs              子命令解析、stderr 进度、退出码 0/1/2
│   ├── rpc.rs              stdio 行分隔 JSON-RPC v1 sidecar
│   ├── core/               纯逻辑层，零 GUI 依赖，全部单测在此
│   │   ├── hash.rs         流式哈希 4 算法，128KiB 缓冲
│   │   ├── paths.rs        \\?\ 长路径、同盘判定、reparse 剔除、磁盘空间预检
│   │   ├── conflict.rs     skip/overwrite/rename + "name (2).ext" 去重
│   │   ├── plan.rs         递归计划（Explorer 合并语义、空目录保留、同/异名处理）
│   │   ├── transfer.rs     ★ 引擎：tmp→复读比对→原子改名；校验过才删源
│   │   └── verify.rs       清单构建/解析/校验 + 单文件期望哈希校验
│   └── ui/                 单窗口（约 300 行）+ 极简配色
│       ├── mod.rs
│       └── theme.rs
plugins/vine.seal/plugin.json   kind: sidecar, panels: []
tests/rpc_protocol.rs           spawn 真实 exe --rpc 的协议级集成测试
```

## 7. 里程碑与进度

| # | 内容 | 状态 |
|---|---|---|
| M0 | 封存 vineFerry（ARCHIVED.md、移出 members、根 README 标注） | ✅ 2026-10-16 |
| M1 | 骨架 + core 引擎（hash/paths/conflict/plan/transfer/verify） | ✅ 2026-10-16 |
| M2 | CLI 四子命令 + 退出码 + 进度 | ✅ 2026-10-16 |
| M3 | 极简单窗口 GUI（选择/选项/进度/结果表/取消） | ✅ 2026-10-16 |
| M4 | RPC sidecar + plugin.json + 协议测试 | ✅ 2026-10-16 |
| M5 | README/AGENTS + build/test/clippy 终验 | ✅ 2026-10-16 |

验收记录：`cargo test -p vine-seal` 56 项全绿（50 单测 + 6 协议测试）；`cargo clippy -p vine-seal --all-targets` 零警告；CLI 冒烟（copy/hash/verify×2/move/退出码）通过；GUI 启动冒烟通过；plugin.json 通过 `PluginManifest::from_json` 校验。

**免安装便携版**：`powershell -ExecutionPolicy Bypass -File tools\package-portable.ps1` → `dist-portable\vineseal-portable.zip`（vine-seal.exe + 使用说明.txt + README.md，exe 即免安装本体，无数据目录）。release exe 已按 cmd/批处理/带管道 PowerShell 三种方式验收退出码 0/1/2。

## 8. 许可

MIT（VineTurtleStudio 内部项目）。
