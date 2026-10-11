pub mod rules;

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileStat {
    pub name: String,
    pub size: u64,
}

/// 扫描得到的目录树（纯数据，可序列化给前端）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirNode {
    pub name: String,
    pub rel_path: String,
    pub files: Vec<FileStat>,
    pub children: Vec<DirNode>,
    /// 递归总字节数（含所有文件）
    pub total_size: u64,
    /// 递归文件数（含系统文件）
    pub total_files: u32,
}

impl DirNode {
    pub fn has_content(&self) -> bool {
        self.files.iter().any(|f| rules::is_content_file(&f.name))
    }

    pub fn has_meaningful_file(&self) -> bool {
        self.files.iter().any(|f| rules::is_meaningful_file(&f.name))
    }
}

/// 扫描根目录，返回根节点（根永远视为容器）
pub fn scan_tree(root: &Path) -> Result<DirNode, String> {
    let meta = fs::metadata(root).map_err(|e| format!("无法访问 {}: {}", root.display(), e))?;
    if !meta.is_dir() {
        return Err(format!("{} 不是目录", root.display()));
    }
    let root_name = root
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| root.display().to_string());
    let mut node = scan_dir(root, &root_name, "")?;
    node.rel_path = String::new();
    Ok(node)
}

fn scan_dir(path: &Path, name: &str, rel_path: &str) -> Result<DirNode, String> {
    let mut node = DirNode {
        name: name.to_string(),
        rel_path: rel_path.to_string(),
        files: Vec::new(),
        children: Vec::new(),
        total_size: 0,
        total_files: 0,
    };

    let rd = match fs::read_dir(path) {
        Ok(rd) => rd,
        Err(e) => return Err(format!("读取 {} 失败: {}", path.display(), e)),
    };

    let mut subdirs: Vec<(String, PathBuf)> = Vec::new();
    for entry in rd.flatten() {
        let fname = entry.file_name().to_string_lossy().to_string();
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.is_dir() {
            if is_reparse_point(&meta) {
                continue;
            }
            subdirs.push((fname, entry.path()));
        } else if meta.is_file() {
            node.files.push(FileStat {
                name: fname,
                size: meta.len(),
            });
        }
    }

    node.files.sort_by(|a, b| a.name.cmp(&b.name));
    subdirs.sort_by(|a, b| a.0.cmp(&b.0));

    for (child_name, child_path) in subdirs {
        let child_rel = rules::join_rel(rel_path, &child_name);
        match scan_dir(&child_path, &child_name, &child_rel) {
            Ok(child) => {
                node.total_size += child.total_size;
                node.total_files += child.total_files;
                node.children.push(child);
            }
            Err(e) => {
                eprintln!("[scan] {}", e);
            }
        }
    }

    for f in &node.files {
        node.total_size += f.size;
        node.total_files += 1;
    }

    Ok(node)
}

fn is_reparse_point(meta: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        meta.file_type().is_symlink()
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
    }
}

/// 单位（最小压缩单位）统计，供说明文件与界面使用
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UnitStats {
    pub total_size: u64,
    pub file_count: u32,
    pub image_count: u32,
    pub audio_count: u32,
    pub video_count: u32,
    pub doc_count: u32,
    pub archive_count: u32,
    pub text_count: u32,
    /// 直接子目录（分卷 / 碟片）
    pub subdirs: Vec<SubdirStat>,
    /// 单位直属内容文件名（不含子目录内）：拆分警示用——拆成子单位后这些散件不属于任何子单位
    #[serde(default)]
    pub direct_files: Vec<String>,
    /// 内容文件名（历史字段：已停止填充，旧项目仍可反序列化）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sample_names: Vec<String>,
    /// 内容文件/子目录名摘要，供 `rules::infer_kind` 做文件名级判定；不进序列化与项目文件
    #[serde(skip)]
    pub hint: String,
    // ---- 判类证据（评分层 `rules::kind_scores` 消费；不进序列化与项目文件）----
    /// 字节分项：音频。全 0 时评分层退回计数占比
    #[serde(skip)]
    pub audio_b: u64,
    #[serde(skip)]
    pub image_b: u64,
    #[serde(skip)]
    pub video_b: u64,
    /// 字节分项：文档（含电子书/漫画压缩包）
    #[serde(skip)]
    pub doc_b: u64,
    /// 字节分项：文本（txt/md/lrc/nfo/log/cue）
    #[serde(skip)]
    pub text_b: u64,
    #[serde(skip)]
    pub archive_b: u64,
    /// CUE 轨表（专辑强证据）
    #[serde(skip)]
    pub cue_count: u32,
    /// 抓轨日志
    #[serde(skip)]
    pub log_count: u32,
    /// 字幕文件（ass/srt/ssa/vtt/idx/sup/sub）
    #[serde(skip)]
    pub sub_count: u32,
    /// 电子书（epub/mobi/azw/azw3）
    #[serde(skip)]
    pub ebook_count: u32,
    /// 漫画压缩包（cbz/cbr）
    #[serde(skip)]
    pub comic_arch: u32,
    /// 散文文本（仅 txt/md，小说证据；排除 log/lrc/nfo 污染）
    #[serde(skip)]
    pub prose_count: u32,
    /// 曲目编号命名的音频数（`01 - Title.flac`）
    #[serde(skip)]
    pub tracks_numbered: u32,
    /// 连页编号命名的图片数（`001.jpg` / `p012.png`）
    #[serde(skip)]
    pub page_numbered: u32,
    /// 集数命名的视频数（S01E02 / EP03 / 第01話 / `01.mkv`）
    #[serde(skip)]
    pub episode_count: u32,
    /// 章回子目录数（第01章 / 第3話 / ch01）
    #[serde(skip)]
    pub chapter_dirs: u32,
    /// 分卷/碟片子目录数（`rules::is_part_name`）
    #[serde(skip)]
    pub part_dirs: u32,
    /// 内嵌封面与散图 dHash 匹配率（0..=1，kindx 后置填充）
    #[serde(skip)]
    pub cover_match: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubdirStat {
    pub name: String,
    pub file_count: u32,
    pub total_size: u64,
}

pub fn collect_unit_stats(node: &DirNode) -> UnitStats {
    let mut s = UnitStats {
        total_size: node.total_size,
        file_count: node.total_files,
        ..Default::default()
    };
    for c in &node.children {
        s.subdirs.push(SubdirStat {
            name: c.name.clone(),
            file_count: c.total_files,
            total_size: c.total_size,
        });
    }
    s.direct_files = node
        .files
        .iter()
        .filter(|f| rules::is_content_file(&f.name))
        .map(|f| f.name.clone())
        .collect();
    walk(node, &mut s);
    s
}

fn walk(node: &DirNode, s: &mut UnitStats) {
    for f in &node.files {
        let ext = rules::extension_of(&f.name).to_ascii_lowercase();
        if s.hint.len() < 4096 {
            s.hint.push(' ');
            s.hint.push_str(&f.name);
        }
        match ext.as_str() {
            "jpg" | "jpeg" | "jfif" | "png" | "gif" | "webp" | "bmp" | "tif" | "tiff"
            | "avif" | "heic" | "heif" | "jxl" => {
                s.image_count += 1;
                s.image_b += f.size;
                if rules::is_page_numbered(&f.name) {
                    s.page_numbered += 1;
                }
            }
            "flac" | "mp3" | "wav" | "ape" | "wv" | "m4a" | "m4b" | "ogg" | "oga" | "aac"
            | "alac" | "opus" | "ac3" | "dff" | "dsf" | "wma" | "dts" | "tta" | "mpc" | "ofr"
            | "shn" | "mka" | "aiff" | "aif" | "ra" | "mp2" | "vqf" => {
                s.audio_count += 1;
                s.audio_b += f.size;
                if rules::is_track_numbered(&f.name) {
                    s.tracks_numbered += 1;
                }
            }
            "mkv" | "mp4" | "avi" | "mov" | "flv" | "ts" | "m2ts" | "mpg" | "mpeg" | "rm"
            | "rmvb" | "webm" | "3gp" | "vob" | "wmv" => {
                s.video_count += 1;
                s.video_b += f.size;
                if rules::is_episode_named(&f.name) {
                    s.episode_count += 1;
                }
            }
            "epub" | "pdf" | "mobi" | "azw" | "azw3" | "cbz" | "cbr" | "djvu" | "doc"
            | "docx" | "rtf" | "odt" | "chm" | "xlsx" | "pptx" => {
                s.doc_count += 1;
                s.doc_b += f.size;
                if matches!(ext.as_str(), "epub" | "mobi" | "azw" | "azw3") {
                    s.ebook_count += 1;
                }
                if matches!(ext.as_str(), "cbz" | "cbr") {
                    s.comic_arch += 1;
                }
            }
            "rar" | "7z" | "zip" | "tar" | "gz" | "xz" | "iso" | "bin" | "img" => {
                s.archive_count += 1;
                s.archive_b += f.size;
            }
            "txt" | "lrc" | "nfo" | "md" | "log" => {
                s.text_count += 1;
                s.text_b += f.size;
                if matches!(ext.as_str(), "txt" | "md") {
                    s.prose_count += 1;
                }
                if ext == "log" {
                    s.log_count += 1;
                }
            }
            "cue" => {
                s.cue_count += 1;
                s.text_b += f.size;
            }
            "ass" | "srt" | "ssa" | "vtt" | "idx" | "sup" | "sub" => {
                s.sub_count += 1;
            }
            _ => {}
        }
    }
    for c in &node.children {
        if s.hint.len() < 4096 {
            s.hint.push(' ');
            s.hint.push_str(&c.name);
        }
        if rules::is_chapter_dir(&c.name) {
            s.chapter_dirs += 1;
        }
        if rules::is_part_name(&c.name) {
            s.part_dirs += 1;
        }
        walk(c, s);
    }
}
