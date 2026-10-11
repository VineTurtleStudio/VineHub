# vinePress — 开发约定

面向个人媒体库的文件归档工具：识别专辑/小说/图册/漫画/乐谱/演唱会/视频/资料（9 类）→ 可编辑项目 → 批量写说明文件 → 批量 RAR/ZIP 导出。
架构与 `D:\VineTurtleStudio\Projects\VinePalmaCursor` 一致：**Tauri 2.x + Rust + TypeScript + Vite（零框架前端）**。

## 常用命令（Windows / PowerShell）

```powershell
npm.cmd run dev          # 仅前端，端口 5174（勿与 VinePalmaCursor 的 5173 冲突）
npm.cmd run tauri dev    # 联调（前端热更新 + cargo watch）
npm.cmd run build        # tsc && vite build
cargo test               # 单测 + notes_test + detect/fsops/project/zip + e2e zip（不需本机数据）
cargo test -- --ignored  # 额外跑依赖本机数据/WinRAR 的测试
cargo test --test ground_truth -- --ignored --nocapture   # 514 基准比对（需 D:\18138；2026-10-08 数据漂移重定基线）
cargo test --test kind_audit -- --ignored --nocapture     # kind 两层判定分布核对 + 低置信清单 + EnrichStats（需 D:\18138）
cargo test --test e2e_test -- --ignored                   # 扫描→说明→导出 全链路（需 WinRAR）
```

- `npm` 必须写 `npm.cmd`（npm.ps1 被执行策略禁用）。
- PowerShell 写文件必须显式 `-Encoding UTF8`，否则 UTF-8 会被 GBK 损坏。
- WinRAR：`C:\Program Files\WinRAR\Rar.exe`（无 7-Zip）；其控制台输出为 GBK，用 `encoding_rs::GB18030` 解码。

## 分层约定

- `src-tauri/src/scan/`、`detect.rs`、`notes.rs`、`archive.rs`、`export.rs`、`project.rs`、`meta.rs`（自动填充、封面提取 APIC/FLAC PICTURE/命名图、`embedded_cover` 内嵌封面）、`fsops.rs`（文件操作）、`dupes.rs`（查重，sha2 两级分组）、`naming.rs`（命名检查与修复：网盘档案 `Profile`、`check_name` 规则、`sanitize` 建议名、`check_tree` 全树扫描、`dedupe` 同级去重）、`kindx.rs`（封面 dHash 交叉比对 → `cover_match` 回填与重判）、`manifest.rs`（导出清单 `vinepress-manifest.json` + 完整性校验）：**纯逻辑，零 Tauri 依赖**，可被 `tests/` 直接调用。
- `src-tauri/src/commands.rs`：薄桥接（`#[tauri::command]` + 事件 emit + `JobState` 单任务互斥）。
- `src/api.ts`：前端唯一 invoke 出口；**结构体字段**与 Rust `Serialize/Deserialize` 一一对应（snake_case），但 **命令参数**按 Tauri 默认 camelCase 传（Rust `show_all` ↔ JS `showAll`）。
- `src/state.ts` 状态与过滤（高级筛选 `year/tag/profile/sizeMin/sizeMax` + `searchHaystack`/`filtersActive`/`clearEntryFilters`）、`src/ui.ts` 渲染（`syncFilterOptions` 由条目集派生筛选选项；`toast` 为堆栈容器、`setProgress` 撤不确定进度态）、`src/main.ts` 事件接线（高级筛选条、`Ctrl+Shift+F` 全局搜索浮层、脏状态 `markDirty`/`isDirty`、表格多选与键盘导航）；`src/layout.ts`（分隔条拖动、面板显隐与尺寸持久化、Excel 式列宽拖拽 `initColResize`）、`src/menu.ts`（IDE 式主菜单栏 + 快捷键分发）、`src/settings.ts`（设置对话框与偏好 prefs）。
- 测试：默认 `cargo test` 含 `meta_test`（目录名/说明文件/CUE/FLAC/ID3 提取）、`fsops_test`（根目录内路径校验、复制/移动、`rename_path` 说明文件同步）、`detect_test`（`has_notes` 精确匹配、根散件 `root_loose`）、`naming.rs` 内 9 单测（非法字符/保留名/首尾空格点/全角映射/超长截断/同级去重）、`manifest_test`（清单 save/upsert/unchanged、校验四态 + 未入清单、取消短路）、`kind_scoring`/`naming_patterns`（评分与命名模式）、`kindx_test`（封面比对与端到端翻案）与 zip e2e（导出后清单 + 校验断言 + `incremental_zip_export`/`unit_fingerprint_stability` 增量语义）。

## 关键不变量

- 单位识别规则见 `scan/rules.rs`（ground truth 测试锁死 **514/514**——2026-10-08 数据漂移后重定基准：`D:\18138` 当日新增 10 个单位目录，fixture 追加 10 行、原 504 行未动，仍锁 `missing=0 / extra=0`；改动必须重跑基准）。类型判定两层 = `rules::kind_verdict(rel, stats) -> KindVerdict{kind, confidence, review}`（**9 类**：album/novel/artbook/comic/score/concert/video/document/other），`infer_kind` 为其薄包装（只取 kind）：**Layer1 路径门**——`word_hit` **最近段**类别意图（idx=命中段距单位名深度；段名恰为类别词 = 分类文件夹直接生效；类别词仅嵌在标题 → 内容裁决，**含音频一律 album**；非精确 novel 命中且 idx>0 时图像/视频改判 artbook/video，防 `Key小说图册` 根词抢 CG/卡片；音乐/公演语境（音乐/专辑/公演/ライブ…）+ **纯视频** → concert，音频+视频共存仍 album（bonus MV 发行物）；`guide` 弱英文词有图时让位向上；同段多词 小说>漫画>图类>音乐>公演>乐谱>视频>资料；`CG`/`comic`/`score`/`guide` 按 ASCII 字母断词，`OCG`/`MCG` 不命中）→ 命中即 `confidence=1.0, review=false`；**Layer2 评分**（仅无类别词路径，供堆叠散件判定）——`kind_scores(stats)` 按证据加权取最高（稳定序 album→concert→score→comic→artbook→novel→document→video→other，权重含字节占比 `share`、cue/log/分轨/页码/剧集/章节卷数/体裁词/音乐公演词/封面匹配度等），**confidence = top1−top2 分差**，`review = confidence < REVIEW_MARGIN(0.7) || kind=="other"`。证据字段全在 `UnitStats`（`#[serde(skip)]`）：`audio_b/image_b/video_b/doc_b/text_b/archive_b` 字节 + `cue_count/log_count/sub_count/ebook_count/comic_arch/prose_count/tracks_numbered/page_numbered/episode_count/chapter_dirs/part_dirs` + `cover_match`，由 `scan/mod.rs::walk` 累加（`UnitStats.hint` ≤4KB 摘要仍供文件名级判定）。**封面交叉比对** `kindx.rs`：`audio>0 && image>0` 且 confidence<1.0 才读盘——根目录 flac/mp3 内嵌封面（`meta::embedded_cover`）vs 单位内图片 dHash（HAMMING_MAX=10、≤16 图、单图 ≤30MB）→ 回填 `cover_match` 重判（评分 album +3.0*r / artbook +1.5*(1−r)）；confidence≥1.0 直接 gated 跳过（`EnrichStats{candidates,gated,computed}`，`scan_directory` 内 detect 之后调用）。**复核标记** `DetectNode/UnitInfo/Entry.needs_review`（`#[serde(default)]`）→ 前端表格/树/详情 `.badge.review`「复核」徽标 + `searchHaystack` 含「复核」；判定为**音视频混合**（`rules::is_mixed_av`：音频与视频都有实体且较少数 ≥ 较多者 20%，字节可用按字节否则按计数）时强制 `review=true`——路径门命中（confidence=1.0）同样生效，kind/confidence 不变；手动改类型（详情 select 或批量 `be-kind`）必须清标记，树徽标与树 kind 同属扫描时状态、不随编辑刷新；全局搜索跳转清筛选、树点击清全筛选的通用规则同样覆盖复核条目。**已知边界**：`detect` 只分类子目录，扫描根目录自身散文件不入单位（rel_path 模型锁死，勿改）——`DetectResult.root_loose` 收集这些散件 → `ScanResponse.root_loose` → 前端扫描完成 toast「根目录有 N 个散件未纳入单位」并写进度日志。前端类型全集唯一来源 = `state.ts` 的 `KINDS`（类型下拉/批量编辑/徽标/筛选同源，徽标配色在 `styles.css` `.badge.kind-*`）；说明模板分支 `notes.rs`（`albumish = audio>0 || kind==album`，其余走资料信息模板 + `publication_type` 按 kind 兜底）。内容关键词扩展名同时进 `CONTENT_EXTS` 与 `scan/mod.rs::walk`，改动必须重跑 504 基准（边界不变）。
- 手动覆盖：`detect::detect(root, ov)` 判定顺序 `force_units` → 「覆盖路径的祖先」→ `force_containers` → `classify`；**空 `overrides` 必须与历史行为完全一致**（同一基准锁死）。`Project.overrides` 带 `#[serde(default)]`，旧项目免迁移。**拆分散件警示**：`forceContainer` 在单位**直属有内容文件**时先弹确认（数据 = `UnitStats.direct_files`，`#[serde(default)]`，`collect_unit_stats` 填充；旧项目缺省为空则直接执行）——散件拆分后不属于任何子单位、导出不含；拆分入口 = 详情面板「含 N 个子目录」区块按钮 + 表格名称列「N 子目录」角标（`subdirs.length>1`）+ 目录树「强制为容器」。
- `Entry.name` / `rel_path` 只能经磁盘重命名同步修改（`fsops::rename_path` + `project::rename_entry_paths`），保证 **压缩包名 ≡ 目录名 ≡ 说明文件名**；`rename_path` 成功后自动把目录内 `<旧名>.txt` 同步改名为 `<新名>.txt`（`eq_ignore_ascii_case`；目标已存在或仅大小写等价则不动，单测锁死）；`name` 在前端只读。
- 导出路径：`<输出根>\<相对路径>\<单位名>\<单位名> [密码].rar`（压缩包在镜像单位目录**内部**）；ZIP 为 `…\<单位名>.zip`，**无密码标签**。
- 分卷 `-v900m`（943718400）→ `name.part1.rar`；头加密 `-hp`；注释 = 说明文件正文且必须 **GBK** 编码写入（RAR 与 ZIP 一致）。
- ZIP：`archive::create_zip`（deflate、跳过重连点与 `Thumbs.db`、已存在且不覆盖 → `skipped`、半成品失败/取消即删）；**不加密、不分卷**（`export.rs::run_export` 分支，`start_export` 仅 `format != "zip"` 才要求 Rar.exe）。
- 导出清单：`run_export` 逐单位循环结束后、finished 事件之前 upsert `<输出根>\vinepress-manifest.json`（`manifest.rs`，按 `archive` 键覆盖、size+mtime 未变跳过重哈希、tmp+rename 原子写；进度 `phase = "manifest"`）；**中途取消不写清单**。校验 `start_verify` 只读逐文件核对 存在性 → 大小 → SHA-256，并扫描输出根列出未入清单的 rar/zip（清单内 txt/md 不算）。
- 增量导出（`ArchiveSettings.incremental`，默认关）：跳过判定 = 清单 `fingerprint` 非空且一致 **且** 磁盘包 size+mtime 未变；指纹 = 源目录 walk（文件数/总字节/max mtime）+ 设置 + 密码 + 说明正文哈希，**仅创建成功时按写盘后状态写入**，非增量重建写空串作废、「不覆盖跳过」保留旧指纹；受管说明文件 `<单位名>.txt` 不计入 walk（由正文哈希覆盖）。说明必须**先渲染 → 增量判定 → 才落盘**（`notes::write_note_file`），跳过路径不碰源目录；`unit_fingerprint` 语义由 `unit_fingerprint_stability` + `incremental_zip_export` 锁死。资料信息模板（`render_publication`）自本轮起含【备注】字段。
- 说明文件：UTF-8 BOM + CRLF，页脚 `本信息文件由本地…自动整理生成`；`Entry.has_notes` = 单位目录内存在**精确等于** `<单位名>.txt` 的文件（`eq_ignore_ascii_case`，`readme.txt` 等任意 txt 不算）。
- 密码方案：多密码时按 `rel_path` 的 FNV-1a 稳定取样（`project.rs::resolve_password`）。
- 删除走系统回收站（`trash_items` → `trash::delete_all`，逐项先做根内校验），**绝不永久删除**。
- 写操作必须落在项目根目录内（`fsops::ensure_under` / `validate_rel`），且拒绝目标位于源内部。
- 前端快捷键唯一来源 = `index.html` 的 `data-keys` 属性（`menu.ts` 分发命令 + 快捷键弹窗同源；命令面板 `Ctrl+K` 同源扫描菜单项）；编辑元素内非 Ctrl/Alt 组合跳过，`Ctrl+A/C/X/V/Z/Y` 在输入框内让给浏览器。
- 全局搜索（`Ctrl+Shift+F` / 编辑菜单 `globalSearch`）Enter 跳转条目时，若目标被当前筛选（含 `#entry-filter`）隐藏，**必须先清空全部筛选并 toast 提示再跳转**；搜索命中集 = `searchHaystack(entry)`，子串高亮。
- 脏状态（无自动保存）：`snapshotOf(project)` 剔除瞬态 `selected` 后与保存/加载快照比较；所有项目数据变更点必须 `markDirty()`（说明生成/核对完成回写 `has_notes` 也算），退出/新建/打开/关窗走 `isDirty()` 强制复核兜底；指示 = 标题 `● ` 前缀 + 保存按钮 `.dirty`，保存/新建/加载 `clearDirty()` 归零；`forceClose` 标志跳过关窗重复询问。
- 表格交互：多选锚点 `lastRowRel`（存 rel_path——排序/筛选后索引会漂移；行点击与键盘导航共享）；Ctrl/⌘ 切换、Shift 范围选、普通点击只移当前行——与文件面板手势一致；↑↓/Home/End/Space/Enter 键盘导航在 `document` keydown，编辑控件、按钮聚焦与弹窗/命令面板/搜索浮层打开时让位；**树点击被筛隐藏的单位必须走 `clearAllFilters()`**（全量清空含高级筛选，禁止只清 3 个基础筛选）。
- 列宽与布局：`#entry-table` 用 `table-layout: fixed` + `th[data-sort]` 宽度，`.col-resizer` 拖动（min 48px）固化全列 px、总宽超容器出现横向滚动，双击复位该列且不触发排序；**末列 `.col-fill` 填充列（width:auto + padding:0）必须存在**——余量靠它吸收，否则浏览器把剩余宽度按比例摊给所有列（「拖一列动全部列」回归即此）；宽度存 `vinepress.layout.colWidths`（键 = `data-sort`），`fitTableWidth` 汇总**除 `.col-fill` 外**的 `thead th` 后比较容器，`applyLayout` 末尾重算（面板尺寸/显隐改变容器宽），重置布局一并清空。
- 反馈通道：`toast(msg)` 为堆栈（最多 3 条 `.toast-item`，含「失败/错误」自动标红 ≥4s），断言 toast 用 `#toast` textContent 包含匹配；`setBusy(kind)` 加 `#progress-track.indeterminate` 动画、`setProgress` 移除，扫描用 `setProgressText` 只改文案保持动画。
- 忙碌守卫与隐藏态：菜单项 `data-busy="block"` + `setMenuBusy`，按钮 `data-busy-guard`（`busyGuard()` 包装扫描/覆盖编辑/改名/删除/复制移动/生成说明/命名修复入口，检查类只读命令不包）；全局 `[hidden] { display: none !important }`——`.btn`/`.btn-sm` 的 `display: inline-flex` 会压过 UA `[hidden]` 样式（曾使菜单最近项目与命名面板隐藏失效）。
- 界面偏好存 localStorage：`vinepress.prefs`（theme/zoom/reduceMotion/archiveDefaults）、`vinepress.layout`（面板尺寸与显隐 + `colWidths` 列宽）、`vinepress.recent`（最近项目 ≤8 条、最新在前、加载失败移除）；主题由 `:root[data-theme='light']` 覆盖深色基线，`system` 用 `matchMedia` 解析。缩放 `--zoom` 作用于 `#app` transform，**所有 vw/vh 必须除以 `var(--zoom)`**。

## 当前 command 清单（`commands.rs`，全部经 `src/api.ts` 暴露）

- 项目：`save_project`、`load_project`（「新建/打开项目」在前端拼默认名 `vinePress-project.json`）
- 扫描：`scan_directory(root, overrides)`、`auto_fill_proposals`
- 查重：`start_find_duplicates`（只读两级分组，事件 `dupes-progress`）
- 命名：`name_profiles`（档案列表）、`check_names(root, profile)`（只读全树检查 → `NameCheckResult{profile, issues, checked_dirs, checked_files}`；**必须返回对象**非数组）、`fix_names(root, items)`（按 rel 段数升序批量磁盘改名 → `FixNamesOutcome{renamed, failed}`；父目录改名位移后续项 rel，前端 `applyRelShift` 同步条目/选中态，返回 `RenamePair[]`）
- 导出/说明：`start_export`、`start_generate_notes`、`start_note_audit`（只读批量核对，事件 `note-audit-progress`）、`start_verify`（只读校验导出清单，事件 `verify-progress`）、`cancel_job`、`preview_note`、`note_diff`（字段级对比：现有 vs 重新渲染）、`cover_image`（详情面板封面缩略图 → 320px JPEG data URL，无封面 None）、`preflight_export`
- 文件操作（全部 async + 根内校验）：`list_dir`、`make_directory`、`copy_items`、`move_items`、`open_path`、`reveal_path`、`rename_path`、`trash_items`
- 环境：`probe_tools`
- 应用：`quit_app`（菜单「退出」）
- 事件：`notes-progress`、`note-audit-progress`、`dupes-progress`、`verify-progress`、`export-progress`（均由 `commands.rs` worker emit，`src/api.ts` 的 `on*Progress` 订阅）
