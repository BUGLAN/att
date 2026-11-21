//! 记录批量导出（记录集「导出选中」「导出当前结果」）：Markdown / TXT / JSON / CSV。
//!
//! - Markdown：每条一节，本地时间作标题、正文在下；TXT：每条一段，首行是 `[时间]`；
//! - JSON：数组，字段与单条导出一致并带上引擎与语言；
//! - CSV：列为 时间、时长、状态、原文、优化文本；UTF-8 带 BOM（Excel 直接打开不乱码），
//!   以 `= + - @` 等开头的单元格前加 `'`，防止被表格软件当成公式执行。
//!
//! 正文取哪一份：`use_optimized` 且有优化文本时用优化文本，否则用原文（与单条导出一致）。

use super::{Record, RecordStatus};
use serde_json::json;

/// 导出格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Markdown,
    Text,
    Json,
    Csv,
}

impl ExportFormat {
    /// `md` / `txt` / `json` / `csv`（不区分大小写；`markdown` 同 `md`）。
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "md" | "markdown" => Some(Self::Markdown),
            "txt" => Some(Self::Text),
            "json" => Some(Self::Json),
            "csv" => Some(Self::Csv),
            _ => None,
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Text => "txt",
            Self::Json => "json",
            Self::Csv => "csv",
        }
    }

    pub fn content_type(&self) -> &'static str {
        match self {
            Self::Markdown => "text/markdown; charset=utf-8",
            Self::Text => "text/plain; charset=utf-8",
            Self::Json => "application/json",
            Self::Csv => "text/csv; charset=utf-8",
        }
    }
}

/// 记录创建时间的本地时间文本（`2026-11-21 14:05`）；读不出来时原样返回。
fn local_time(created_at: &str, with_seconds: bool) -> String {
    match chrono::DateTime::parse_from_rfc3339(created_at) {
        Ok(time) => {
            let pattern = if with_seconds { "%Y-%m-%d %H:%M:%S" } else { "%Y-%m-%d %H:%M" };
            time.with_timezone(&chrono::Local).format(pattern).to_string()
        }
        Err(_) => created_at.to_string(),
    }
}

/// 状态的界面名称（CSV 用）。
fn status_label(status: RecordStatus) -> &'static str {
    match status {
        RecordStatus::Pending => "待处理",
        RecordStatus::Transcribing => "转写中",
        RecordStatus::Transcribed => "已识别",
        RecordStatus::Optimizing => "优化中",
        RecordStatus::Optimized => "已优化",
        RecordStatus::Failed => "失败",
    }
}

/// 正文：优先优化文本（`use_optimized` 时），没有就用原文。
fn body_text(record: &Record, use_optimized: bool) -> String {
    let optimized = record.optimized_text.as_deref().filter(|text| use_optimized && !text.trim().is_empty());
    optimized.or(record.raw_text.as_deref()).unwrap_or("").trim().to_string()
}

/// CSV 单元格：需要时加引号；以公式字符开头时前加 `'`。
fn csv_cell(value: &str) -> String {
    let guarded =
        if value.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{value}") } else { value.to_string() };
    if guarded.contains([',', '"', '\n', '\r']) || guarded.starts_with(' ') || guarded.ends_with(' ') {
        format!("\"{}\"", guarded.replace('"', "\"\""))
    } else {
        guarded
    }
}

/// 下载文件名：`att-records-<本地时间>-<条数>.<扩展名>`。
pub fn file_name(format: ExportFormat, count: usize, now: chrono::DateTime<chrono::Local>) -> String {
    format!("att-records-{}-{count}.{}", now.format("%Y%m%d-%H%M"), format.extension())
}

/// 按格式生成导出内容（`records` 已按需要的顺序排好）。
pub fn render(records: &[Record], format: ExportFormat, use_optimized: bool) -> String {
    match format {
        ExportFormat::Markdown => {
            let mut out = format!("# 落墨记录导出\n\n共 {} 条。\n", records.len());
            for record in records {
                let text = body_text(record, use_optimized);
                out.push_str(&format!(
                    "\n## {}\n\n{}\n",
                    local_time(&record.created_at, false),
                    if text.is_empty() { "（没有文字）" } else { &text }
                ));
            }
            out
        }
        ExportFormat::Text => records
            .iter()
            .map(|record| {
                format!("[{}]\n{}\n", local_time(&record.created_at, false), body_text(record, use_optimized))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        ExportFormat::Json => {
            let items: Vec<_> = records
                .iter()
                .map(|record| {
                    json!({
                        "id": record.id,
                        "created_at": record.created_at,
                        "duration": record.duration,
                        "status": record.status.as_str(),
                        "raw_text": record.raw_text,
                        "optimized_text": record.optimized_text,
                        "engine": record.engine,
                        "language": record.language,
                    })
                })
                .collect();
            serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".into())
        }
        ExportFormat::Csv => {
            let mut out = String::from("\u{feff}时间,时长（秒）,状态,原文,优化文本\r\n");
            for record in records {
                let row = [
                    local_time(&record.created_at, true),
                    format!("{:.1}", record.duration),
                    status_label(record.status).to_string(),
                    record.raw_text.clone().unwrap_or_default(),
                    record.optimized_text.clone().unwrap_or_default(),
                ];
                out.push_str(&row.iter().map(|cell| csv_cell(cell)).collect::<Vec<_>>().join(","));
                out.push_str("\r\n");
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records() -> Vec<Record> {
        let at = |hour: u32| {
            chrono::Local::now()
                .date_naive()
                .and_hms_opt(hour, 5, 0)
                .unwrap()
                .and_local_timezone(chrono::Local)
                .unwrap()
                .with_timezone(&chrono::Utc)
                .to_rfc3339()
        };
        vec![
            Record {
                id: "rec-b".into(),
                created_at: at(14),
                duration: 4.26,
                raw_text: Some("原文 B, 带逗号".into()),
                optimized_text: Some("优化 B".into()),
                status: RecordStatus::Optimized,
                ..Default::default()
            },
            Record {
                id: "rec-a".into(),
                created_at: at(9),
                duration: 1.0,
                raw_text: Some("=SUM(A1) 说\"你好\"\n第二行".into()),
                status: RecordStatus::Failed,
                ..Default::default()
            },
        ]
    }

    #[test]
    fn 格式解析与扩展名() {
        assert_eq!(ExportFormat::parse("MD"), Some(ExportFormat::Markdown));
        assert_eq!(ExportFormat::parse("csv").map(|f| f.extension()), Some("csv"));
        assert_eq!(ExportFormat::parse("docx"), None);
    }

    #[test]
    fn markdown_每条一节_时间作标题_优先优化文本() {
        let out = render(&records(), ExportFormat::Markdown, true);
        assert!(out.starts_with("# 落墨记录导出\n\n共 2 条。"), "{out}");
        let headings: Vec<&str> = out.lines().filter(|line| line.starts_with("## ")).collect();
        assert_eq!(headings.len(), 2);
        assert!(headings[0].ends_with("14:05") && headings[1].ends_with("09:05"), "{headings:?}");
        assert!(out.contains("\n优化 B\n"), "有优化文本时用优化文本");
        assert!(out.contains("第二行"), "没有优化文本时用原文");
        assert!(!render(&records(), ExportFormat::Markdown, false).contains("优化 B"), "不用优化文本时取原文");
    }

    #[test]
    fn txt_与_json() {
        let txt = render(&records(), ExportFormat::Text, true);
        assert!(txt.starts_with('[') && txt.contains("]\n优化 B\n"), "{txt}");
        let json: serde_json::Value = serde_json::from_str(&render(&records(), ExportFormat::Json, true)).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 2);
        assert_eq!(json[0]["id"], "rec-b");
        assert_eq!(json[1]["status"], "failed");
    }

    #[test]
    fn csv_表头_引号转义_防公式() {
        let csv = render(&records(), ExportFormat::Csv, true);
        let mut lines = csv.split("\r\n");
        assert_eq!(lines.next().unwrap(), "\u{feff}时间,时长（秒）,状态,原文,优化文本");
        let first = lines.next().unwrap();
        assert!(first.ends_with(",4.3,已优化,\"原文 B, 带逗号\",优化 B"), "{first}");
        assert!(csv.contains(",1.0,失败,\"'=SUM(A1) 说\"\"你好\"\"\n第二行\","), "公式前加 ' 并转义引号与换行：{csv}");
    }

    #[test]
    fn 文件名带时间与条数() {
        let now = chrono::Local::now();
        let name = file_name(ExportFormat::Csv, 12, now);
        assert!(name.starts_with("att-records-") && name.ends_with("-12.csv"), "{name}");
    }
}
