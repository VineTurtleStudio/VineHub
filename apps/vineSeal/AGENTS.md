# vineSeal（藤印）——工作约定与不变量速查

本项目是「复制/移动 + 转移后哈希校验」的极简工具，**vineFerry 的后继**。修改前先读本文件与 `README.md`。

## 安全不变量（改动不得破坏，均有对应单测）

1. **tmp → 复读比对 → 原子改名**：任何写入先落 `目标名.seal.tmp`（常量 `transfer::TMP_SUFFIX`），复读哈希比对通过才 `fs::rename` 为正式名；失败/取消删 tmp（`cancel_mid_copy_cleans_tmp`、`no_tmp_left_after_success`）。
2. **移动校验过才删源**：跨盘走 copy_file，仅 `out.verified && mode == Move` 才 `remove_file(源)`（`run_transfer_move_deletes_source_after_verify`、`move_failure_keeps_source`）；同盘走 `rename` 快路径（数据未搬动，无需哈希）。
3. **源永远不被 vineSeal 主动破坏**：任何失败路径只动 tmp 与目标侧（`invalid_dest_fails_and_source_kept`）。
4. **同路径 / 目标在源内部一律拒绝**：`paths::ensure_not_inside`（`rejects_same_path` 等）。
5. **reparse point（symlink/junction，属性 0x400）在扫描与计划中剔除**：`paths::is_reparse_point` + plan 的 `filter_entry`。
6. **长路径**：所有 fs 调用过 `paths::to_long_path`（`\\?\` / `\\?\UNC\`）。
7. **默认 sha256、默认冲突策略 rename、默认重试 2 次、默认跳过同 size+mtime**：`CopyOptions::default()`。

## 架构边界

- `src/core/` **零 GUI、零 RPC 依赖**，全部单测在 core 内（`#[cfg(test)] mod tests`，用 `tempfile`）。
- `src/ui/` 只做单窗口胶水：**禁止**引入文件浏览/缩略图/剪贴板/历史库功能（见 README §3 明确不做）。
- `src/rpc.rs` 只用 `vine-plugin-api` 已有 DTO（`TransferStartParams`/`VerifyStartParams`），新字段先改 `crates/vine-plugin-api/src/rpc.rs` 并跑其测试。
- 新增依赖必须进根 `Cargo.toml [workspace.dependencies]`，应用侧写 `{ workspace = true }`；`[profile]` 只能在根。

## 常用命令

```powershell
# cargo 不在默认 PATH 时先：
$env:PATH = "C:\Users\ChengmuSu\.cargo\bin;$env:PATH"

cargo build -p vine-seal
cargo test  -p vine-seal          # 50 单测 + 6 协议测试（tests/rpc_protocol.rs spawn 真 exe）
cargo clippy -p vine-seal --all-targets   # 必须零警告

# 便携版打包（含 release 构建；-SkipBuild 跳过构建）
powershell -ExecutionPolicy Bypass -File tools\package-portable.ps1
# 产出 dist-portable\vineseal-portable.zip；.ps1 含中文必须带 UTF-8 BOM（PS5.1 解析）
```

- **argv 最早分流**（`src/main.rs`）：`--rpc` 必须在任何控制台/GUI 初始化之前返回；GUI 仅在完全无参数时启动。
- Windows 控制台中文：`main.rs` 的 `enable_utf8_console()`（`SetConsoleOutputCP(65001)`），rpc 模式跳过——与 vinePulse 同款。
- 退出码约定：`0` 全部成功 / `1` 有失败或校验不符 / `2` 参数错误。
- `tests/rpc_protocol.rs` 用 `env!("CARGO_BIN_EXE_vine-seal")`（连字符原样）。

## 编码/协作红线

- 中文源文件一律用编辑工具修改，**禁用 PowerShell `Set-Content`/`-replace`**（UTF-8→GBK 损坏）。
- 不动 `apps/vineFerry/`（已封存，见其 `ARCHIVED.md`）；其移出/恢复只改根 `Cargo.toml` members。
- 不动 `/reports/`；协议/清单格式变更需同步 `plugins/vine.seal/plugin.json` 与根 README。
