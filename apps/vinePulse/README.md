# vinePulse

给文件「把脉」——一款能检测各类文件是否损坏的桌面端工具。支持图片 / 音视频 / 文档 / 压缩包 / 数据库与数据文件，以及任意类型的通用完整性检测；同时支持「哈希清单比对」与「无参考值盲检」两种模式。

## 特性

- **单文件交付**：Rust + egui 编译为单个 exe，无运行时依赖，内存占用低
- **双模式检测**
  - 哈希模式：导入 `.sha256` / `.md5` / `.sfv` 清单，或自动寻找同名 sidecar 文件比对
  - 盲检模式：魔数识别真实类型 → 格式结构解析 → 可选深度解码
- **四级结论**：`Ok`（正常）/ `Warning`（可疑）/ `Corrupt`（确认损坏）/ `Unknown`（无法判定）
- **精确定位**：问题附带字节偏移（offset），可指出损坏发生的位置
- **外部工具增强**：启动时探测 `ffmpeg/ffprobe/7z`，存在则升级为深度解码测试；不存在则降级为纯 Rust 结构校验
- **中文界面**：拖拽扫描、结果过滤排序、问题详情、报告导出

## 快速开始

```powershell
cargo build --release          # 构建（产物：target\release\vine-pulse.exe）
cargo test                     # 运行损坏样本回归测试
cargo clippy --all-targets     # 代码检查（应零警告）
cargo run -- <路径...>         # 扫描文件或目录
```

构建环境要求（M1 时已在本机配好）：

- Rust stable + MSVC toolchain；`.cargo/config.toml` 内置了本机 `link.exe` 与
  Windows SDK 的库搜索路径，因此**普通 PowerShell 即可直接 cargo build**
- 若升级 VS Build Tools 导致 MSVC 版本号变化，需同步更新 `.cargo/config.toml`
  中的版本目录；或改用「x64 Native Tools 命令提示符」构建

> `main.rs` 按 argv 最早分流：`--rpc` → sidecar 模式；无参数 → eframe 图形界面；带参数 → CLI。三种模式共用同一套核心引擎（`src/lib.rs`）。

## CLI 用法

```text
vine-pulse [选项] <文件或目录>...

选项:
  -m, --manifest <文件>        哈希清单模式（.sha256 / .md5 / .sfv），以清单中的
                               文件为扫描目标并比对哈希；盲检模式下则会自动寻找
                               同名 sidecar（如 a.zip.sha256）比对
  -j, --json <输出文件>        导出 JSON 报告
  -d, --deep                   强制深度解码（超过大小阈值的文件也解码）
      --max-deep-size <MB>     深度解码大小阈值（默认 64；小于等于该值总是深度解码）
  -q, --quiet                  仅输出有问题的文件
      --entropy-threshold <f>  低熵判定阈值（0~8，默认 0.5）
      --no-color               关闭彩色输出
  -h, --help                   显示帮助

退出码:
  0 = 全部正常   1 = 存在 Warning/Unknown   2 = 存在 Corrupt   3 = 参数/清单错误
```

示例：

```powershell
.\target\release\vine-pulse.exe .\samples -j report.json    # 盲检目录并导出报告
.\target\release\vine-pulse.exe -m checksums.sha256         # 哈希清单比对
```

## 项目状态

- [x] **M1 核心引擎**：core 分发器 + 通用/PNG/JPEG/ZIP 验证器 + 哈希模式 + CLI + 损坏样本回归测试
- [x] **M2 验证器补全**：全部格式验证器（音视频 / 文档 / 数据 / 压缩包）
- [x] **M3 桌面界面**：egui 完整界面（无参数启动；带参数仍走 CLI，`--rpc` 最早分流）
- [x] **M4 打磨交付**：外部工具探测、并行扫描优化、exe 图标/版本信息 + 绿色版打包（`pack-portable.ps1` → `VinePulse-<版本>-windows-x64.zip`）
- [x] **GUI 体验改造**（2026-10-15）：FlacValidator 模式流式显示问题文件 + 进度统计面板（失败/速度/ETA）+ 双击定位/右键菜单（定位/打开/复制路径）+ 默认「仅有问题」过滤 + 窗口紧凑化
- [x] **第二轮改进**（2026-10-15）：修复 ID3v2 前缀的 FLAC 误判为 MP3（`detect_magic` 跳过 ID3 标签识别真实签名，`FileHint::probe` 按需扩窗）+ 结果表「首因」徽章列（悬停看完整问题列表）+ 大批量扫描卡顿修复（`Report.size` 由目录遍历携带、去掉每文件 `metadata()` stat 与每帧 `to_vec()` 深拷贝）+ 状态栏速度 EWMA 平滑
- [x] **第三轮：完美检测**（2026-10-15）：FLAC 金标准校验——完整解码整段音频并按 RFC 9639 §8.2 计算 PCM MD5，与 STREAMINFO 存储值比对（等价 `flac -t` / FlacValidator）；阈值与 CLI `--max-deep-size` 可调；熵采样从头/中/尾 3 点改为全文件均匀 32 点

## 技术栈

| 项 | 选型 |
|---|---|
| 语言 | Rust（MSVC toolchain） |
| GUI | egui / eframe |
| 音视频解码 | symphonia（纯 Rust）+ 可选外部 ffmpeg |
| SQLite 检测 | rusqlite（bundled）`PRAGMA quick_check` |
| 哈希 | sha2、md-5、crc32fast |
| 格式解析 | zip、png、image 等 |
| 报告导出 | serde / serde_json |

## 架构

```
vinePulse/
├── Cargo.toml                # package name: vine-pulse
├── src/
│   ├── main.rs               # 入口：--rpc / GUI / CLI 按 argv 最早分流
│   ├── lib.rs                # 模块导出，供 CLI 与测试共用
│   ├── core/
│   │   ├── mod.rs            # Verifier trait / Severity / Report 定义
│   │   ├── dispatch.rs       # 魔数 + 扩展名 → 选择验证器
│   │   ├── hash.rs           # 哈希清单模式
│   │   ├── entropy.rs        # 通用熵分析（盲检辅助）
│   │   └── walker.rs         # 目录遍历、并行扫描、进度/取消
│   ├── verifiers/
│   │   ├── generic.rs        # 空文件、魔数不符、截断、全零填充
│   │   ├── image.rs          # PNG 块 CRC、JPEG marker + 解码
│   │   ├── archive.rs        # ZIP 逐条 CRC、EOCD
│   │   ├── media.rs          # [M2] MP4 box 树、AVI/WAV RIFF、MP3 帧链、FLAC CRC
│   │   ├── doc.rs            # [M2] PDF、OOXML/ODF、文本编码
│   │   └── data.rs           # [M2] SQLite quick_check、CSV、Parquet
│   └── ui/
│       ├── mod.rs            # [M3] 状态机、页面切换
│       ├── scan_view.rs      # [M3] 拖拽、结果表格（过滤/排序）、进度
│       └── detail_view.rs    # [M3] 问题详情、报告导出(JSON/HTML/CSV)
└── tests/
    └── integration.rs        # 运行时生成损坏样本的回归测试
```

## 核心设计

```rust
enum Severity { Ok, Warning, Corrupt, Unknown }

struct Finding {
    severity: Severity,
    code: &'static str,     // 问题编码，如 "PNG_BAD_CRC"
    message: String,        // 中文描述
    offset: Option<u64>,    // 损坏字节位置
}

struct Report {
    path: PathBuf,
    file_type: String,      // 魔数识别的真实类型
    mode: Mode,             // Hash / Blind
    findings: Vec<Finding>,
    duration: Duration,
}

trait Verifier {
    fn supports(&self, hint: &FileHint) -> bool;
    fn verify(&self, path: &Path, opts: &Opts) -> Report;
}
```

- **分发**：先读魔数判断真实类型，扩展名仅作提示（可发现伪装/改名的文件）
- **深度解码可开关**：按文件大小阈值或用户设置跳过，避免大文件卡顿

## 各格式检测点

| 类别 | 格式 | 检测方式 |
|---|---|---|
| 图片 | PNG | 逐块 CRC、IEND 存在性、尺寸一致性 |
| 图片 | JPEG | marker 结构遍历 + 全图解码 |
| 图片 | GIF/BMP/WebP | 结构解析 + 解码 |
| 音视频 | MP4/MOV | box 树遍历、长度越界、stco/stsc 索引越界；有 ffprobe 时深度解码 |
| 音视频 | WAV/AVI | RIFF 块遍历、fmt/data 一致性 |
| 音视频 | MP3/FLAC/OGG | 帧链遍历、FLAC CRC16 帧校验、OGG 页 CRC；FLAC 另做全解码 + STREAMINFO MD5 金标准比对；symphonia 深度解码 |
| 文档 | PDF | header、xref/trailer、`%%EOF`、对象结构 |
| 文档 | DOCX/XLSX/PPTX/ODT | ZIP 中央目录 + 内部结构（mimetype、内容类型） |
| 文档 | TXT/CSV 文本 | 编码有效性、非法字节序列 |
| 压缩包 | ZIP | 逐条解压测试 CRC、中央目录完整性 |
| 压缩包 | gzip | 尾部 CRC32/ISIZE 校验 |
| 压缩包 | tar | 结构遍历、块对齐、结束标记 |
| 压缩包 | 7z/RAR | 有 `7z t` 时深度测试，否则头部结构检查 |
| 数据 | SQLite | `PRAGMA quick_check`（rusqlite bundled） |
| 数据 | CSV | 全量解析、列数一致性 |
| 数据 | Parquet | `PAR1` 魔数 + footer 完整性 |
| 通用 | 任意文件 | 空文件、魔数与扩展名不符、疑似截断、熵异常/全零填充、哈希清单比对 |

## 桌面端功能

1. 拖拽文件/文件夹（或文件选择器），批量扫描 + 进度条 + 可取消；扫描中流式显示问题文件（FlacValidator 模式），实时统计失败数 / 速度 / ETA
2. 结果表格：文件名、类型、结论、问题数；**默认只显示问题文件**（可切「全部」查看正常文件）、按列排序、搜索过滤
3. 行交互：**双击在资源管理器中定位**；右键菜单「在资源管理器中定位 / 打开文件 / 复制路径」
4. 详情面板：每个问题的描述与字节偏移
5. 工具栏：导入哈希清单 / 导出报告 / 设置（外部工具路径、深度解码开关、熵阈值）
6. 中文界面；窗口紧凑布局（960×600 默认，可缩放 760×480 ~ 1280×860）

## 里程碑

1. [x] **M1 核心引擎**：`core` 分发器 + 通用/PNG/JPEG/ZIP 验证器 + 哈希模式 + CLI + 损坏样本回归测试
2. [x] **M2 验证器补全**：其余格式验证器（音视频 / 文档 / 数据 / 其他压缩包）
3. [x] **M3 桌面界面**：egui 完整界面（拖拽、表格、详情、导出）
4. [x] **M4 打磨交付**：外部工具探测、并行扫描优化、exe 图标/版本信息 + 绿色版打包
5. [x] **GUI 体验改造**：流式问题文件 + 进度统计 + 双击定位/右键菜单 + 窗口优化（2026-10-15）
6. [x] **第二轮改进**：ID3v2/FLAC 误判修复 + 首因徽章列 + 大批量扫描性能修复 + 速度平滑（2026-10-15）
7. [x] **第三轮：完美检测**：FLAC 全解码 + STREAMINFO MD5 金标准比对 + 阈值可调 + 熵采样 32 点（2026-10-15）

## 测试策略

`tests/integration.rs` 在运行时生成样本并施加破坏手段，断言各验证器输出正确的 `Severity`：

- 截断文件（砍掉尾部 N 字节）
- 翻转 IDAT / 归档数据区字节（触发 CRC 不符）
- 破坏魔数/文件头
- 空文件、全零填充、扩展名与内容不符
- 哈希清单（sha256 / sfv / sidecar）篡改前后对比

## 风险与备注

- 本机 MSVC 构建环境：VS Build Tools 2022（含 C++ 工具集）+ Windows SDK 10.0.26100，
  链接路径固化在 `.cargo/config.toml`，升级工具链后需同步更新
- RAR 深度测试依赖外部 7z，否则降级为结构检查
- 大文件解码耗时 → 大小阈值 + 可关闭深度解码
- Cargo package 名为 `vine-pulse`（crate 名全小写 + 连字符规范）
