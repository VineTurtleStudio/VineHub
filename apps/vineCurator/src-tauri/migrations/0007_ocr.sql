-- 阶段四第 2 步：OCR 队列（原始媒体只读；文本入库供 FTS，派生 PDF 落 derived/ocr/）
ALTER TABLE book_items ADD COLUMN ocr_text TEXT;
ALTER TABLE book_items ADD COLUMN ocr_mtime INTEGER;
