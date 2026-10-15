//! 详情面板与报告导出（JSON / CSV / HTML）

use std::path::PathBuf;

use eframe::egui;

use crate::{Report, Severity};

use super::severity_color;

/// 右侧详情面板：选中文件的问题编码 + 中文描述 + 字节偏移
pub fn ui(ui: &mut egui::Ui, report: Option<&Report>) {
    ui.add_space(4.0);
    ui.heading("详情");
    ui.add_space(2.0);
    let Some(r) = report else {
        ui.label(egui::RichText::new("在结果表中选择一个文件").weak());
        return;
    };

    let sev = r.worst();
    ui.horizontal(|ui| {
        ui.colored_label(severity_color(sev), sev.label());
        ui.label(
            r.path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| r.path.display().to_string()),
        );
    });
    ui.label(
        egui::RichText::new(r.path.display().to_string())
            .small()
            .weak(),
    );
    ui.label(
        egui::RichText::new("双击结果表行可在资源管理器中定位；右键可定位/打开/复制路径")
            .small()
            .weak(),
    );
    ui.add_space(4.0);
    ui.label(format!("类型：{} · 模式：{} · 耗时：{:.0}ms", r.file_type, super::mode_label(r.mode), r.duration.as_secs_f64() * 1000.0));
    ui.separator();

    if r.findings.is_empty() {
        ui.colored_label(severity_color(Severity::Ok), "未发现问题");
        return;
    }

    egui::ScrollArea::vertical().show(ui, |ui| {
        for f in &r.findings {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(severity_color(f.severity), f.severity.label());
                ui.label(egui::RichText::new(f.code).strong());
                if let Some(off) = f.offset {
                    ui.label(egui::RichText::new(format!("偏移 0x{off:X}")).weak());
                }
            });
            ui.label(&f.message);
            ui.add_space(6.0);
        }
    });
}

/// 保存对话框；用户取消返回 None
fn save_dialog(default_name: &str, filter_name: &str, ext: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_file_name(default_name)
        .add_filter(filter_name, &[ext])
        .save_file()
}

pub fn export_json(reports: &[Report]) -> Result<Option<PathBuf>, String> {
    let Some(path) = save_dialog("vine-pulse-report.json", "JSON 报告", "json") else {
        return Ok(None);
    };
    let json = serde_json::to_string_pretty(reports).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("{}：{e}", path.display()))?;
    Ok(Some(path))
}

/// CSV：一行一个文件；问题编码/说明以 “; ” 连接；UTF-8 BOM 便于 Excel 打开
pub fn export_csv(reports: &[Report]) -> Result<Option<PathBuf>, String> {
    let Some(path) = save_dialog("vine-pulse-report.csv", "CSV 报告", "csv") else {
        return Ok(None);
    };
    let mut out = String::from("\u{FEFF}"); // BOM
    out.push_str("路径,文件类型,模式,结论,问题数,问题编码,问题说明,字节偏移,耗时(ms)\n");
    for r in reports {
        let codes: Vec<&str> = r.findings.iter().map(|f| f.code).collect();
        let msgs: Vec<String> = r.findings.iter().map(|f| f.message.clone()).collect();
        let offsets: Vec<String> = r
            .findings
            .iter()
            .map(|f| {
                f.offset
                    .map(|o| format!("0x{o:X}"))
                    .unwrap_or_else(String::new)
            })
            .collect();
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{}\n",
            csv_escape(&r.path.display().to_string()),
            csv_escape(&r.file_type),
            csv_escape(super::mode_label(r.mode)),
            csv_escape(r.worst().label()),
            r.problem_count(),
            csv_escape(&codes.join("; ")),
            csv_escape(&msgs.join("; ")),
            csv_escape(&offsets.join("; ")),
            r.duration.as_millis(),
        ));
    }
    std::fs::write(&path, out).map_err(|e| format!("{}：{e}", path.display()))?;
    Ok(Some(path))
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// HTML：独立单文件报告（内联样式，含汇总 + 每文件问题明细）
pub fn export_html(reports: &[Report]) -> Result<Option<PathBuf>, String> {
    let Some(path) = save_dialog("vine-pulse-report.html", "HTML 报告", "html") else {
        return Ok(None);
    };
    let (mut ok, mut warning, mut corrupt, mut unknown) = (0usize, 0usize, 0usize, 0usize);
    for r in reports {
        match r.worst() {
            Severity::Ok => ok += 1,
            Severity::Warning => warning += 1,
            Severity::Corrupt => corrupt += 1,
            Severity::Unknown => unknown += 1,
        }
    }
    let color = |s: Severity| match s {
        Severity::Ok => "#50b050",
        Severity::Warning => "#daa520",
        Severity::Corrupt => "#dc4646",
        Severity::Unknown => "#50aad2",
    };

    let mut html = String::new();
    html.push_str("<!DOCTYPE html>\n<html lang=\"zh-CN\">\n<head>\n<meta charset=\"utf-8\">\n");
    html.push_str("<title>vinePulse 检测报告</title>\n<style>\n");
    html.push_str("body{font-family:'Microsoft YaHei',sans-serif;margin:24px;color:#222}\n");
    html.push_str("table{border-collapse:collapse;width:100%}\n");
    html.push_str("th,td{border:1px solid #ddd;padding:6px 8px;text-align:left;font-size:13px}\n");
    html.push_str("th{background:#f5f5f5}\ntr:nth-child(even){background:#fafafa}\n");
    html.push_str(".path{color:#888;font-size:12px}\n.tag{padding:1px 8px;border-radius:3px;color:#fff}\n");
    html.push_str("</style>\n</head>\n<body>\n");
    html.push_str("<h1>vinePulse 检测报告</h1>\n");
    html.push_str(&format!(
        "<p>共 {} 个文件 —— 正常 <b>{ok}</b> · 可疑 <b>{warning}</b> · 损坏 <b>{corrupt}</b> · 未知 <b>{unknown}</b></p>\n",
        reports.len()
    ));
    html.push_str("<table>\n<tr><th>文件</th><th>类型</th><th>模式</th><th>结论</th><th>问题</th></tr>\n");
    for r in reports {
        let sev = r.worst();
        html.push_str("<tr>");
        html.push_str(&format!(
            "<td>{}<br><span class=\"path\">{}</span></td>",
            html_escape(
                &r.path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| r.path.display().to_string())
            ),
            html_escape(&r.path.display().to_string())
        ));
        html.push_str(&format!("<td>{}</td>", html_escape(&r.file_type)));
        html.push_str(&format!("<td>{}</td>", super::mode_label(r.mode)));
        html.push_str(&format!(
            "<td><span class=\"tag\" style=\"background:{}\">{}</span></td>",
            color(sev),
            sev.label()
        ));
        html.push_str("<td>");
        if r.findings.is_empty() {
            html.push('—');
        } else {
            html.push_str("<ul style=\"margin:0;padding-left:18px\">");
            for f in &r.findings {
                let off = f
                    .offset
                    .map(|o| format!(" @ 0x{o:X}"))
                    .unwrap_or_default();
                html.push_str(&format!(
                    "<li><b>[{}] {}</b>{}：{}</li>",
                    f.severity.label(),
                    html_escape(f.code),
                    off,
                    html_escape(&f.message)
                ));
            }
            html.push_str("</ul>");
        }
        html.push_str("</td></tr>\n");
    }
    html.push_str("</table>\n</body>\n</html>\n");
    std::fs::write(&path, html).map_err(|e| format!("{}：{e}", path.display()))?;
    Ok(Some(path))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
