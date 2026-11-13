# vineFerry（藤渡）

给文件「摆渡」——一款面向 Windows 的**文件复制 / 移动 + 转移后哈希校验**桌面工具。主界面是一块**类资源管理器的文件管理面板**（左侧导航树 + 右侧文件区，默认单栏、可一键切换双栏），转移与校验能力叠加其上：**校验不过绝不删源**，每一份搬过去的文件都有字节级凭据。

VineTurtleStudio 项目 · 平台：Windows 10 / 11 · 技术栈：**Rust（MSVC）+ egui/eframe 单 exe** · 所属：VineHub 子项目（`plugins/vine.ferry`）

> 独立运行能力 100% 保留：无 hub 时照常双击使用；后续通过 `--rpc` 子命令作为 sidecar 被主面板编排（插件 id `vine.ferry`，exe `vine-ferry.exe`）。

---

## 1. 定位与分工

### 1.1 定位

- **与系统资源管理器并存**：不接管 shell、不注册右键菜单，只做资源管理器做得不够好的那部分——**批量转移 + 字节级校验**。
- **文件管理面板 + 任务式转移引擎**：浏览体验对齐资源管理器；选中文件 → 「复制到… / 移动到…」→ 冲突策略 → 队列执行 → 逐文件哈希回读比对 → 校验列回填 ✓/✗。

### 1.2 与现有子项目的分工（明确不重叠）

| 项目 | 职责 | 与 vineFerry 的边界 |
|---|---|---|
| vineSync | 定时 / 实时**镜像备份**（文件夹对、删除策略、版本备份） | 它管「持续同步」，Ferry 管「一次性转移」 |
| vinePress | 个人媒体库**归档**（识别 / 说明文件 / 批量压缩导出） | 其内嵌文件面板限项目根目录内，**不做替代、互不影响** |
| vinePulse | **只读损坏检测**（格式级验证 + 哈希清单比对） | Ferry 导出的 `.sha256/.sfv` 清单可直接交给 vinePulse 复检 |
| vineCurator / VinePalmaCursor | 媒体资产管理 / 光标编辑 | 无关 |

### 1.3 参考同类软件（取各家之长）

| 软件 | 借鉴点 |
|---|---|
| **TeraCopy** | 复制时流式算源哈希 → 完成后回读目标比对；校验和文件导出；失败文件单独重试 |
| **FastCopy** | 「常规校验」（只重读目标）vs「完美校验」（源+目标都重读）两档；多算法（xxHash3 默认 / SHA-256 等）；校验失败处置策略可选；跨 MAX_PATH（`\\?\` verbatim）；未校验文件打标记的思路（Ferry 改为 SQLite 持久化，不污染文件本身） |
| **rclone** | 传输后自动 checksum 复核、`check` 事后比对、SUM 清单格式互通 |
| **Copy Handler** | 断点续传（列路线图 M6） |
| **FreeFileSync** | 差异/镜像对比（vineSync 已覆盖，Ferry 不重做） |

---

## 2. 技术选型（已定稿）

| 层 | 选型 | 说明 |
|---|---|---|
| 语言 / 运行时 | **Rust（MSVC toolchain）** | 沿用 workspace 根 `Cargo.toml` 与 `.cargo/config.toml`（内置本机 link.exe 与 Windows SDK 路径，普通 PowerShell 可直接 build） |
| GUI | **egui / eframe** | 与 vinePulse 一致：单 exe、无 WebView2 依赖、启动快 |
| 哈希 | `sha2`、`xxhash-rust`（xxh3）、`crc32fast`、`md-5` | 全部 128 KiB 缓冲流式计算，内存恒定 |
| 扫描 | `walkdir`（并行遍历按需加 `jwalk`） | 重连点 symlink/junction 一律剪枝 |
| 持久化 | `rusqlite`（bundled） | 转移历史、校验结果、书签、预设 |
| 对话框 | `rfd` | 目录/文件选择 |
| 图片预览 | `image`（png/jpeg/gif/bmp/webp，无默认特性） | 预览窗格缩放显示 |
| 删除 | `trash` | 删除走系统回收站，**绝不永久删除** |
| 时间 / 序列化 | `chrono`、`serde` / `serde_json` | 日志时间戳、配置与报告 |

运行时依赖：MSVC 运行库（随打包说明分发）。

---

## 3. UI 设计（类资源管理器）

### 3.1 主窗口布局

```
┌────────────────────────────────────────────────────────────────────┐
│ [←][→][↑][刷新]  📁 D:\Photos\2026旅行    [搜索________]  [单栏|双栏] │ ← 地址栏（面包屑可点/可直输 Ctrl+L）+ 工具栏
├────────────┬───────────────────────────────────────────────────────┤
│ ⭐ 快速访问  │ 名称            大小      类型     修改日期    校验     │
│   桌面      │ 📁 京都                               10-01         │ ← 文件区（默认单栏，可切双栏）
│   下载      │ 📁 大阪                               10-02         │    详细信息视图（可切大图标 / 列表）
│ 📁 目录树   │ 🖼 IMG_001.jpg     4.2 MB  JPEG     10-02    ✓     │    点列头排序 · Ctrl/Shift 多选 · 拖放
│ 🖥 此电脑    │ 🖼 IMG_002.jpg     5.1 MB  JPEG     10-02    ⟳     │
│   C: 120 GB │ 📄 路线规划.pdf    820 KB  PDF      10-03    ✓     │ ← 双栏开启后右侧再出一个文件区
│   D: 800 GB │                                                       │    左右各自地址/历史，互相拖放即转移
│   F: 1.8 TB │                                                       │
├────────────┴───────────────────────────────────────────────────────┤
│ ▾ 转移队列（1/3）D:\Photos → F:\备份\Photos    45 MB/s  剩 00:12 ⏸ │ ← 可折叠任务队列 + 总进度/速度/ETA
└────────────────────────────────────────────────────────────────────┘
│ 已选 12 项 · 3.4 GB  ·  D: 可用 800 GB  ·  上次校验：全部通过        │ ← 状态栏
```

### 3.2 文件管理（对齐资源管理器习惯）

- **导航**：左栏「快速访问（可固定书签）+ 此电脑驱动器（含可用空间）+ 可展开目录树」；地址栏为可点击面包屑，也支持直接输入路径（`Ctrl+L`）；`Alt+←/→` 后退前进、`Alt+↑` 上一级、`F5` 刷新。
- **视图**：详细信息（默认，含「校验」列）/ 大图标 / 列表；列头点击排序（名称/大小/类型/修改日期/校验状态）。
- **选择与拖放**：单击选中、`Ctrl` 增选、`Shift` 范围选、`Ctrl+A` 全选；同盘拖放=移动、跨盘=复制（`Ctrl` 反转语义）；**双栏间拖放直接生成转移任务**（走队列 + 哈希校验）。
- **常规操作**：`Enter` 打开、`F2` 重命名、`Delete` 移入回收站、`Ctrl+C/X/V` 复制/剪切/粘贴（粘贴时弹冲突对话框）、新建文件夹/文件。
- **右键菜单**：打开 / 在资源管理器中定位（`explorer /select,`）/ 重命名 / 删除（回收站）/ 复制到… / 移动到… / **校验哈希** / **导出校验和清单** / 属性。
- **搜索框**：当前目录递归文件名即时过滤。

### 3.3 转移与校验交互

1. 选中若干文件/目录 → 工具栏或右键「复制到… / 移动到…」（或双栏拖放）；
2. **空间预检**：比对所选总大小与目标可用空间，不足则预警并可取消；
3. **冲突策略**对话框（逐条可改）：跳过 / 覆盖 / 自动改名（`name (2).ext`）/ 合并目录；
4. 进入底部**转移队列**：文件级进度 + 字节速度 + ETA + 暂停/继续/取消；
5. 完成后列表「校验」列回填：`⟳` 校验中 → `✓` 通过 / `✗` 失败（失败可一键「仅重试失败项」）；
6. 结果写入 SQLite 历史，可导出 CSV / JSON 报告与 `.sha256` / `.sfv` 校验和清单。

### 3.4 其它交互约定

- `Ctrl+K` 命令面板（模糊搜索全部命令）；`Ctrl+Shift+D` 切换单/双栏。
- 深色 / 浅色主题跟随系统，图标风格贴近资源管理器；快捷键与资源管理器一致，零学习成本。
- **转移预设**：保存「源 → 目标 + 哈希算法 + 校验档位 + 冲突策略」，一键重复执行。

---

## 4. 功能里程碑

| 阶段 | 内容 | 验收 |
|---|---|---|
| **M1 核心引擎** | 纯逻辑库（零 egui 依赖）：任务计划生成、冲突策略、安全复制（`*.ferry.tmp` → 校验 → 原子改名 → 回写 mtime）、流式哈希、常规/完美校验比对、失败处置与重试、取消、`\\?\` 长路径、symlink/junction 剪枝、空间预检 | `cargo test` 全绿（标准哈希向量、冲突四策略、校验失败路径、长路径用例） |
| **M2 文件管理面板** | Explorer 式浏览：导航树 / 地址栏 / 视图切换 / 排序 / 多选 / 右键菜单 / 回收站删除 / 新建 / 重命名 / 定位；此阶段复制走原生（无校验），先把「像资源管理器」跑通 | 手测清单：日常浏览操作与资源管理器习惯一致 |
| **M3 转移队列 + 哈希校验** | 队列执行、冲突对话框、双栏拖放即任务、进度（速度/ETA/暂停）、校验状态列、常规/完美两档、失败处置（重试 → 报告 / 改名 `.verify_err` / 删除目标 tmp）、算法可选 | 真实目录跨盘复制 + 校验通过；人为篡改目标后校验必报 ✗ |
| **M4 安全移动 + 报告** | 移动 = 复制 → **校验通过才删源**（不过绝不删源）、SQLite 历史、CSV/JSON 报告、`.sha256/.sfv` 清单导出、事后校验（与清单比对 / 对比两目录）、校验结果持久化 | 校验失败的移动不删源；重启后 ✓/✗ 仍在 |
| **M5 hub 接入** | `--rpc` 子命令（argv 最早分流，跳过 GUI 初始化）、`plugins/vine.ferry/plugin.json`（`kind: "sidecar"`）、能力 `transfer.start` / `verify.check` / `task.cancel`；协议级集成测试 | hub 内发起一次真实转移并收到进度/终态 |
| **M6 路线图** | 见 §8 | 逐项独立交付 |

---

## 5. 核心引擎设计

### 5.1 一次转移任务的执行流

```
1. 计划阶段
   - 枚举所选项（目录递归 walk，重连点 symlink/junction 剪枝，权限拒绝子树跳过并记录）
   - 产出 change list：{ kind: add | update | skip, rel_path, size }
   - 冲突判定：目标已存在 → 按策略 跳过 / 覆盖 / 改名 / 合并
   - 空间预检：Σ size > 目标可用空间 → 预警，可取消
2. 复制阶段（按「复制线程数」并行，默认 4；每文件失败自动重试 3 次、递增延迟）
   - 写入 <dest>\<rel>.ferry.tmp
   - 边写边算源哈希（一次读盘，流式，128 KiB 缓冲）
   - 常规校验：回读目标 tmp 算哈希 → 与源哈希比对
     完美校验：源也重读再算一次（可检出读源时的瞬时错误）
   - 比对一致 → 回写源 mtime → 原子改名到最终名
     不一致   → 清理 tmp，进入失败处置（重试 N 次 → 仍失败记 ✗ 并按设置处置）
3. 删除阶段（仅「移动」）
   - 该文件校验通过 → 才删除源文件（跨盘移动 = 复制 + 校验通过后删源）
   - 校验未通过     → 源文件原样保留，任务记 ✗
4. 收尾
   - 更新 SQLite（历史 + 每文件校验状态）、广播进度/完成事件、按设置弹通知
   - 移动模式下，源目录清理仅限「已全部成功迁移」的空目录
```

### 5.2 校验档位与算法

| 项 | 说明 |
|---|---|
| 档位 | **常规校验**（默认：源边写边算，写完重读目标比对）/ **完美校验**（源也重读，检出读源瞬时错误，速度约减半） |
| 算法 | `xxh3`（默认，最快）/ `sha256`（清单导出默认，兼容性最好）/ `crc32` / `md5`；传输校验与清单导出可各选各的 |
| 失败处置（四选一，对齐 FastCopy） | ① 仅报告 ② 目标改名 `<name>.verify_err` ③ 删除目标 tmp ④ 重试后再定 |
| 事后校验 | 「与清单比对…」（读 `.sha256/.md5/.sfv`）与「对比两个目录…」（大小+mtime 命中者再比哈希）；只读、可取消，结果进历史，与 vinePulse 清单格式互通 |

### 5.3 关键不变量（必须由测试锁死）

1. **校验不过绝不删源**——移动模式的唯一安全底线；
2. 目标位于源内部、或源==目标路径 → 一律拒绝（杜绝复制进自身子目录的无限递归）；
3. 写盘必经 `<dest>.ferry.tmp` → 校验 → 原子改名，断电/中断不留半截正式文件；
4. 源哈希与写入共用同一次读盘遍历（仅完美校验二次读源）；
5. 取消在**文件粒度**响应，正在写的文件完成当前块后中止并清理 tmp；
6. 删除一律走系统回收站（`trash`），**绝不永久删除**；
7. 扫描跳过重连点 symlink / junction（防环形联接与越界），硬链接按独立普通文件对待；
8. 长路径：根路径转 verbatim（`\\?\D:\…`、UNC → `\\?\UNC\…`），浏览/扫描/复制全覆盖。

---

## 6. 数据模型

### 6.1 数据目录（M6 便携模式）

- 安装版：`%LOCALAPPDATA%\vineFerry\`（目前仅 `history.db`；未来 `config.json`）
- 便携版：exe 旁存在 `vineferry.portable` 标记 → 数据落在 `<exe>\portable_data\`（`history.db`），状态栏显示「便携模式」
- 打包：`powershell -ExecutionPolicy Bypass -File tools\package-portable.ps1`（release exe + 标记 + 使用说明 → `dist-portable\vineferry-portable.zip`；运行中实例拒绝打包；已有 `portable_data` 保留不进 zip）

```jsonc
{
  "copyThreads": 4,               // 1~16 并行复制线程数
  "hashAlgo": "xxh3",             // xxh3 | sha256 | crc32 | md5（传输中即时校验用）
  "exportAlgo": "sha256",         // 导出清单默认算法
  "verifyMode": "conventional",   // conventional（常规）| perfect（完美）
  "onVerifyFail": "report",       // report | rename | delete | retryThenReport
  "retryTimes": 3,                // 每文件失败自动重试次数
  "conflictPolicy": "ask",        // ask | skip | overwrite | rename | merge
  "preserveMtime": true,          // 回写源修改时间
  "precheckSpace": true,          // 复制前空间预检
  "confirmDelete": true,          // 删除前确认
  "paneLayout": "single",         // single | dual（文件区布局）
  "viewMode": "details",          // details | icons | list
  "showHidden": false,
  "bookmarks": [ { "name": "照片", "path": "D:\\Photos" } ],
  "presets": [
    { "name": "照片归档", "dest": "F:\\备份\\Photos", "mode": "move",
      "hashAlgo": "sha256", "verifyMode": "conventional", "conflictPolicy": "rename" }
  ],
  "theme": "system",              // system | dark | light
  "language": "zh"                // zh | en（M6）
}
```

### 6.2 SQLite `state.db`（同目录）

| 表 | 字段 | 用途 |
|---|---|---|
| `transfer_log` | id, started_at, finished_at, mode(copy/move), src, dest, total, ok, bad, skipped, verify_mode, hash_algo, status, message | 任务历史 |
| `file_result` | log_id, rel_path, size, src_hash, dst_hash, verified, status, message | 逐文件校验结果（✓/✗ 持久化依据） |
| `preset` | id, name, dest, mode, hash_algo, verify_mode, conflict_policy | 转移预设（与 config.json 冲突时以 SQLite 为准） |

---

## 7. 目录结构（规划）

```
vineFerry/
├─ README.md                  # 本文件
├─ Cargo.toml                 # package: vine-ferry（lib + bin 同包，lib 供测试）
└─ src/
   ├─ main.rs                 # 入口：--rpc 最早分流 → RPC 模式；无参数 → eframe GUI
   ├─ lib.rs                  # 模块导出，供 GUI/RPC/测试共用
   ├─ core/                   # 纯逻辑层（零 egui 依赖，cargo test 直接覆盖）
   │  ├─ mod.rs
   │  ├─ plan.rs              # 计划生成、枚举、冲突判定、空间预检
   │  ├─ copy.rs              # 安全复制（tmp → 校验 → 原子改名 → 回写 mtime）、重试
   │  ├─ hash.rs              # 流式哈希（xxh3/sha256/crc32/md5 统一接口）
   │  ├─ verify.rs            # 常规/完美校验、事后比对、清单解析（.sha256/.md5/.sfv）
   │  ├─ conflict.rs          # 冲突四策略与自动改名去重
   │  ├─ report.rs            # CSV / JSON 报告、校验和清单导出
   │  └─ paths.rs             # 长路径 verbatim、UNC、内部路径校验
   ├─ storage/
   │  ├─ config.rs            # config.json 读写（原子写、缺省项回填）
   │  └─ db.rs                # SQLite 建表/迁移、历史与校验结果读写
   ├─ runtime/
   │  └─ queue.rs             # 转移队列协调（互斥执行、暂停/恢复、进度事件、取消）
   ├─ rpc.rs                  # [M5] --rpc 模式：plugin.info / transfer.start / verify.check / task.cancel
   └─ ui/
      ├─ mod.rs               # 状态机、页面/面板切换
      ├─ browser.rs           # 资源管理器式面板（导航树/地址栏/文件表/双栏）
      ├─ transfer_view.rs     # 转移队列、冲突对话框、校验结果
      └─ settings_view.rs     # 设置、书签、预设
```

分层原则（沿用 VinePalmaCursor / vinePress 约定）：**core 与 storage 为纯逻辑库层，零 egui 依赖**，`cargo test` 直接覆盖；`ui/` 与 `main.rs` 只做接线。

---

## 8. 路线图（M6 及以后）

- [x] 多标签页（2026-10-16，`PaneGroup`：每栏独立标签组，标签条显示目录名 +「+」新建（继承当前目录）+「×」关闭（最后一标签不可关），点击切换；双栏两侧各自成组；地址/历史/选择按标签隔离）
- [x] 只读预览窗格（2026-10-15，工具栏「预览」开关，右侧窗格：png/jpeg/gif/bmp/webp 缩放显示 + ≤256 KiB 文本速览，含 NUL 判定二进制拒绝、超大图降采样、缓存按路径+大小+mtime）
- [x] 从资源管理器拖文件**进入**窗口加入队列（2026-10-15，拖入活动栏目录自动开始复制，忙碌/目标内项目自动拒绝）
- [x] 双栏同步导航（2026-10-15，双栏 + 「同步导航」开关：跳转/上一级/地址栏一侧变化另一侧跟随）
- [x] 中英双语（2026-10-15，`ui/i18n.rs`：全局原子语言 + `t(zh, en)`，工具栏末尾「中/EN」按钮即时切换；覆盖工具栏、导航树、文件表（列头/类型列/右键菜单）、地址栏提示、对话框、状态栏、历史/预设窗口、完成后动作菜单——主框架全双语；深层错误消息（IO/校验详情）保持中文）
- [x] 便携版（2026-10-15，`vineferry.portable` 标记 → 数据落 `<exe>\portable_data\`，状态栏便携徽标；`tools\package-portable.ps1` 打 zip，运行中实例拒绝打包、已有数据保留）
- [x] 断点续传（2026-10-15，取消时保留 `*.ferry.tmp` + `*.ferry.resume` journal（源大小+mtime），下次源未变则从断点续写并先哈希已写前缀；源变更自动放弃续传；成功/不保留策略清理 tmp+journal；`CopyOptions.resume` 默认开启）
- [x] 同大小同时间文件快速跳过（2026-10-15，`CopyOptions.skip_identical` 默认开启，CLI `--no-skip-identical` 可关）
- [x] 命令行 CLI 模式（2026-10-15，`vine-ferry copy|move <源>... <目标> [-c 策略] [-a 算法] [--json/--csv]`，退出码 0/1/2，终端进度条，报告导出）
- [x] 转移预设增强（2026-10-15：
  - 完成后动作：工具栏「完成后：无/关机/睡眠/休眠/锁定」菜单，转移全部完成后触发一次并复位（关机/休眠 30 秒缓冲，`shutdown /a` 可取消）
  - 转移预设：SQLite `presets` 表（名称唯一、源/目标/模式/间隔秒/启用/上次运行），「预设」窗口可保存当前栏目录为目标待设、设目标、启用开关、立即运行、删除；启用且间隔 > 0 的预设每 5 秒检查一次，到点且空闲时自动运行，源不存在则跳过本轮）
- [x] 校验结果导出给 vinePulse 复检的双向打通（2026-10-15：Ferry → Pulse：历史窗口选中校验 run →「导出复检清单…」将不一致/缺失/读取失败项写为 `.sha256`，vinePulse「导入哈希清单」直接复检；Pulse → Ferry：「按清单校验…」已兼容 `.sha256/.md5/.sfv` 三种格式）
- [ ] Shell 右键集成（需代码签名，默认不做，用户显式开启才装）

---

## 9. 明确不做（避免与现有子项目重叠）

| 不做 | 归属 |
|---|---|
| 定时 / 实时镜像同步、删除策略、版本备份 | vineSync |
| 项目内归档、说明文件、批量压缩导出 | vinePress |
| 文件损坏格式检测（FLAC/PDF/ZIP 结构级验证） | vinePulse |
| 媒体库管理、光标编辑 | vineCurator / VinePalmaCursor |
| 接管系统右键菜单（默认） | 路线图可选项 |

---

## 10. 开发与构建（规划）

```powershell
# 环境：Rust stable-msvc；沿用 workspace 根 .cargo/config.toml
cargo build                 # 构建（产物：target\release\vine-ferry.exe）
cargo test                  # 核心引擎单测（纯逻辑层）
cargo clippy --all-targets  # 代码检查（应零警告）
cargo run                   # 启动 GUI
cargo run -- --rpc          # [M5] sidecar 模式（走 stdin/stdout JSON-RPC）
cargo run -- copy <src...> <dst> [-c rename] [-a xxh3] [--json out.json]  # [M6] CLI 模式
cargo run -- move <src...> <dst>                                          # [M6] 移动（校验通过才删源）
```

## 11. 开发进度

- [x] **M1 核心引擎**（2026-10-15）：`core/` 纯逻辑层全量落地并接入 workspace
  - `paths.rs` 长路径 verbatim（正斜杠归一化、UNC）、源内部/同路径拒绝、重连点判定、同盘判断
  - `hash.rs` 统一流式哈希接口（xxh3 / sha256 / crc32 / md5，128 KiB 缓冲）
  - `conflict.rs` 冲突四策略（Ask 触发计划期报错）+ 自动改名去重（含子目录）
  - `plan.rs` 计划生成（walkdir、重连点剪枝、单文件/目录、大小统计）+ 空间预检（`GetDiskFreeSpaceExW`）
  - `copy.rs` 安全复制（`*.ferry.tmp` → 边写边算源哈希 → 回读校验 → 原子改名 → 回写 mtime）、重试、取消、失败处置（报告/保留 `.verify_err`/删除 tmp）、同盘移动 rename 快路径、**校验通过才删源**、源空目录清理
  - `verify.rs` 清单生成/解析（`.sha256`/`.sfv` 兼容 `hash *path` 与 `hash  path`）、按清单事后校验、目录对比（大小 → 哈希两级）
  - `report.rs` CSV（RFC 4180 转义）/ JSON 报告导出
  - 测试：**43 项单测全绿**（哈希标准向量、冲突、计划、复制/移动/取消/失败路径、清单往返与篡改检测、报告转义）；`cargo clippy --all-targets` 零警告
- [x] **M2 文件管理面板**（2026-10-15）：
  - 单/双栏切换（Ctrl+Shift+D）：双栏各带地址栏/历史/选择，右键"发送到另一栏"即转移
  - 左侧导航树：快速访问（桌面/下载/文档）+ 此电脑驱动器列表，按层级展开（懒加载缓存）
  - 文件表：名称/大小/修改/类型四列（`TableBuilder` 虚拟滚动）、目录优先、重连点 🔗、Ctrl/Shift 多选、右键菜单（打开/定位/重命名/回收站删除/复制到/移动到/发送到另一栏）
  - 快捷键：Delete 回收站 · F2 重命名 · F5 刷新 · Ctrl+C/X/V 复制/剪切/粘贴（内部剪贴板，粘贴走转移引擎）· Alt+←/→/↑ 导航
  - 对话框：重命名 / 新建文件夹（居中弹窗、回车提交）
  - 状态栏：当前目录统计 + 剪贴板提示 + 消息区 + 转移进度条（文件数 + 当前文件 + 取消按钮）
- [x] **M3 转移队列 + 哈希校验接线**（2026-10-15）：后台线程跑 `run_transfer`（M1 引擎），Mutex 共享 Progress/RunSummary，UI 轮询刷新；空间预检前置报错；完成后自动刷新双栏并落 `last_summary`；结果 CSV 一键导出（`rfd` 存盘 + RFC 4180 转义）
- [x] **M4 SQLite 历史 / 清单 / 事后校验**（2026-10-15）：
  - `core/storage.rs`：SQLite（`%LOCALAPPDATA%\vineFerry\history.db`，WAL），表 `runs`/`outcomes`/`verify_runs`/`verify_items`，转移完成自动落库、校验完成自动落库
  - `ui/jobs.rs`：后台线程清单生成（`.sha256`）与按清单事后校验（复用 `verify.rs`），进度共享 + 状态栏进度条
  - 工具栏新增：导出清单… / 按清单校验…（互斥忙碌态）/ 历史
  - 历史窗口：转移历史表（时间/结果/任务/字节）→ 点选展开逐文件明细；校验历史表 → 点选展开逐项明细；手动刷新
  - 测试：**46 项单测全绿**（新增 storage 往返 3 项）；`cargo clippy --all-targets` 零警告；GUI 冒烟通过
- [x] **M5 hub 接入**（2026-10-15）：
  - `vine-plugin-api` 新增方法 `transfer.start` / `verify.start` 及 DTO（`TransferStartParams/Result`、`VerifyStartParams/Result`）
  - `src/rpc.rs`：`--rpc` 最早分流（跳过 eframe），stdio 行分隔 JSON-RPC 协议 v1——`plugin.info`（capabilities: transfer/verify/cancel/history）、`ping`、`transfer.start`（ack 回 taskId → 后台 `run_transfer` → `task.progress` 通知 → 同 id 终态响应含逐文件 outcomes，自动落 SQLite）、`verify.start`（同上，终态含逐项 items + 四态统计）、`history.list`（runs + verifyRuns）、`task.cancel`（通知/请求双形态）、stdin EOF 等待在途任务 10s
  - `plugins/vine.ferry/plugin.json`：sidecar 清单（entry 指向 `target/debug/vine-ferry.exe`，commands: transfer.start / verify.start / history.list，fs.read/write=user-picked，net/exec=false）
  - 冒烟：`plugin.info`/`ping`/`transfer.start` 经 stdio 全链路通过（ack → progress → 终态 ok=1 verified，目标文件落地，历史入库）
  - clippy 零警告；vine-ferry 46 项 + vine-plugin-api 21 项测试全绿
- [x] **M6 路线图 · 已完成五项**（2026-10-15）：
  - 快速跳过：`CopyOptions.skip_identical`（size+mtime 一致直接 Skipped，CLI `--no-skip-identical` 可关）
  - CLI 模式：`vine-ferry copy|move <源>... <目标> [-c 策略] [-a 算法] [--json/--csv]`，退出码 0/1/2，终端进度条；`main.rs` 首参 `copy|move|-h|--help` 最早分流
  - 拖放加入队列：从资源管理器拖入 → 复制到活动栏目录（忙碌/目标内项目拒绝）
  - 双栏同步导航：双栏 + 「同步导航」开关（跳转/上一级/地址栏一侧变化另一侧跟随）
  - 只读预览窗格：工具栏「预览」开关，右侧窗格 png/jpeg/gif/bmp/webp 缩放 + ≤256 KiB 文本速览（NUL 判定二进制、超大图降采样、按路径+大小+mtime 缓存）；新增 workspace 依赖 `image`
  - 验证：clippy 零警告 · **49 项测试全绿** · GUI 冒烟 + RPC stdio 全链路 + CLI 冒烟均通过
- [x] **M6 路线图 · 本轮八项**（2026-10-16）：
  - 便携版：`paths::is_portable()`/`data_dir()`（`vineferry.portable` 标记 → `<exe>\portable_data`，状态栏徽标；`tools\package-portable.ps1` 打 zip，运行中实例拒绝、已有数据保留不进 zip）
  - vinePulse 双向打通：历史窗口「导出复检清单…」（非 Ok 项 → `.sha256`，Pulse 导入复检）；反向「按清单校验」已兼容三种格式
  - 断点续传：取消保留 `*.ferry.tmp` + `*.ferry.resume` journal（源大小+mtime 校验），续写前先哈希已写前缀，源变更自动放弃；`CopyOptions.resume` 默认开启
  - 完成后动作：工具栏菜单（无/关机/睡眠/休眠/锁定），转移完成触发一次即复位
  - 转移预设：SQLite `presets` 表 + 「预设」窗口（保存/设目标/启停/立即运行/删除）+ 5 秒 tick 到点自动运行（空闲才跑，源不存在跳过）
  - 中英双语：`ui/i18n.rs`（原子语言 + `t()`），工具栏「中/EN」即时切换，主框架全双语
  - 多标签页：`pane::PaneGroup`（每栏标签组，+新建/×关闭/点击切换，双栏各自独立），地址/历史/选择按标签隔离
  - 验证：clippy 零警告 · **66 项测试全绿** · GUI 冒烟通过
- [x] **UI 整体重设计**（2026-10-16）：参考资源管理器/360 文件管理器
  - `ui/theme.rs` 全局主题：CHROME/BORDER/TEXT/ACCENT/SELECTION 等色板 + `bar_frame`/`tab_strip_frame`/`flat_buttons`/`visuals`/`apply`
  - 布局重排：顶部命令栏（flat 按钮分组，中/EN 靠右）→ 底部状态栏全宽 → 左导航 → 右预览 → 中央零边距；原中央工具栏删除
  - 地址行合并单行：[←][→][↑] + 地址输入（宽自适应，Ctrl+L 聚焦）+ 右端「选择目录…/刷新」；标签条 active tab 白底直角样式
  - 文件表新列序：名称|修改日期|类型|大小（大小右对齐、`type_label` 扩展名映射、行高 24/表头 26/HEADER_BG 底色+分隔线）
  - 导航树：小节标题 + painter 自绘 chevron 三角形（字形 ▸ 会渲染成方框）、label 点击导航
  - 修表格溢出：名称列 `Column::remainder().at_least(200).resizable(false)`（builder `resizable(true)` 下 remainder 非末列被当 exact 固定导致总宽超出窗口，见 egui_extras `table.rs` L627-646）
  - 验证：clippy 零警告 · **66 项测试全绿** · DPI-aware 截图冒烟（列序/chevron/大小列完整显示均核对通过）
- [x] **资源管理器化第二轮（便捷性）**（2026-10-16）：
  - 面包屑地址栏：祖先路径分段按钮（点击逐级导航，头部「…」折叠，尾部空白点击进入编辑态，Enter/失焦提交）+ ⟳ 刷新 + 搜索框即时过滤（双栏 110/单栏 160）
  - 后退/前进/上一级禁用态感知（`can_back/can_forward/can_up`），历史按标签隔离
  - 表头点击排序：`SortKey`（名称/日期/类型/大小）+ `sort_entries`（目录优先恒定，组内 asc/desc，大小/日期数值序，名称大小写不敏感）、`▲/▼` 方向指示、排序/过滤后清选择
  - 命令栏 Explorer 化：「新建 ▼」下拉（文件夹…/文本文档）| ✂ + painter 自绘双框复制/剪贴板粘贴（`⧉`(29C9) 等字形全字体链缺失，自绘避免 tofu）+ ✏(F2) + 🗑(Delete) | 选择目录… 移入 | 右端「…」更多菜单（导出清单/按清单校验/历史/预设）
  - 快捷键补齐：Ctrl+A 全选（遵守过滤）、Esc 清除选择；状态栏显示选中 N 项与合计大小、过滤时显示筛选提示；`无匹配项` 空态提示
  - `reload()` 按文件名保留选中（导航往返不丢选择）；`view_indices()` 过滤→全量索引映射，选中/锚点实时裁剪
  - 自截图验证管线：`VINEFERRY_SHOT=<png>` → 第 5 帧 `ViewportCommand::Screenshot` → `Event::Screenshot` → `image::save_buffer`（wgpu 后端不支持 `EFRAME_SCREENSHOT_TO`，故内置；须每帧 `request_repaint()`）
  - 字形覆盖实测（WPF `GlyphTypeface.CharacterToGlyphMap`）：`⧉▾⋯` msyh/egui 内置字体全缺 → 避用；`▼…` msyh 有 → 采用；`›▲⟳✂✏🗑📁📄🔗` 经截图核验可渲染
  - 验证：clippy 零警告 · **71 项测试全绿**（新增排序/过滤/选择/历史 5 项）· 应用内自截图核验无 tofu
- [x] **资源管理器化第三轮（双栏/右键/网格/剪贴板）**（2026-10-16）：
  - 双栏焦点隔离：`render_pane` 去掉每帧无条件 Activate（旧代码右栏后处理永远覆盖左栏）；仅真实交互（点击/右键）经 cover rect 发 Activate；组容器按焦点画 2px ACCENT / 1px BORDER 描边（`StrokeKind::Inside`）
  - App 级右键菜单：`PaneEvent::ContextMenu(pane, pos, MenuTarget)` → `CtxMenu` + `egui::Area`(Foreground, `fade_in(false)`) + `Frame::popup` + `menu_style` 扁平化；Items 型（打开/定位/剪切/复制/重命名/删除/复制路径/复制到/移动到/发送到另一栏/属性）+ Background 型（查看▸/排序方式▸/刷新/新建▸/粘贴/全选/属性）；打开 160ms 内不响应外部点击（防首帧自关）、Esc 关闭、点击外部关闭、动作选择关闭；子菜单箭头「▸」(25B8) tofu → 改「›」(203A)
  - 剪贴板系统互通 + AI 排除：`ui/clip.rs` Win32 剪贴板（`OpenClipboard/EmptyClipboard/SetClipboardData`）；`CF_HDROP` 编解码（`encode_hdrop`/`decode_hdrop` 纯函数 + 往返测试）；`action_copy_cut` 同步写系统剪贴板，`clipboard_paste` 内部空时读 `CF_HDROP`；egui 输出经 `OutputCommand::CopyText` 被 `take_copy_text` 拦截改走自写剪贴板并附注册格式 `ExcludeClipboardFromMonitorProcessing`="1"（防第三方 AI 剪贴板接管）；workspace windows-sys features 扩为 DataExchange/Memory/Ole
  - 网格视图：`Pane.view_mode`（List/Grid）+ `render_grid`（CELL 96x100、ICON 56、`ScrollArea::show_rows` 虚拟化、id_salt 按 pane_idx 区分防双栏 ID 冲突、选中/hover 底、📁/🔗/📄 或缩略图、LayoutJob 单行省略号居中）；命令栏「列表/图标」切换 active 栏
  - 缩略图缓存：`ui/thumbs.rs` `ThumbCache`（768 条 LRU、每帧 8 加载预算、`image` 解码 + `thumbnail(96,96)`、4096px/64MB limits、非图扩展名快速短路）
  - 驱动器卷标：`tree.rs` `drive_display()`（`GetVolumeInformationW` 读卷标，回退 `本地磁盘 (X:)`）
  - 预览文本可选中复制：preview.rs `Content::Text` 改 `TextEdit::multiline().font(Monospace).frame(NONE)`
  - 属性对话框：`DialogKind::Properties`（名称/位置/类型/大小/修改时间 + 关闭）
  - 键盘补齐：Backspace=上一级、Enter=打开、Ctrl+Shift+N=新建文件夹；Escape 在菜单打开时不清选择
  - 截图辅助 env：`VINEFERRY_DUAL=1`（双栏）、`VINEFERRY_GRID=1`（两栏网格）、`VINEFERRY_MENU=x,y`（开 Background 菜单下一帧截图）
  - 验证：clippy 零警告 · **77 项测试全绿**（新增 hdrop 往返/拒绝、view_mode 默认、sort_key 标签、drive_display、node_label 回退 6 项）· 自截图核验（双栏焦点描边/网格/菜单扁平白底无 tofu/卷标/SID 冲突红字消失）
- [x] **资源管理器化第四轮（系统图标/文件夹缩略图/焦点框/侧栏稳定）**（2026-10-16）：
  - 系统图标：`ui/shell_icons.rs` `SHGetFileInfoW`（SHGFI_ICON + LARGE/SMALLICON）→ HICON → GetIconInfo + CreateDIBSection + DrawIconEx 提取 RGBA → egui TextureHandle；`ShellIcons` LRU 256 条（dir 缓存 `d{size}`、reparse 缓存 `r{path}`、文件缓存 `f{size}{ext}`）；列表 16px / 网格 48px；替换 📁🔗📄 emoji
  - 文件夹内容缩略图：`ui/thumbs.rs` `FolderPeek`（每帧 4 次 read_dir 预算、每文件夹缓存前 4 张图片路径）；网格视图 `draw_folder_preview`：黄色文件夹 + 2×2 小缩略图叠加（有图时），无图回退系统文件夹图标
  - 焦点蓝框移除：`render_group` 删除组容器 rect_stroke 描边（用户不需要视觉焦点指示）
  - 侧栏悬停抖动修复：`Panel::left("nav")` 改 `exact_size(210.0).resizable(false)`（旧 resizable 导致悬停时右侧 CentralPanel 宽度变化 → 内容左右晃动）
  - 验证：cargo build 零警告 · **77 项测试全绿** · 自截图核验（系统黄色文件夹/文件/PS1 图标、无蓝框、侧栏稳定）

- [x] **资源管理器化第五轮（框选/状态栏视图切换）**（2026-10-16）：
  - 框选（marquee）：`Pane.marquee: Option<(Pos2, Pos2)>`；cover 区域改 `Sense::click_and_drag()`，`drag_started` 记起点并清选择（Ctrl 保留）、`dragged` 更新终点、`drag_stopped` 调 `apply_marquee` 按矩形与 cell_rects 相交批量 `selected.insert`；渲染半透明白底 + ACCENT 描边选框；grid cell 与 list 行均推入 `cell_rects`
  - 状态栏右下角视图切换：`Panel::bottom("status")` 内 `Layout::right_to_left` 加「列表/图标」selectable_label，与顶部工具栏同步
  - 验证：cargo build 零警告 · **83 项测试全绿** · 自截图核验状态栏按钮

- [x] **资源管理器化第六轮（整行点击/工具栏补充/拖拽）**（2026-10-16）：
  - 整行点击选中：列表行改用 `row.response()`（移到所有 `row.col()` 之后——egui_extras 要求最后调用，旧位置 panic）处理 clicked/double_clicked/secondary_clicked；Name/图标去掉独立 `Sense::click`，整行响应
  - 双栏分隔 + 激活指示：`cols[0].painter().vline()` 画分隔线；`render_group` 加 `is_active` 参数，活动栏顶部 3px ACCENT 蓝条
  - ID 冲突修复：TableBuilder `id_salt("pane_table_{pane_idx}")`、header_cell 交互 id 带 pane_idx、地址栏/筛选栏 `id_salt`（双栏同帧不再闪红字）
  - 列宽自适应：`Column::initial(avail_w * ratio).at_least(min)` 比例缩放 + `resizable(false)`，小窗口不再溢出
  - 鼠标指针：列表行/网格格 hover 时 `PointingHand`
  - 拖拽转移：`ItemDrag { paths }` egui DnD payload；网格格 `drag_started` 注册、cover `dnd_hover_payload` 高亮、`dnd_release_payload` 推 `PaneEvent::DragDrop` → `start(paths, dest, Copy)`
  - cover 区域重构：`ui.interact` 在列表/网格之前注册（低 z 序），空白区框选/单击清空/拖放目标；子控件优先接收点击
  - 工具栏补充：Back/Forward/Up/Refresh/全选(Ctrl+A)/属性(Alt+Enter) 按钮
  - 验证：cargo clippy 零警告 · **83 项测试全绿** · release 构建 + 自截图核验（整行点击/双栏分隔/工具栏按钮）

- [x] **资源管理器化第七轮（拖拽转移/就地改名）**（2026-10-16）：
  - 列表拖拽转移修复：`TableBuilder.sense` 改 `Sense::click_and_drag()`（旧 `Sense::click()` 无 drag sense → 拖动时 hit_test 落到 cover → 显示框选）；行 `drag_started_by(Primary)` 时未选中先 `select` 再 `dnd_set_drag_payload(ItemDrag)`（仿网格格）；cover `drag_started` 加守卫——起点落在 `cell_rects` 内则不启动 marquee（防穿透）
  - 就地改名（Explorer 语义）：`Pane` 加 `renaming: Option<usize>` + `rename_text` + `rename_focused` + `last_click: Option<(usize, f64)>`；慢二次点击（间隔 > 0.35s 且非 double_clicked 且单选中且点击落在名称列内）触发；名字列在 `renaming == Some(idx)` 时渲染 `TextEdit::singleline` 替代 Label，`gained_focus` 时经 `TextEditState::load_state` + `cursor.set_char_range` 选中主文件名（stem，不含扩展名，目录/点文件全选）；Enter/失焦提交、Esc 取消；提交推 `PaneEvent::Rename(pane, old, new)` → mod.rs `fs::rename` + `reload` + 树失效；`reload`/`navigate` 清除改名态；double_clicked 时清改名态防冲突
  - 验证：cargo build 零警告 · **77 项测试全绿**（源码 `#[test]` 实数 77，此前 83 计数有误）· release 构建 + 自截图核验（双栏列表渲染无回归）

- [x] **资源管理器化第八轮（计划语义/交互/性能/窗口，一次通过修 8 项）**（2026-10-16）：
  - 文件夹复制不再打散：`plan.rs` 目录项走 Explorer 合并语义（目标已存在 → `ChangeKind::Update`，不计冲突、Rename 不改名）、顶层目录命中同名时整树去重前缀（`src (2)/…`）、所有目录（不只顶层）推入 `PlanItem`（新增 `is_dir: bool`）；`copy.rs::run_transfer` 对目录项 `create_dir_all`（long path）+ Move 模式完成后逆序 `fs::remove_dir` 清空源空目录
  - 同路径/拖回原目录取消：`pane::filter_drop_paths`（源=目标或源父目录=目标 → 剔除，大小写/斜杠归一；全剔则不派发 `DragDrop`），3 项单测
  - 单击/双击改名可靠：慢击阈值 0.35→0.5s（egui `InputOptions.max_double_click_delay`=0.3s 与 OS 无关）；首击只登记 `pending_rename` 意向，约 0.32s（> 双击窗口）后无双击才提交——`double_clicked` 分支清意向，双击首帧不再闪出改名框；`reload`/F2 对话框入口同步清 pending
  - 点行间隙不再误清选择：cover `clicked_by`/`drag_started` 改用 `cell_rects` union（含 3px 行间隙）判定 `on_list`，起点在行内（含间隙）不启动框选、不触发清选
  - 拖动 ghost：`pane::render_drag_ghost`（Foreground `Area` + 黑底半透明圆角 + 首文件名/条目数，`pointer.any_down` + `DragAndDrop::payload` 门控、`interactable(false)`）
  - 启动不再卡死：`build_plan`/concat/空间预检全部移入 worker 线程（`TransferShared` 增 `scanning/label/error`），计划错误/空计划/空间不足经 `poll` 回 UI 落消息；扫描期取消 → 不落库、不触发完成后动作；状态栏紧凑行显示「计划扫描中…」
  - 进度截断修复：进度行 `ProgressBar.desired_width(160)+show_percentage()`、当前文件路径 `Label::truncate()`（浮动窗口同步）
  - 属性窗口：`CtxAct::Properties(Vec<PathBuf>)`（空=当前目录，右键多选/工具栏均传全量选中）→ 多选汇总（N 个项目/类型构成/递归合计、多位置标记）+ 单选文件夹递归大小（`walkdir` + 重连点剪枝不跟联接）+ 属性行（`MetadataExt::file_attributes` 0x1/0x2/0x4/0x20 → 只读/隐藏/系统/存档）；`PropsInfo` 打开时计算一次缓存、关闭清空
  - 转移窗口：浮动 `egui::Window::open(&mut show_transfer_win)`（活动转移标签、扫描 spinner、进度条+计数+截断当前文件、取消按钮、X 关闭经状态栏「详情」重开），底部状态栏保留一行紧凑摘要
  - 预存 clippy 清理：`render_group` too_many_args allow、shell_icons 冗余 `*mut c_void` cast×4、`chunks_exact(4)→as_chunks::<4>()`
  - 验证：cargo clippy --all-targets **零警告** · **86 项测试全绿**（新增 pane 拖放 3 项、`compute_props` 3 项、计划目录语义/移动清源测试更新）· release 构建 + 主界面/右键菜单自截图核验

## 12. 许可

VineTurtleStudio 内部项目。

