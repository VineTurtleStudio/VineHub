-- 阶段一第 5 步：元数据提取状态（探测完成标记，增量核心）
ALTER TABLE items ADD COLUMN meta_state TEXT NOT NULL DEFAULT 'none'
  CHECK (meta_state IN ('none','done','failed'));
