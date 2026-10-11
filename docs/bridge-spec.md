# VineHub 插件面板桥接协议（bridge-spec v1）

> 状态：P3.1 定稿。宿主（vine-hub）与插件前端（iframe 内页面）之间唯一的通信通道。

## 1. 通道与信封

插件前端以 `<iframe>` 嵌入宿主。通信使用 `window.postMessage`，信封固定为：

```jsonc
// 插件 → 宿主（请求）
{ "channel": "vinehub", "v": 1, "id": 1, "method": "fs.pick", "params": { } }

// 宿主 → 插件（成功响应；id 与请求一致）
{ "channel": "vinehub", "v": 1, "id": 1, "result": { } }

// 宿主 → 插件（失败响应）
{ "channel": "vinehub", "v": 1, "id": 1, "error": { "code": -32010, "message": "方法不在白名单" } }

// 宿主 → 插件（通知，无 id）
{ "channel": "vinehub", "v": 1, "method": "task.progress", "params": { } }
```

规则：

- `channel` 必须为 `"vinehub"`，`v` 必须为 `1`；宿主丢弃其余消息。
- 宿主只接受 `event.source === iframe.contentWindow` 的消息（防顶层页伪造）。
- 请求必须带标量 `id`（number/string）；宿主按 id 单调关联响应。
- 插件发往宿主的通知（无 id）当前不定义；插件若需上报事件请走 `log` 请求。
- 宿主发往插件的通知目前仅 `task.progress`（透传 sidecar 的 `task.progress` 参数）。

## 2. 方法白名单

宿主**只**允许下表方法。白名单外的方法一律回 `error.code = -32010`（BridgeMethodNotAllowed），不透传、不落盘。

| method | params | result | 说明 |
|---|---|---|---|
| `fs.pick` | `{ filters?: {name, extensions[]}[], multiple?: boolean }` | `{ paths: string[] }` | 系统文件选择对话框；取消返回空数组 |
| `config.get` | `{ key: string }` | `{ value: string \| null }` | 插件命名空间配置读取（`plugin.{pluginId}.cfg.{key}`） |
| `config.set` | `{ key: string, value: string }` | `{ ok: true }` | 插件命名空间配置写入 |
| `task.start` | `{ pluginId: string, method: string, params: object }` | `{ taskId: string }` | 以任意 RPC 方法启动任务（ack+终态）；`pluginId` 必须等于宿主分配给该面板的插件 id |
| `rpc.request` | `{ method: string, params?: object }` | sidecar 响应 `result` | **阻塞式代理**到本面板插件的 sidecar（单响应 RPC，如 `plugin.info`/`library.roots`/`search.items`）；宿主自动拉起 sidecar |
| `rpc.notify` | `{ method: string, params?: object }` | `{ ok: true }` | 通知类代理（如 `task.cancel` 原生通知）；不等待响应 |
| `task.list` | `{ pluginId?: string }` | `{ tasks: TaskView[] }` | 任务列表（含进度/终态对账） |
| `task.cancel` | `{ taskId: string }` | `{ ok: true }` | 按宿主记录 id 取消 |
| `task.retry` | `{ taskId: string }` | `{ taskId: string }` | 用参数快照重启，返回新记录 id |
| `notify` | `{ level: "info"\|"error", message: string }` | `{ ok: true }` | 宿主横幅提示 |
| `log` | `{ level?: "debug"\|"info"\|"warn"\|"error", message: string }` | `{ ok: true }` | 写入宿主 `{data}/logs/{pluginId}-ui.log` |

### 错误码

| code | 含义 |
|---|---|
| `-32010` | 方法不在白名单（BridgeMethodNotAllowed） |
| `-32602` | 参数非法 |
| `-32005` | 宿主内部错误（插件内部错误沿用各插件 RPC 错误码） |

## 3. 面板加载

- manifest `panels[].ui` 为**插件目录内相对路径**（如 `dist/index.html`），校验禁止绝对路径与 `..`。
- 宿主命令 `resolve_panel_ui(pluginId, panelId)` 返回该文件的绝对路径；前端用 `convertFileSrc` 转为 asset 协议 URL 作为 `iframe.src`。
- 宿主已启用 asset protocol 并对插件根目录放行 scope。
- iframe 沙箱属性：`allow="clipboard-read; clipboard-write"`；不开放 `allow-same-origin` 之外的危险特性；同源仅用于 postMessage 定位，插件页面不得直接访问宿主 DOM。

## 4. 双模适配（插件侧）

每个插件前端提供 `src/host-bridge.ts`：

- **宿主模式判定**：`window.parent !== window` **且** URL 查询参数 `vinehub=1`
  （宿主在拼接 iframe src 时追加；查询参数在页面脚本执行前即存在，无注入时序竞态）。
- 宿主模式 → `postMessage` 桥接（`rpc.request`/`rpc.notify` 走本插件 sidecar）；
- 独立运行 → 原生 `invoke`，行为与改造前完全一致。

推荐收敛形态（各 app `api.ts` 逐函数改造，其余调用方不改）：

```ts
// host-bridge.ts 提供
hubInvoke<T>(rpcMethod: string | null, invokeName: string, args: Record<string, unknown>): Promise<T>
// 独立：invoke(invokeName, args)；hub：rpc.request { method: rpcMethod, params: args }
// rpcMethod 为 null 的命令仅独立可用（如宿主专属的 UI 操作）。
```

所有 Tauri 调用收敛在各插件 `src/api.ts`，适配层只改这一处。

## 5. 宿主实现位置

| 职责 | 位置 |
|---|---|
| 信封校验 + 方法路由 | `apps/vine-hub/src/components/PanelFrame.tsx` |
| `resolve_panel_ui` / `task_start` / `config_get` / `config_set` / `host_log` | `apps/vine-hub/src-tauri/src/commands.rs` |
| asset protocol scope | `apps/vine-hub/src-tauri/src/lib.rs`（setup 时对插件目录放行） |
| 面板声明 | 各 `plugins/*/plugin.json` 的 `panels` 数组 |

## 6. 边界（明确不做）

- 插件 UI 之间不共享组件/状态；
- 不提供插件互调——编排只发生在宿主层；
- 不透传任意 Tauri command；白名单即封闭集合，新增方法须改本 spec + 宿主路由表。
