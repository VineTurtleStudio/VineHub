-- 0008: item_categories.source（manual|auto|rule）——自动分类重跑只动 auto 行，不降级手动/规则归类
ALTER TABLE item_categories ADD COLUMN source TEXT NOT NULL DEFAULT 'manual';
