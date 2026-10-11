use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};

/// 探测执行器：按类型选 ffprobe（video/music）或 exiftool（book/comic）
pub trait Runner: Send {
    fn probe(&mut self, abs_path: &Path, type_name: &str) -> Result<String, String>;
}

pub struct ToolRunner {
    pub ffprobe: String,
    pub exiftool: String,
}

impl ToolRunner {
    pub fn from_overrides(overrides: &HashMap<String, String>) -> Self {
        ToolRunner {
            ffprobe: overrides
                .get("ffprobe_path")
                .filter(|p| !p.trim().is_empty())
                .cloned()
                .unwrap_or_else(|| "ffprobe".into()),
            exiftool: overrides
                .get("exiftool_path")
                .filter(|p| !p.trim().is_empty())
                .cloned()
                .unwrap_or_else(|| "exiftool".into()),
        }
    }
}

fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
}

fn run_json(program: &str, args: &[&str], abs_path: &Path) -> Result<String, String> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .arg(abs_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_window(&mut cmd);
    let out = cmd
        .output()
        .map_err(|e| format!("无法启动 {program}: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "{program} 退出码 {:?}: {}",
            out.status.code(),
            err.lines().next().unwrap_or("")
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

impl Runner for ToolRunner {
    fn probe(&mut self, abs_path: &Path, type_name: &str) -> Result<String, String> {
        match type_name {
            "video" | "music" => run_json(
                &self.ffprobe,
                &[
                    "-v",
                    "quiet",
                    "-print_format",
                    "json",
                    "-show_format",
                    "-show_streams",
                ],
                abs_path,
            ),
            "book" | "comic" => run_json(&self.exiftool, &["-j", "-G1"], abs_path),
            other => Err(format!("不支持探测类型: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct FakeRunner;

    impl Runner for FakeRunner {
        fn probe(&mut self, _abs_path: &Path, type_name: &str) -> Result<String, String> {
            match type_name {
                "video" => Ok(r#"{"format":{"duration":"100.0"},"streams":[{"codec_type":"video","codec_name":"h264","width":1920,"height":1080}]}"#.into()),
                "book" => Ok(r#"[{"Title":"T","Author":"A","PageCount":10}]"#.into()),
                other => Err(format!("no tool for {other}")),
            }
        }
    }

    #[test]
    fn fake_runner_dispatch() {
        let mut r = FakeRunner;
        let p = PathBuf::from("/tmp/a.mkv");
        assert!(r.probe(&p, "video").is_ok());
        assert!(r.probe(&p, "book").is_ok());
        assert!(r.probe(&p, "image").is_err());
    }

    #[test]
    fn tool_runner_defaults_from_env_names() {
        let t = ToolRunner::from_overrides(&HashMap::new());
        assert_eq!(t.ffprobe, "ffprobe");
        assert_eq!(t.exiftool, "exiftool");
    }

    #[test]
    fn tool_runner_respects_overrides() {
        let mut m = HashMap::new();
        m.insert("ffprobe_path".to_string(), "C:/x/ffprobe.exe".to_string());
        m.insert("exiftool_path".to_string(), "  ".to_string());
        let t = ToolRunner::from_overrides(&m);
        assert_eq!(t.ffprobe, "C:/x/ffprobe.exe");
        assert_eq!(t.exiftool, "exiftool");
    }
}
