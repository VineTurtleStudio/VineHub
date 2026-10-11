# VinePalmaCursor — 开发约定

跨平台鼠标指针制作软件。技术栈：Tauri 2.x + Rust + TypeScript + Canvas 2D。

## 常用命令

- 启动开发（Tauri 窗口 + Vite dev server）：`npm run tauri dev`
- 前端构建：`npm run build`（tsc + vite build）
- Rust 测试（codec 层）：`cd src-tauri && cargo test`
- 前端类型检查：`npx tsc --noEmit`
- 打包单测：`cargo test --test codec_test`

## 架构原则

1. **编解码为纯逻辑库层**：`src-tauri/src/codec/` 零 Tauri 依赖，必须可单测。
   - `bitmap.rs` — RGBA 图像 + 热点结构体（`BitmapImage`、`CursorFrame`{image, delay_ms}）
   - `cur.rs` — .cur 读写（ICO 容器，CUR type=2）
   - `dib.rs` — RGBA ↔ DIB（XOR mask + AND mask）
   - `scale.rs` — 多尺寸下采样（最近邻/双线性）+ 标准尺寸生成
   - `ani.rs` — .ani 读写（RIFF/ACON），jiffies ⇄ ms 换算
   - `image.rs` — PNG/GIF/BMP/JPEG 解码，GIF 拆帧（依赖 image crate，带 4096 上限）
2. **前端只持有 RGBA 像素 + 元数据**，所有文件写入走后端 codec（`src/api.ts` 封装 Tauri invoke）。
3. `commands.rs` 只做薄桥接，不含格式逻辑。

## Tauri 命令

- `export_cur` — 传 `{images: [{width, height, hotspot_x, hotspot_y, rgba}]}`，返回 .cur 字节数组
- `export_ani` — 传 `{frames: [{width, height, hotspot_x, hotspot_y, rgba, delay_ms}]}`，所有帧须同尺寸，返回 .ani 字节数组
- `import_file` — 传 `{data: number[]}`，自动检测 RIFF/ACON(ani)、CUR 头、图片 magic，返回 `{format, frames: [{width, height, hotspot_x, hotspot_y, rgba, delay_ms}]}`
- `import_cursor` — 仅 .cur，返回 `{width, height, hotspot_x, hotspot_y, rgba}[]`

## 前端结构

- `src/state/` — 文档模型（frame/layer）与撤销栈；`mergeFramePixels()` 分层 alpha 合成、`fitPixelsToSize()`/`scalePixelsToSquare()` 导入适配、`translateLayerPixels()`/`flipLayerPixels()` 供移动/翻转工具、`serializeDocument()`/`deserializeDocument()` 供 .vpc 项目文件（RGBA base64，version=1）
- `src/editor/` — 画布渲染（canvas.ts：棋盘格/白/黑背景 + 网格开关 + 洋葱皮）与工具逻辑（tools.ts：不透明度、矩形/椭圆）
- `src/preview/` — 模拟桌面预览 + 动画播放（rAF 驱动 tick）
- `src/main.ts` — 应用入口，装配图层/时间轴/播放/导入导出事件
- `src/api.ts` — Tauri 命令封装（导出合成后像素，导入统一走 import_file；PNG 导出与 .vpc 保存走前端编码，另提供 `importFileData()` 供拖拽导入）

## 工具与交互

- 工具：画笔(b) 橡皮(e) 填充(g) 直线(l) 矩形(r) 椭圆(o) 取色(i) 热点(h) 移动图层(m)，直线/矩形/椭圆为起止点拖拽（快照预览）
- 绘制不透明度：工具栏滑块 10–100%，作用于画笔/填充/直线/矩形/椭圆
- 填充容差：属性面板滑块 0–128（0 = 精确匹配），配合填充工具做改色
- 快键：按住右键画笔 = 临时橡皮；Alt+点击 任意处 = 临时取色；空格/中键拖拽 = 平移画布；滚轮 = 缩放（Ctrl+0 适配窗口，Ctrl+S/O 保存打开）
- 画布背景：棋盘格/白/黑；网格可开关（属性面板）；缩放可在 1x–32x；滚轮缩放、空格/中键平移；画布区默认居中
- 右侧面板（图层/属性/预览/历史）标题可点击收起/展开
- 图层面板：增删/复制/可见切换/上移下移/水平垂直翻转/双击重命名
- 时间轴（底部横排）：帧增删/复制/移动、每帧延迟、洋葱皮、播放与速度、一键填充全部帧（同步图层与热点）
- 历史面板：操作标签记录（`pushUndo(label)`），点击任意历史行跳转撤销/重做
- 导出 .cur 弹窗多选尺寸（实时预览）；.ani 全部帧；PNG 当前帧；.vpc 项目保存/打开（定可拖入窗口）
- 导入自动检测 cur/ani/png/gif/bmp/jpg；支持把文件拖入窗口直接导入；图片/非方形导入时弹窗选择缩放目标尺寸（居中保留透明）

## 格式要点

- CUR 中 `planes`/`bitcount` 字段分别复用为热点 X/Y
- DIB 高度为像素高度 ×2（AND mask 双倍高度），32bit 时 XOR 为 BGRA
- ANI 的 `iDispRate`/`rate` 以 jiffies（1/60s）为单位；`anih` 块 36 字节（含 cbSize），LIST/fram 内嵌单图 ICONDIR(icon 块)
- 前端分层导出前用 `mergeFramePixels` 做 source-over 合成，各层 opacity/visible 参与合成