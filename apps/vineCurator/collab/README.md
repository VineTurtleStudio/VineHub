# collab — 远端协同接口（阶段三不实现，仅占位）

> 对应 README 阶段三第 3 条：axum / Tailscale / 协同接口**不在本期**，此处只保留设计骨架。
> 本地优先不变量不变：SQLite 仍是唯一真相源，远端只做同步/访问层，绝不反向改写媒体文件。

## 已就绪的地基（阶段三已落地）

- `users`（Argon2 哈希、admin/editor/viewer 三角色）+ `audit_log`（所有写操作 100% 落账，见 `audit.rs` / `tests/audit_test.rs` 的命令覆盖率守护）。
- 登录会话当前驻留进程内存（`auth::AuthState`），角色门禁**尚未**接入命令——有意为之，留给远端阶段一并做。

## 设计草案（进入阶段三外部时再定稿）

1. **传输**：axum 只读 API（`GET /items`、`GET /collections`…）与写操作走独立 `PUT`，Tailscale 私网直连，不暴露公网端口；后续可选 HTTPS（自签 + pinned cert）。
2. **鉴权**：JWT（短时效）或 Tailscale identity 取信 + 本地 `users` 校验；`role` 门禁在 command 层统一 enforce：viewer=只读、editor=条目/标签/集合写、admin=根目录/规则/用户/设置。
3. **审计**：远端写操作复用 `audit::record`，`user_id` 为远端登录用户；本地与远端共享同一审计流。
4. **同步策略**：单写者假设（远端写完 → 本地读 WAL/复制数据库文件，或后续演进为 litestream/WAL 流）；冲突以 `updated_at` + 后写胜出，媒体文件永不被远端删除。
5. **与 vinePress 分工**：vinePress 仍是一次性归档打包，不经过 collab。
