use serde::Deserialize;
use std::collections::HashMap;

/// ffprobe `-print_format json -show_format -show_streams` 输出解析
#[derive(Debug, Default, Deserialize)]
pub struct FfprobeData {
    #[serde(default)]
    pub format: FormatInfo,
    #[serde(default)]
    pub streams: Vec<StreamInfo>,
}

#[derive(Debug, Default, Deserialize)]
pub struct FormatInfo {
    #[serde(default)]
    pub duration: Option<String>,
    #[serde(default)]
    pub bit_rate: Option<String>,
    #[serde(default)]
    pub format_name: Option<String>,
    #[serde(default)]
    pub tags: HashMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct StreamInfo {
    #[serde(default)]
    pub codec_type: Option<String>,
    #[serde(default)]
    pub codec_name: Option<String>,
    #[serde(default)]
    pub width: Option<i64>,
    #[serde(default)]
    pub height: Option<i64>,
}

impl FfprobeData {
    pub fn duration(&self) -> Option<f64> {
        self.format.duration.as_deref()?.parse().ok()
    }

    pub fn bit_rate(&self) -> Option<i64> {
        self.format.bit_rate.as_deref()?.parse().ok()
    }

    pub fn video_stream(&self) -> Option<&StreamInfo> {
        self.streams
            .iter()
            .find(|s| s.codec_type.as_deref() == Some("video"))
    }

    pub fn resolution(&self) -> Option<String> {
        let s = self.video_stream()?;
        Some(format!("{}x{}", s.width?, s.height?))
    }

    pub fn video_codec(&self) -> Option<String> {
        self.video_stream()?.codec_name.clone()
    }

    pub fn tag(&self, keys: &[&str]) -> Option<String> {
        find_tag(&self.format.tags, keys)
    }

    pub fn tag_int(&self, keys: &[&str]) -> Option<i64> {
        let raw = self.tag(keys)?;
        raw.split('/').next().unwrap_or("").trim().parse().ok()
    }

    pub fn year(&self) -> Option<i64> {
        let raw = self.tag(&["date", "year", "DATE", "YEAR"])?;
        extract_year(&raw)
    }
}

fn find_tag(tags: &HashMap<String, String>, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some((_, v)) = tags
            .iter()
            .find(|(tk, tv)| tk.eq_ignore_ascii_case(k) && !tv.trim().is_empty())
        {
            return Some(v.clone());
        }
    }
    None
}

/// 从 "2019-05-01" / "2019" / "2019:05:01" 提取年份
pub fn extract_year(raw: &str) -> Option<i64> {
    let digits: String = raw.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() >= 4 {
        let y: i64 = digits[..4].parse().ok()?;
        if (1000..=2999).contains(&y) {
            return Some(y);
        }
    }
    None
}

pub fn parse(json: &str) -> Result<FfprobeData, String> {
    serde_json::from_str(json).map_err(|e| format!("ffprobe JSON 解析失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_video_fixture() {
        let json = r#"{
          "format": {"duration": "3625.5", "bit_rate": "4500000", "format_name": "matroska,webm",
                     "tags": {"title": "Ep 01", "date": "2021-04-01"}},
          "streams": [
            {"codec_type": "audio", "codec_name": "aac"},
            {"codec_type": "video", "codec_name": "hevc", "width": 3840, "height": 2160}
          ]
        }"#;
        let d = parse(json).unwrap();
        assert_eq!(d.duration(), Some(3625.5));
        assert_eq!(d.bit_rate(), Some(4500000));
        assert_eq!(d.resolution().as_deref(), Some("3840x2160"));
        assert_eq!(d.video_codec().as_deref(), Some("hevc"));
        assert_eq!(d.year(), Some(2021));
    }

    #[test]
    fn parse_music_tags_case_insensitive() {
        let json = r#"{
          "format": {"duration": "225.1", "bit_rate": "900000", "format_name": "flac",
                     "tags": {"ARTIST": "Miles Davis", "ALBUM": "Kind of Blue",
                              "TRACK": "3/9", "DISC": "1/1",
                              "MUSICBRAINZ_TRACKID": "abcd-1234"}}
        }"#;
        let d = parse(json).unwrap();
        assert_eq!(d.tag(&["artist"]).as_deref(), Some("Miles Davis"));
        assert_eq!(d.tag(&["album"]).as_deref(), Some("Kind of Blue"));
        assert_eq!(d.tag_int(&["track"]), Some(3));
        assert_eq!(d.tag_int(&["disc"]), Some(1));
        assert_eq!(
            d.tag(&["musicbrainz_trackid"]).as_deref(),
            Some("abcd-1234")
        );
    }

    #[test]
    fn extract_year_variants() {
        assert_eq!(extract_year("2019"), Some(2019));
        assert_eq!(extract_year("2019-05-01"), Some(2019));
        assert_eq!(extract_year("2019:05:01"), Some(2019));
        assert_eq!(extract_year("abc"), None);
        assert_eq!(extract_year("999"), None);
    }
}
