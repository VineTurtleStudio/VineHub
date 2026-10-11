CREATE TABLE IF NOT EXISTS music_tracks (
  item_id INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
  artist TEXT,
  albumartist TEXT,
  album TEXT,
  disc INTEGER,
  track INTEGER,
  duration REAL,
  format TEXT,
  bitrate INTEGER,
  mbid TEXT
);

CREATE TABLE IF NOT EXISTS video_items (
  item_id INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
  series TEXT,
  season INTEGER,
  episode INTEGER,
  resolution TEXT,
  codec TEXT,
  duration REAL,
  tmdb_id INTEGER
);

CREATE TABLE IF NOT EXISTS book_items (
  item_id INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
  author TEXT,
  series TEXT,
  volume TEXT,
  isbn TEXT,
  publisher TEXT,
  pages INTEGER,
  format TEXT,
  ocr_status TEXT NOT NULL DEFAULT 'none' CHECK (ocr_status IN ('none','queued','done','failed'))
);

CREATE TABLE IF NOT EXISTS comic_items (
  item_id INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
  series TEXT,
  volume TEXT,
  page_count INTEGER,
  scan_quality TEXT
);
