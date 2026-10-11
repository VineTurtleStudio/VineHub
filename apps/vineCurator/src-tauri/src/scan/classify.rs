/// 四类分派：扩展名 → items.type（白名单外一律 other）
/// cbz/cbr → comic；epub/pdf/mobi → book；txt 归 other（歌词/字幕常见，书库可手工改）
pub fn classify_type(ext: &str) -> &'static str {
    match ext {
        // 音乐
        "flac" | "mp3" | "m4a" | "aac" | "ape" | "wav" | "wv" | "tta" | "ogg" | "opus" | "aiff"
        | "aif" | "dsf" | "dff" | "wma" | "mpc" | "m4b" => "music",
        // 视频
        "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" | "m2ts" | "mts" | "ts" | "mpg"
        | "mpeg" | "m4v" | "rm" | "rmvb" | "asf" | "3gp" | "vob" | "divx" => "video",
        // 电子书
        "epub" | "pdf" | "mobi" | "azw" | "azw3" | "djvu" | "djv" | "fb2" | "lit" | "prc"
        | "rtf" => "book",
        // 漫画归档
        "cbr" | "cbz" | "cb7" | "cbt" => "comic",
        // 图片
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "heic" | "heif"
        | "avif" | "svg" | "psd" | "cr2" | "nef" | "arw" | "rw2" | "orf" | "dng" => "image",
        _ => "other",
    }
}

/// 四类媒体的 1:1 扩展表名（image/other 无扩展表）
pub fn ext_table(type_name: &str) -> Option<&'static str> {
    match type_name {
        "music" => Some("music_tracks"),
        "video" => Some("video_items"),
        "book" => Some("book_items"),
        "comic" => Some("comic_items"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_four_media_types() {
        assert_eq!(classify_type("flac"), "music");
        assert_eq!(classify_type("mp3"), "music");
        assert_eq!(classify_type("mkv"), "video");
        assert_eq!(classify_type("epub"), "book");
        assert_eq!(classify_type("cbz"), "comic");
        assert_eq!(classify_type("cbr"), "comic");
    }

    #[test]
    fn classify_image_and_other() {
        assert_eq!(classify_type("jpg"), "image");
        assert_eq!(classify_type("png"), "image");
        assert_eq!(classify_type("nfo"), "other");
        assert_eq!(classify_type("srt"), "other");
        assert_eq!(classify_type("txt"), "other");
        assert_eq!(classify_type("cue"), "other");
        assert_eq!(classify_type(""), "other");
    }

    #[test]
    fn ext_table_maps_four_types_only() {
        assert_eq!(ext_table("music"), Some("music_tracks"));
        assert_eq!(ext_table("video"), Some("video_items"));
        assert_eq!(ext_table("book"), Some("book_items"));
        assert_eq!(ext_table("comic"), Some("comic_items"));
        assert_eq!(ext_table("image"), None);
        assert_eq!(ext_table("other"), None);
    }
}
