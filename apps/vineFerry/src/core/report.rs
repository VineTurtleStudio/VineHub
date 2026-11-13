use crate::core::copy::RunSummary;

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

pub fn summary_to_csv(summary: &RunSummary) -> String {
    let mut out = String::from("rel,size,verified,status,src_hash,dst_hash,message\r\n");
    for o in &summary.outcomes {
        out.push_str(&csv_escape(&o.rel));
        out.push(',');
        out.push_str(&o.size.to_string());
        out.push(',');
        out.push_str(if o.verified { "true" } else { "false" });
        out.push(',');
        out.push_str(&format!("{:?}", o.status));
        out.push(',');
        out.push_str(&csv_escape(o.src_hash.as_deref().unwrap_or("")));
        out.push(',');
        out.push_str(&csv_escape(o.dst_hash.as_deref().unwrap_or("")));
        out.push(',');
        out.push_str(&csv_escape(&o.message));
        out.push_str("\r\n");
    }
    out
}

pub fn summary_to_json(summary: &RunSummary) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(summary)
}

pub fn write_report(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(crate::core::paths::to_long_path(parent))?;
    }
    std::fs::write(crate::core::paths::to_long_path(path), content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::copy::{FileOutcome, FileStatus};

    fn sample() -> RunSummary {
        RunSummary {
            total: 2,
            ok: 1,
            bad: 1,
            skipped: 0,
            cancelled: 0,
            bytes: 10,
            outcomes: vec![
                FileOutcome {
                    rel: "a,b.txt".into(),
                    size: 5,
                    src_hash: Some("aa".into()),
                    dst_hash: Some("aa".into()),
                    verified: true,
                    status: FileStatus::Verified,
                    message: "ok".into(),
                },
                FileOutcome {
                    rel: "c.txt".into(),
                    size: 5,
                    src_hash: None,
                    dst_hash: None,
                    verified: false,
                    status: FileStatus::Failed,
                    message: "含,逗号".into(),
                },
            ],
        }
    }

    #[test]
    fn csv_has_header_and_escaping() {
        let csv = summary_to_csv(&sample());
        let lines: Vec<&str> = lines_of(&csv);
        assert_eq!(lines.len(), 3);
        assert!(lines[1].starts_with("\"a,b.txt\",5,true,Verified"));
        assert!(lines[2].contains("\"含,逗号\""));
    }

    #[test]
    fn json_roundtrip_parses() {
        let json = summary_to_json(&sample()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["ok"], 1);
        assert_eq!(v["bad"], 1);
    }

    fn lines_of(s: &str) -> Vec<&str> {
        s.split("\r\n").filter(|l| !l.is_empty()).collect()
    }
}
