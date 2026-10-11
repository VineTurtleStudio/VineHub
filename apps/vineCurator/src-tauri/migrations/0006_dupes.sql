-- 阶段四第 1 步：重复检测（只读媒体文件；partial_hash = sha256(size || 头 1MiB || 尾 1MiB)）
ALTER TABLE items ADD COLUMN partial_hash TEXT;

CREATE INDEX IF NOT EXISTS idx_items_size ON items(size);
