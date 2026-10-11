use serde::Serialize;
use std::collections::HashMap;
use std::process::{Command, Stdio};

#[derive(Debug, Serialize)]
pub struct ToolStatus {
    pub name: String,
    pub found: bool,
    pub version: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ToolsReport {
    pub tools: Vec<ToolStatus>,
}

const TOOLS: &[(&str, &[&str])] = &[
    ("ffprobe", &["-version"]),
    ("ffmpeg", &["-version"]),
    ("exiftool", &["-ver"]),
    ("beets", &["--version"]),
    ("ebook-meta", &["--version"]),
    ("ocrmypdf", &["--version"]),
    ("tesseract", &["--version"]),
    ("restic", &["--version"]),
];

/// settings 覆盖键：ebook-meta → ebook_meta_path
pub fn override_key(name: &str) -> String {
    format!("{}_path", name.replace('-', "_"))
}

fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

fn probe(name: &str, args: &[&str], override_path: Option<&str>) -> ToolStatus {
    let configured = override_path.map(str::trim).filter(|p| !p.is_empty());
    let program = configured.unwrap_or(name);
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let path = configured.map(str::to_string);
    match cmd.output() {
        Ok(out) if out.status.success() => {
            let text = if out.stdout.is_empty() {
                String::from_utf8_lossy(&out.stderr).into_owned()
            } else {
                String::from_utf8_lossy(&out.stdout).into_owned()
            };
            ToolStatus {
                name: name.to_string(),
                found: true,
                version: first_line(&text),
                path,
            }
        }
        Ok(_) => ToolStatus {
            name: name.to_string(),
            found: false,
            version: None,
            path,
        },
        Err(_) => ToolStatus {
            name: name.to_string(),
            found: false,
            version: None,
            path,
        },
    }
}

pub fn probe_tools(overrides: &HashMap<String, String>) -> ToolsReport {
    ToolsReport {
        tools: TOOLS
            .iter()
            .map(|(n, a)| {
                let ov = overrides.get(&override_key(n)).map(String::as_str);
                probe(n, a, ov)
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_missing_tool_reports_not_found() {
        let status = probe("vine-curator-no-such-tool", &["--version"], None);
        assert!(!status.found);
        assert!(status.version.is_none());
    }

    #[test]
    fn probe_tools_covers_all_declared() {
        let report = probe_tools(&HashMap::new());
        assert_eq!(report.tools.len(), TOOLS.len());
        assert_eq!(report.tools[0].name, "ffprobe");
        assert!(report.tools[0].path.is_none());
    }

    #[test]
    fn override_key_translates_dashes() {
        assert_eq!(override_key("ebook-meta"), "ebook_meta_path");
        assert_eq!(override_key("ffprobe"), "ffprobe_path");
    }

    #[test]
    fn probe_respects_broken_override() {
        let status = probe(
            "ffprobe",
            &["-version"],
            Some("C:\\vine-curator-no-such-tool\\ffprobe.exe"),
        );
        assert!(!status.found);
        assert_eq!(
            status.path.as_deref(),
            Some("C:\\vine-curator-no-such-tool\\ffprobe.exe"),
            "配置过的路径即使不可用也要回显"
        );
    }
}
