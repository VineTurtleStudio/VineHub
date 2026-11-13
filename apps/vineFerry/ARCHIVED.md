# vineFerry 已封存

> 封存日期：2026-10-16

## 状态

**vineFerry（藤渡）已封存，暂停维护。** 代码原样保留，但已从根 `Cargo.toml` 的 workspace members 中移除，`cargo build` / `cargo test` 不再编译本项目。

## 封存原因

vineFerry 最初的定位是「复制/移动 + 转移后哈希校验」的轻量工具，核心引擎（`src/core/`）始终健壮（86 项单测）。但项目中途演进为「资源管理器式文件管理器」：双栏多标签浏览、缩略图、系统图标、剪贴板互操作、SQLite 历史、预设调度、续传日志、i18n……UI 层膨胀至约 4900 行，整体约 8800 行。继续追求「完美文件管理器」的成本已超出收益。

## 后继项目

**vineSeal（藤印）**，位于 `apps/vineSeal`：只保留 vineFerry 的核心价值——转移即校验、校验不过绝不删源——以单窗口极简 GUI + CLI + VineHub 插件三形态实现，明确不做文件管理功能。

## 如何恢复本项目

如未来需要 vineFerry 的文件管理能力：

1. 在根 `Cargo.toml` 的 `[workspace] members` 中重新加入 `"apps/vineFerry"`；
2. `cargo build -p vine-ferry` 确认可编译；
3. 将本文件移除或更新状态。

`plugins/vine.ferry/plugin.json` 未改动；恢复编译后 hub 可重新发现该插件（封存期间因 exe 不再构建，hub 中会显示为不可用，属预期状态）。
