# VineHub

> VineTurtleStudio 文件类工具的统一主面板（宿主）+ 插件化整合计划。
> 产品 identifier：`com.vineturtle.vinehub`（可改）。

## 0. 方案定稿与项目选择

### 0.1 已定稿的四项架构决策

| 决策项 | 最终选择 | 依据 |
|---|---|---|
| UI 栈 | **React 19 + Vite** 做宿主壳；插件 UI 走 **iframe 隔离** | 组件框架是动态插件面板的业界主流（Obsidian/Grafana/Notion）；iframe 隔离使插件前端框架无关 |
| 插件运行时 | **全 sidecar**（进程外 + JSON-RPC over stdio），不使用进程内 dylib | 单一机制、心智负担最小；崩溃隔离（VS Code 模型）；文件 I/O 场景进程边界无性能损失；独立 exe 天然可双击 |
| 仓库形态 | **monorepo + cargo workspace** | 接口同步演进、依赖统一管理；业界主流 |
| 整合深度 | **壳级融合**（单窗口 + iframe 面板）为上限，**不做**组件级融合 | 业界公认最优平衡点；组件级融合普遍因版本漂移被放弃 |

补充原则：

- **独立运行能力 100% 保留**——插件化是给每个工具加「被宿主编排」的能力，不是把它变成宿主的附庸。hub 不存在时，5 个工具照常各用各的。
- **插件分发** = zip（`.vpkg`），开发期直接放目录；第一方插件随主包内置。

### 0.2 纳入 hub 的插件项目（已选定）

全部 5 个文件类项目均做成插件，**接入优先级顺序如下**（媒体资产管理、光标编辑因细节特别多，排最后）：

| 优先级 | 项目 | 职责 | 接入阶段 |
|---|---|---|---|
| 1 | **vinePulse** | 文件损坏检测（M1 已完成，首个试点） | P1 |
| 2 | **vinePress** | 文件整理与批量归档 | P2 第一个 |
| 3 | **vineSync** | Windows 文件夹同步备份 | P2 第二个 |
| 4 | **vineCurator** | 本地媒体资产管理（细节多） | P2 第三个 |
| 5 | **VinePalmaCursor** | 鼠标光标编辑（细节多） | P2 最后 |

**存量项目状态（2026-10-16）**：

- **vineFerry**（复制/移动 + 哈希校验，曾附带资源管理器式文件管理 GUI）——**已封存**，移出 workspace members，不再编译；详见 `apps/vineFerry/ARCHIVED.md`。
- **vineSeal**（藤印）——vineFerry 的后继：只做「转移即校验」的极简工具（CLI + 单窗口 GUI + sidecar 插件），已加入 workspace。

### 0.3 明确不纳入

- **GrapeLeafi-QQbot**（及 share 副本）、**vine-sinitic-治世**（Godot 游戏）、**Android/VinePalma Player**——非文件类/平台不符，保持独立开发，不列入 hub。
- `VineSinitic_治世` 为重复文档目录，建议归并入 `vine-sinitic-治世` 后删除（与本计划无关，另行处理）。

---

## 一、目标仓库结构（monorepo 终态）

```
D:\VineTurtleStudio\Projects\
├── VineHub\                        ★ 本仓（新建主仓）
│   ├── .cargo\config.toml          ← 迁入 MSVC 链接路径（现 vinePulse 那份）
│   ├── Cargo.toml                  [workspace] resolver + workspace.dependencies
│   ├── .gitignore / .gitattributes
│   ├── apps\
│   │   └── vine-hub\               宿主（Tauri 2 + React 19 + Vite + TS）
│   │       ├── src\                React：侧边栏/面板容器/导入向导/设置/任务中心
│   │       └── src-tauri\          Rust：registry、SidecarManager、SQLite、桥
│   ├── crates\
│   │   ├── vine-plugin-api\        manifest schema + RPC 消息类型 + Host API DTO + 错误码
│   │   └── vine-host-core\         插件注册表 / 子进程生命周期 / 权限闸门 / 配置存储
│   ├── plugins\                    第一方 manifest（5 份 plugin.json，仅清单不含二进制）
│   └── docs\
│       ├── plugin-spec.md          插件规范 v1
│       ├── rpc-protocol.md         RPC 协议 v1
│       └── bridge-spec.md          iframe 桥 v1
├── apps\                           ← P2 迁入的 5 个项目
├── GrapeLeafi-QQbot\ ...           非文件类项目留在原处，不迁移
└── vine-sinitic-治世\ ...          同上
```

- **迁移动作集中在 P2 一次性完成**（P0/P1 期间 5 个项目原地不动，hub 用绝对路径 + 设置页路径覆盖定位 exe，零打扰）。
- **P0 第一项任务是 `git init`**（多阶段重构必须有版本控制；仅本地仓，不外发）。
- 工作规则：
  - 中文源码文件一律用编辑工具修改，**禁用 PowerShell `Set-Content`/`-replace`**（会把 UTF-8 转成 GBK 造成编码损坏）。
  - `.cargo/config.toml` 的 MSVC 链接路径随迁移走，普通 PowerShell 必须能直接 `cargo build`。
  - `zip 2.x` 与 `zip 8.x` 在 cargo 依赖图中可并存，无需强行统一；新增依赖统一走 `workspace.dependencies`。

---

## 二、插件规范 v1（`plugin.json`）

```jsonc
{
  "schemaVersion": 1,
  "id": "vine.pulse",                 // ^[a-z][a-z0-9]*(\.[a-z0-9]+)*$，全局唯一，即数据目录名
  "name": "vinePulse",
  "version": "0.1.0",
  "apiVersion": "1.0",                // 宿主 semver 门禁：major 不同拒载，minor 高于宿主拒载
  "kind": "sidecar",                  // external | sidecar（external=仅启动独立程序，P0 形态）
  "entry": "bin/vine-pulse.exe",      // 解析顺序：绝对路径 > 插件目录相对 > 设置页覆盖 > PATH
  "panels": [
    { "id": "scan", "title": "损坏检测", "ui": "dist/index.html", "icon": "pulse" }
  ],
  "commands": [
    { "id": "scan.start", "title": "执行扫描" }
  ],
  "permissions": {
    "fs.read": ["user-picked"],       // user-picked | all-selected（用户授权的根路径集合）
    "fs.write": [],
    "net": false,
    "exec": false
  },
  "dataDir": "vine.pulse",            // → %APPDATA%\VineHub\plugins\vine.pulse\
  "minHubVersion": "0.1.0"
}
```

要点：

- **`kind` 是插件的生命周期等级**：`external`（P0：只负责一键打开独立程序）→ `sidecar`（P1+：宿主拉起进程走 RPC）。升级只改这一个字段，其余不动。
- **校验规则**（`vine-plugin-api`）：id 格式、入口文件存在性、`panels[].ui` 路径不越界、apiVersion 兼容。**任一失败 = 该插件标「不可用」并在 UI 显示原因，不影响其他插件。**

---

## 三、RPC 协议 v1（`docs/rpc-protocol.md`）

**传输**：子进程 stdin/stdout，**一行一条 UTF-8 JSON**（JSON-RPC 2.0 风格）；stderr 一律视为日志，宿主落盘到 `dataDir/logs/`。

**Windows 规则**：`CREATE_NO_WINDOW` 启动；宿主退出用 Job Object（或 `taskkill /T`）保证无孤儿进程。

```jsonc
// 连接后宿主必发握手
→ {"jsonrpc":"2.0","id":0,"method":"plugin.info"}
← {"jsonrpc":"2.0","id":0,"result":{"id":"vine.pulse","version":"0.1.0",
     "apiVersion":"1.0","capabilities":["scan","report"]}}
// 不匹配 → 宿主直接杀进程并标「不可用（版本不兼容）」

→ {"jsonrpc":"2.0","id":1,"method":"scan.start","params":{"path":"D:/photos","types":["zip","png"]}}
← {"jsonrpc":"2.0","id":1,"result":{"taskId":"t-42"}}
← {"jsonrpc":"2.0","method":"task.progress","params":{"taskId":"t-42","done":120,"total":800,"msg":"正在校验 x.zip"}}
← {"jsonrpc":"2.0","id":1,"result":{"taskId":"t-42","ok":798,"bad":2,"report":{...}}}   // id 配对=终态
→ {"jsonrpc":"2.0","method":"task.cancel","params":{"taskId":"t-42"}}                   // 无 id=notification
```

**规范要点**：

- 标准错误码：`-32700` 解析失败、`-32601` 方法不存在、`-32602` 参数非法；应用码：`-32001` 已取消、`-32002` 超时、`-32003` 权限拒绝、`-32004` 任务不存在、`-32005` 插件内部错误。
- 心跳：宿主每 15s 发 `ping`，插件 3 次无响应 → 标记假死 → 杀进程 → 任务标失败并提示可重试。
- 并发：宿主侧每插件默认最多 2 个并发任务，队列化；进度通知可丢，**响应不可丢**。
- `task.cancel` 后插件必须在 5s 内回终态，否则宿主强杀。
- 能力协商：`capabilities` 列表决定面板/命令是否可用（灰显）。

**首批插件的命令面**（P1/P2 逐个在各自 spec 附录定稿）：

| 插件 | 命令 |
|---|---|
| vine.pulse | `scan.start` / `task.cancel` |
| vine.press | `plan.start`（试算分类，不落盘）/ `organize.execute` / `task.cancel` |
| vine.sync | `task.list` / `task.run` / `task.cancel` |
| vine.curator | `library.scan` / `search` / `task.cancel` |
| vine.cursor | `file.convert` / `task.cancel` |

---

## 四、分阶段任务拆解与验收标准

**总周期**：一人约 7–9 周，每阶段结束都是可独立交付的可用状态。

### P0 · 宿主骨架（约 1 周）

| # | 任务 | 验收 |
|---|---|---|
| 0.1 | `git init` + `.gitignore` + workspace 骨架（`crates/`、`apps/vine-hub`、`plugins/`、`docs/`） | `cargo build` 在普通 PowerShell 下通过（MSVC 路径就位） |
| 0.2 | `vine-plugin-api`：manifest 结构/校验 + RPC 消息类型 + 错误码 | 单测覆盖：5 份合法样例全过、非法样例（坏 id/坏版本/路径越界）全拒 |
| 0.3 | `vine-host-core`：PluginRegistry（扫目录→解析→缓存→状态） | 单测：加载/热重载/单插件失败隔离 |
| 0.4 | `vine-host-core`：SidecarManager 骨架（spawn、行分隔编解码、id 配对、pending 表、超时、Job 收尾） | 集成测试：假插件 exe fixture 握手→请求→响应→杀进程全绿 |
| 0.5 | 配置存储：rusqlite 单库（`plugins`/`settings`/`tasks` 三表） | 单测：迁移/读写/路径覆盖项 |
| 0.6 | 宿主 UI：侧边栏（registry 动态生成）、插件详情页、**外部启动按钮**、设置页（exe 路径覆盖）、导入 zip 骨架（解压+校验+重扫） | 手测：5 份 manifest 载入，5 个独立程序从 hub 一键启动成功 |
| 0.7 | 写 5 份第一方 `plugin.json`（`kind: "external"`） | 随 0.6 验证 |

**P0 交付**：一个能列出、导入、启动全部工具的统一入口，RPC 通道已就绪。

### P1 · vinePulse sidecar 化（约 3–5 天）

| # | 任务 | 验收 |
|---|---|---|
| 1.1 | vinePulse 加 `--rpc` 子命令（argv 最早分流；rpc 模式跳过控制台 UTF-8 初始化、日志走 stderr） | 现有 18 个测试 + CLI 行为**零回归** |
| 1.2 | 实现 `plugin.info` / `scan.start` / `task.progress` / `task.cancel`；扫描跑在线程，进度回调发通知，取消用 `AtomicBool` 协作中断 | 管道脚本手动驱动全流程通过 |
| 1.3 | 协议级集成测试：spawn 真实 exe 走 握手→扫描→取消→终态 | 进入 `cargo test` |
| 1.4 | manifest 升 `kind: "sidecar"`；hub 面板：路径选择、类型过滤、进度条、结果表（ok/bad、熵值、哈希不匹配）、取消按钮 | hub 内完成一次真实目录扫描并出报告 |
| 1.5 | 假死/超时/崩溃三种异常路径演练 | UI 显示可理解错误 + 重试入口，宿主不卡死 |

**P1 交付**：端到端 RPC 链路 + 第一个真插件面板，同时证明「独立 CLI 与 hub 插件共用同一 core」。

### P2 · 迁移 + 其余 4 个插件（约 2–3 周）

接入顺序（按 0.2 优先级，细节多的排后）：**vinePress → vineSync → vineCurator → VinePalmaCursor**。

| # | 任务 | 验收 |
|---|---|---|
| 2.1 | **一次性迁移**：5 项目移入 `VineHub/apps/`，workspace 收编，`.cargo/config.toml` 并入根，manifest entry 改相对路径 | 每个项目独立 `cargo build/test` 与迁移前一致；vinePulse 18 测试仍绿 |
| 2.2 | SidecarManager 硬化：心跳、孤儿清理、重启策略、并发队列、日志落盘 | P1.5 异常演练扩展为自动化测试 |
| 2.3 | vinePress 加 `--headless --rpc` + 接入 hub | 协议级测试；原独立 GUI 不受影响 |
| 2.4 | vineSync 加 `--headless --rpc` + 接入 hub | 同上 |
| 2.5 | vineCurator 加 `--headless --rpc` + 接入 hub（SQLite 以只读查询为主） | 同上 |
| 2.6 | VinePalmaCursor 加 `--headless --rpc` + 接入 hub | 同上 |
| 2.7 | 任务中心 UI：跨插件任务列表、进度、取消、失败重试 | 多插件并发任务可观测、可取消 |

### P3 · iframe 面板收编（约 2–3 周，逐项目）

面板迁入顺序与 P2 一致：**vinePress → vineSync → vineCurator → VinePalmaCursor**。

| # | 任务 | 验收 |
|---|---|---|
| 3.1 | 宿主 iframe 容器 + `bridge-spec.md`：`{channel:"vinehub", v:1, id, method, params}` postMessage 协议；白名单 API：`fs.pick` / `config.get·set` / `task.*` / `notify` / `log` | 权限外方法被拒并回错误码 |
| 3.2 | 每个前端做**双模适配层** `src/host-bridge.ts`：hub 内→postMessage，独立运行→原生 `invoke`（一处收敛，其余代码不改） | 独立 exe 行为与改造前完全一致 |
| 3.3 | 按既定顺序迁入面板，处理 CSP/本地资源/主题/滚动 | 每个面板在 hub 内可用；`tsc --noEmit` 全过 |
| 3.4 | 卸载/禁用插件后其面板从侧边栏消失、数据目录按选项保留或清理 | 手测清单通过 |

**边界（明确不做）**：插件 UI 之间不共享组件/状态；不提供插件互调——编排只发生在宿主层。

### P4 · 打包发布（约 1–2 周）

| # | 任务 | 验收 |
|---|---|---|
| 4.1 | `.vpkg`（zip）正式化：`manifest.sha256` 校验、安装/覆盖更新/回滚、卸载清理 | 损坏包、错版本包、伪造包均被拒绝且提示明确 |
| 4.2 | Tauri 打包：第一方 manifest 随包分发 + sidecar exe 布局（`externalBin` 命名坑处理） | 干净虚拟机安装→开箱即用 |
| 4.3 | 插件管理页：启用/停用、版本与 apiVersion 兼容性提示、打开数据目录、崩溃日志查看 | 手测清单 |
| 4.4 | Windows 环境收尾：杀软误杀报错话术、签名决策（第一方随主包签） | 误杀时显示可行动提示而非静默失败 |
| 4.5 | 三份 spec 文档定稿 + 本 README 更新（里程碑/测试策略/风险） | 文档与实现一致 |

---

## 五、横切约定（全程遵守）

- **配置/状态**：hub 单一 SQLite；**各插件业务库原地不动**（Curator FTS5、Sync 历史表），只登记路径——不做数据迁移。
- **权限信任模型（如实声明）**：sidecar 是用户自装的可信程序，**不做 OS 级沙箱**；manifest 权限用于 ① 宿主转发闸门（不给越权路径/方法）② UI 明示用户。`net:false` 等声明靠宿主不注入网络能力 + 代码审查，非强制隔离。
- **测试口径**：`cargo test`（含协议级 spawn 测试）+ `tsc --noEmit` + 每阶段手测清单；P1 起每个 sidecar 必须有协议测试才算完成。
- **独立运行**：每个项目保留现有独立 exe/app 入口不变；sidecar 模式通过 `--headless --rpc` 子命令叠加，不新增第二个可执行文件。

## 六、风险与对策

| 风险 | 影响阶段 | 对策 |
|---|---|---|
| 迁移打断各项目构建 | P2.1 | 一次性迁移 + 迁移后逐项目全量验证，不过不进下一步 |
| Tauri app 加 `--rpc` 与现有启动逻辑冲突 | P2.3–2.6 | argv 最早分流，rpc 模式完全绕开 GUI 初始化 |
| iframe 本地资源/CSP 兼容 | P3 | 先拿最简前端打样，趟通再推广 |
| SmartScreen/杀软拦截 sidecar | P2/P4 | 沿用 strip 策略；P4 明确报错话术；第一方随主包签名 |
| vineCurator / VinePalmaCursor 细节多导致周期拉长 | P2 后半段 | 已排在最后两位，前 3 个插件趟通后接口与脚手架均为现成 |
| 一人周期拉长 | 全程 | 每阶段结束都是可用产品，可随时暂停在任一里程碑 |

## 七、里程碑

```
P0 = 统一入口（external 启动 5 个工具）
P1 = 真插件链路（vinePulse 端到端）
P2 = 全插件可编排（Press → Sync → Curator → Cursor）
P3 = 单窗口壳（iframe 面板收编）
P4 = v1.0 可发布
```
