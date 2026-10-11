use serde::Deserialize;
use std::collections::HashMap;

/// exiftool `-j` 输出解析（JSON 数组，取第一个对象）
#[derive(Debug, Default, Deserialize)]
pub struct ExifData {
    #[serde(flatten)]
    pub tags: HashMap<String, serde_json::Value>,
}

fn get_str(d: &ExifData, keys: &[&str]) -> Option<String> {
    for k in keys {
        for (tk, tv) in &d.tags {
            if tk.eq_ignore_ascii_case(k) {
                match tv {
                    serde_json::Value::String(s) if !s.trim().is_empty() => return Some(s.clone()),
                    serde_json::Value::Number(n) => return Some(n.to_string()),
                    _ => {}
                }
            }
        }
    }
    None
}

fn get_num(d: &ExifData, keys: &[&str]) -> Option<i64> {
    let raw = get_str(d, keys)?;
    raw.split('/').next().unwrap_or("").trim().parse().ok()
}

impl ExifData {
    pub fn author(&self) -> Option<String> {
        get_str(self, &["Author", "Creator", "Artist", "dc:creator"])
    }

    pub fn series(&self) -> Option<String> {
        get_str(self, &["Series", "SeriesTitle", "comic:series"])
    }

    pub fn volume(&self) -> Option<String> {
        get_str(self, &["Volume", "SeriesIndex", "comic:volume"])
    }

    pub fn isbn(&self) -> Option<String> {
        get_str(self, &["ISBN", "ISBN-13", "isbn"])
    }

    pub fn publisher(&self) -> Option<String> {
        get_str(self, &["Publisher", "dc:publisher"])
    }

    pub fn page_count(&self) -> Option<i64> {
        get_num(self, &["PageCount", "Pages", "NumberOfPages"])
    }

    pub fn format(&self) -> Option<String> {
        get_str(self, &["FileTypeExtension", "Format", "FileType"])
    }

    pub fn year(&self) -> Option<i64> {
        let raw = get_str(self, &["Year", "CreateDate", "PublishDate", "ReleaseDate"])?;
        super::ffprobe::extract_year(&raw)
    }
}

pub fn parse(json: &str) -> Result<ExifData, String> {
    // exiftool -j 输出是数组
    let arr: Vec<HashMap<String, serde_json::Value>> =
        serde_json::from_str(json).map_err(|e| format!("exiftool JSON 解析失败: {e}"))?;
    let tags = arr.into_iter().next().unwrap_or_default();
    Ok(ExifData { tags })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_book_fixture() {
        let json = r#"[{
          "SourceFile": "d.epub",
          "Title": "Dune",
          "Author": "Frank Herbert",
          "ISBN": "978-0441013593",
          "Publisher": "Ace",
          "PageCount": 412,
          "Series": "Dune Saga",
          "SeriesIndex": 1,
          "PublishDate": "1998/06/01"
        }]"#;
        let d = parse(json).unwrap();
        assert_eq!(d.author().as_deref(), Some("Frank Herbert"));
        assert_eq!(d.series().as_deref(), Some("Dune Saga"));
        assert_eq!(d.volume().as_deref(), Some("1"));
        assert_eq!(d.isbn().as_deref(), Some("978-0441013593"));
        assert_eq!(d.publisher().as_deref(), Some("Ace"));
        assert_eq!(d.page_count(), Some(412));
        assert_eq!(d.year(), Some(1998));
    }

    #[test]
    fn parse_empty_array() {
        let d = parse("[]").unwrap();
        assert!(d.author().is_none());
        assert!(d.page_count().is_none());
    }
}
