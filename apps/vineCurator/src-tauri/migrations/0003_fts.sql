CREATE VIRTUAL TABLE IF NOT EXISTS fts_items USING fts5(
  item_id UNINDEXED,
  title,
  body,
  tags_text,
  tokenize = 'unicode61 remove_diacritics 2'
);
