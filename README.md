<div align="center">

# VineHub · 藤巢

**把一堆文件类工具收进一个主面板，同时保留每个工具的独立运行能力**

<a href="#-简体中文">简体中文</a> ·
<a href="#-繁體中文">繁體中文</a> ·
<a href="#-english">English</a> ·
<a href="#-日本語">日本語</a>

</div>

---

## 🇨🇳 简体中文

VineHub 是一个 **monorepo（Cargo workspace + 多个 Vite 前端）**：里面既有一个统一主面板 `vine-hub`，也有一组各自独立可运行的桌面工具。

插件化只是给每个工具**增加「被宿主编排」的能力**，不是把它变成宿主的附庸——hub 不存在时，每个工具照常双击运行。

### 包含的应用

| 应用 | 做什么 | 形态 |
|---|---|---|
| **vine-hub** | 宿主主面板：单窗口 + iframe 面板 + sidecar 插件调度 | Tauri 2 + React 19 |
| **vinePulse** | 文件损坏检测（图片 / 音视频 / 文档 / 压缩包 / 数据库，支持哈希清单比对与盲检） | Tauri 2 |
| **vinePress** | 文件整理与批量归档（识别 9 类目录，按最小压缩单位批量导出） | Tauri 2 + Rust + TS |
| **vineSync** | Windows 文件夹同步备份（镜像式同步） | Tauri 2 + React + Rust |
| **vineCurator** | 本地媒体资产管理（层级分类、标签、智能集合、全文检索） | Tauri 2 + TS |
| **vineSeal** | 复制 / 移动后哈希校验（「转移即校验」，校验不过绝不删源） | Rust 单 exe |
| **vineFerry** | 文件摆渡：类资源管理器的双栏面板 + 转移校验 | Rust + egui |
| **VinePalmaCursor** | 光标 / 指针编辑（开发中） | Tauri 2 |

### 环境要求

| 依赖 | 版本 |
|---|---|
| Node.js | ≥ 20（建议 20 / 22 LTS） |
| Rust | ≥ 1.85（workspace 使用 `edition = "2024"`） |
| 系统 | Windows 10/11（需 WebView2 + MSVC 生成工具）· macOS 13+ · Ubuntu 22+ |

> Windows 上 Rust 的 MSVC 工具链会自动探测 VS Build Tools 与 Windows SDK，无需手动跑 `vcvars64.bat`。

### 快速开始

克隆后先确认 Rust 侧能整体编译：

```bash
cargo build
```

然后进入你想跑的应用，安装前端依赖并启动开发模式：

```bash
cd apps/vine-hub
npm install
npm run tauri dev
```

其他应用同理（`apps/vinePulse`、`apps/vinePress`、`apps/vineSync`、`apps/vineCurator`、`apps/VinePalmaCursor`）。打包：

```bash
npm run tauri build
```

### 仓库结构

```
apps/            各个应用（每个是独立的 Tauri 工程）
  vine-hub/        宿主主面板
  vinePulse/  vinePress/  vineSync/  vineCurator/
  vineSeal/   vineFerry/  VinePalmaCursor/
crates/
  vine-host-core/  宿主核心（面板管理、插件生命周期）
  vine-plugin-api/ 插件接口定义（JSON-RPC over stdio）
plugins/         插件包（.vpkg，开发期可直接放目录）
docs/            架构与桥接协议文档
```

### 插件是怎么工作的

- 运行时**全 sidecar**：插件是独立进程，通过 **JSON-RPC over stdio** 通信，崩溃不会带走宿主
- 插件 UI 走 **iframe 隔离**，因此插件前端用什么框架都行
- 插件分发是一个 zip（`.vpkg`），开发期直接放目录即可
- 接口定义见 `crates/vine-plugin-api`，协议说明见 `docs/bridge-spec.md`

### 常见问题

- **`cargo build` 报链接错误**：确认已安装 VS Build Tools 的 C++ 生成工具与 Windows SDK
- **`npm run tauri dev` 打不开窗口**：先在应用目录执行 `npm install`，并确认 Node ≥ 20
- **只想用某一个工具**：直接进它的目录 `npm run tauri dev`，不需要启动 hub

## 🇹🇼 繁體中文

VineHub 是一個 **monorepo（Cargo workspace + 多個 Vite 前端）**：裡面既有一個統一面板 `vine-hub`，也有一組可各自獨立執行的桌面工具。

外掛化只是替每個工具**加上「被宿主編排」的能力**，不是讓它變成宿主的附屬品——hub 不存在時，每個工具照常雙擊執行。

### 包含的應用程式

| 應用 | 功能 | 形式 |
|---|---|---|
| **vine-hub** | 宿主主面板：單視窗 + iframe 面板 + sidecar 外掛調度 | Tauri 2 + React 19 |
| **vinePulse** | 檔案損壞偵測（圖片 / 影音 / 文件 / 壓縮檔 / 資料庫） | Tauri 2 |
| **vinePress** | 檔案整理與批次歸檔（辨識 9 類目錄） | Tauri 2 + Rust + TS |
| **vineSync** | Windows 資料夾同步備份（鏡像式） | Tauri 2 + React + Rust |
| **vineCurator** | 本機媒體資產管理（分類、標籤、智慧集合、全文檢索） | Tauri 2 + TS |
| **vineSeal** | 複製 / 移動後雜湊校驗（校驗失敗絕不刪除來源） | Rust 單 exe |
| **vineFerry** | 檔案擺渡：類檔案總管的雙欄面板 + 轉移校驗 | Rust + egui |
| **VinePalmaCursor** | 游標 / 指標編輯（開發中） | Tauri 2 |

### 環境需求

| 依賴 | 版本 |
|---|---|
| Node.js | ≥ 20（建議 20 / 22 LTS） |
| Rust | ≥ 1.85（workspace 使用 `edition = "2024"`） |
| 系統 | Windows 10/11（WebView2 + MSVC 工具）· macOS 13+ · Ubuntu 22+ |

### 快速開始

```bash
cargo build
```

```bash
cd apps/vine-hub
npm install
npm run tauri dev
```

打包：`npm run tauri build`

### 倉庫結構

```
apps/            各應用（各自獨立的 Tauri 專案）
crates/          vine-host-core（宿主核心）、vine-plugin-api（外掛介面）
plugins/         外掛包（.vpkg）
docs/            架構與橋接協定文件
```

### 外掛運作方式

- 執行時**全 sidecar**：外掛是獨立行程，以 **JSON-RPC over stdio** 通訊，崩潰不會拖垮宿主
- 外掛 UI 以 **iframe 隔離**，前端框架不限
- 外掛以 zip（`.vpkg`）分發，開發期直接放目錄即可

## 🇬🇧 English

VineHub is a **monorepo (Cargo workspace + several Vite front ends)**: a unified host shell called `vine-hub`, plus a set of desktop tools that each still run on their own.

Plugins only **add the ability to be orchestrated by the host** — they never become dependent on it. If the hub is not running, every tool still launches standalone.

### What's inside

| App | Purpose | Stack |
|---|---|---|
| **vine-hub** | host shell: single window, iframe panels, sidecar plugin scheduling | Tauri 2 + React 19 |
| **vinePulse** | file corruption detection (images / audio / video / documents / archives / databases) | Tauri 2 |
| **vinePress** | file organisation and batch archiving (9 folder archetypes) | Tauri 2 + Rust + TS |
| **vineSync** | Windows folder mirror-backup | Tauri 2 + React + Rust |
| **vineCurator** | local media asset manager (hierarchy, tags, smart collections, full-text search) | Tauri 2 + TS |
| **vineSeal** | hash verification after copy / move — never deletes the source on mismatch | single Rust exe |
| **vineFerry** | file ferry: explorer-like two-pane panel with transfer verification | Rust + egui |
| **VinePalmaCursor** | cursor / pointer editor (in progress) | Tauri 2 |

### Requirements

| Dependency | Version |
|---|---|
| Node.js | ≥ 20 (20 / 22 LTS recommended) |
| Rust | ≥ 1.85 (workspace uses `edition = "2024"`) |
| OS | Windows 10/11 (WebView2 + MSVC build tools) · macOS 13+ · Ubuntu 22+ |

### Quick start

```bash
cargo build
```

```bash
cd apps/vine-hub
npm install
npm run tauri dev
```

Build a release bundle with `npm run tauri build`.

### Layout

```
apps/            one Tauri project per app
crates/          vine-host-core (host logic), vine-plugin-api (plugin contract)
plugins/         plugin packages (.vpkg)
docs/            architecture and bridge protocol
```

### How plugins work

- **Everything is a sidecar**: plugins are separate processes talking **JSON-RPC over stdio**, so a crash cannot take down the host
- Plugin UIs run in **isolated iframes**, so any front-end framework works
- Plugins ship as a zip (`.vpkg`); during development you can just drop a folder in

### Troubleshooting

- **Linker errors on `cargo build`** — install the C++ build tools and Windows SDK from VS Build Tools
- **`npm run tauri dev` shows no window** — run `npm install` inside the app folder and check Node ≥ 20
- **Only need one tool** — run it from its own folder; the hub is not required

## 🇯🇵 日本語

VineHub は **monorepo（Cargo workspace + 複数の Vite フロントエンド）** です。統合パネル `vine-hub` と、単体でも動くデスクトップツール群をまとめています。

プラグイン化は各ツールに**ホストから編成される能力を追加するだけ**で、依存関係を作るものではありません。hub がなくても各ツールはそのまま起動できます。

### 収録アプリ

| アプリ | 役割 | 構成 |
|---|---|---|
| **vine-hub** | ホストパネル（単一ウィンドウ + iframe パネル + sidecar プラグイン） | Tauri 2 + React 19 |
| **vinePulse** | ファイル破損検査（画像 / 音声動画 / 文書 / 圧縮 / DB） | Tauri 2 |
| **vinePress** | ファイル整理と一括アーカイブ（9 種類のフォルダを判定） | Tauri 2 + Rust + TS |
| **vineSync** | Windows フォルダのミラー同期バックアップ | Tauri 2 + React + Rust |
| **vineCurator** | ローカルメディア管理（階層・タグ・スマートコレクション・全文検索） | Tauri 2 + TS |
| **vineSeal** | コピー / 移動後のハッシュ検証（不一致なら元を消さない） | Rust 単体 exe |
| **vineFerry** | ファイル運搬：エクスプローラー風 2 ペイン + 転送検証 | Rust + egui |
| **VinePalmaCursor** | カーソル / ポインター編集（開発中） | Tauri 2 |

### 必要なもの

| 依存 | バージョン |
|---|---|
| Node.js | ≥ 20（20 / 22 LTS 推奨） |
| Rust | ≥ 1.85（`edition = "2024"`） |
| OS | Windows 10/11（WebView2 + MSVC ビルドツール）· macOS 13+ · Ubuntu 22+ |

### クイックスタート

```bash
cargo build
```

```bash
cd apps/vine-hub
npm install
npm run tauri dev
```

リリースビルドは `npm run tauri build`。

### 構成

```
apps/            各アプリ（Tauri プロジェクト）
crates/          vine-host-core（ホスト本体）、vine-plugin-api（プラグイン仕様）
plugins/         プラグインパッケージ（.vpkg）
docs/            アーキテクチャとブリッジ仕様
```

### プラグインの仕組み

- **すべて sidecar**：プラグインは別プロセスで **JSON-RPC over stdio** により通信。クラッシュしてもホストは落ちません
- プラグイン UI は **iframe 分離**。フロントエンドのフレームワークは自由
- 配布は zip（`.vpkg`）。開発中はフォルダを置くだけ

### よくある問題

- **`cargo build` でリンクエラー** — VS Build Tools の C++ と Windows SDK をインストール
- **`npm run tauri dev` で窓が出ない** — アプリのフォルダで `npm install`、Node ≥ 20 を確認
- **1 つのツールだけ使いたい** — そのフォルダで直接起動すれば hub は不要
---

## 许可 / License

本项目以 [MIT](./LICENSE) 许可证发布。

Licensed under the [MIT License](./LICENSE).
