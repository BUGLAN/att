//! 用量统计：按本机日期汇总记录条数、录音时长、识别字数与 LLM 优化 token（GET /api/stats）。
//!
//! 口径：
//! - 按记录的创建时间（本机时区）归到某一天；创建时间读不出来、或晚于今天的记录不计；
//! - 条数含录音、文件转写与文本记录（失败的也算，它同样占用了一次识别）；
//! - 字数 = 原文（`raw_text`）里的非空白字符数，中文按字、英文按字母计；
//! - token = 记录上累计的 `optimize_tokens`，旧记录没有这个字段按 0 计；
//! - 不做费用换算，也不按服务商分开统计。
//!
//! 数据来自记录仓库的内存索引（[`super::RecordRepo::usage_stats`]），不逐个读文件。

use super::{parse_record_time, Record};
use chrono::NaiveDate;
use serde::Serialize;
use std::collections::BTreeMap;

/// 统计范围：近 7 天 / 近 30 天（都含今天）/ 全部。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatsRange {
    Days7,
    Days30,
    All,
}

impl StatsRange {
    /// 解析查询参数 `range`：`7d` / `30d` / `all`。
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "7d" => Some(Self::Days7),
            "30d" => Some(Self::Days30),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Days7 => "7d",
            Self::Days30 => "30d",
            Self::All => "all",
        }
    }

    /// 固定天数的范围往前回溯的天数（含今天）；`All` 没有固定天数。
    fn days(&self) -> Option<i64> {
        match self {
            Self::Days7 => Some(7),
            Self::Days30 => Some(30),
            Self::All => None,
        }
    }
}

/// 一段时间内的用量合计。
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct UsageTotals {
    /// 记录条数
    pub count: u64,
    /// 录音时长（秒）
    pub duration: f64,
    /// 识别字数（原文非空白字符数）
    pub chars: u64,
    /// LLM 优化用掉的 token
    pub tokens: u64,
}

impl UsageTotals {
    fn add_record(&mut self, record: &Record) {
        self.count += 1;
        if record.duration.is_finite() && record.duration > 0.0 {
            self.duration += record.duration;
        }
        self.chars += record.raw_text.as_deref().map(text_chars).unwrap_or(0);
        self.tokens = self.tokens.saturating_add(record.optimize_tokens.unwrap_or(0));
    }

    fn add(&mut self, other: &UsageTotals) {
        self.count += other.count;
        self.duration += other.duration;
        self.chars += other.chars;
        self.tokens = self.tokens.saturating_add(other.tokens);
    }

    /// 时长保留两位小数输出（浮点累加的尾差不进响应）。
    fn rounded(mut self) -> Self {
        self.duration = (self.duration * 100.0).round() / 100.0;
        self
    }
}

/// 某一天的用量。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DayUsage {
    /// 本机日期 `YYYY-MM-DD`
    pub date: String,
    #[serde(flatten)]
    pub usage: UsageTotals,
}

/// GET /api/stats 的响应。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UsageStats {
    pub range: &'static str,
    /// 范围的第一天；`all` 且没有记录时为 None
    pub from: Option<String>,
    /// 范围的最后一天（今天）
    pub to: String,
    /// 今天的用量（与 `range` 无关，记录集顶部那行小统计用）
    pub today: UsageTotals,
    /// 范围内的合计
    pub total: UsageTotals,
    /// 范围内逐日用量（从 `from` 到 `to` 每天一条，没有记录的日子为 0，按日期升序）
    pub days: Vec<DayUsage>,
}

/// 原文字数：非空白字符数。
pub fn text_chars(text: &str) -> u64 {
    text.chars().filter(|c| !c.is_whitespace()).count() as u64
}

/// 按天汇总（纯函数：`today` 由调用方给，测试可固定日期）。
pub fn summarize<'a>(records: impl IntoIterator<Item = &'a Record>, range: StatsRange, today: NaiveDate) -> UsageStats {
    let mut by_day: BTreeMap<NaiveDate, UsageTotals> = BTreeMap::new();
    for record in records {
        let Some(day) = parse_record_time(&record.created_at).map(|time| time.date_naive()) else { continue };
        if day > today {
            continue;
        }
        by_day.entry(day).or_default().add_record(record);
    }
    let today_usage = by_day.get(&today).copied().unwrap_or_default().rounded();

    let from = match range.days() {
        Some(days) => Some(today - chrono::Duration::days(days - 1)),
        None => by_day.keys().next().copied(),
    };
    let mut total = UsageTotals::default();
    let mut days = Vec::new();
    if let Some(from) = from {
        for day in from.iter_days().take_while(|day| *day <= today) {
            let usage = by_day.get(&day).copied().unwrap_or_default();
            total.add(&usage);
            days.push(DayUsage { date: day.format("%Y-%m-%d").to_string(), usage: usage.rounded() });
        }
    }
    UsageStats {
        range: range.as_str(),
        from: from.map(|day| day.format("%Y-%m-%d").to_string()),
        to: today.format("%Y-%m-%d").to_string(),
        today: today_usage,
        total: total.rounded(),
        days,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, NaiveTime, TimeZone};

    /// 本机时区某天中午的 RFC3339 时间（统计按本机日期归天）。
    fn at(date: &str) -> String {
        let day = NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap();
        Local.from_local_datetime(&day.and_time(NaiveTime::from_hms_opt(12, 0, 0).unwrap())).unwrap().to_rfc3339()
    }

    fn record(date: &str, duration: f64, text: &str, tokens: Option<u64>) -> Record {
        Record {
            id: format!("rec-{date}"),
            created_at: at(date),
            duration,
            raw_text: Some(text.into()),
            optimize_tokens: tokens,
            ..Record::default()
        }
    }

    fn day(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn 近七天_逐日补零_今天单列() {
        let records = vec![
            record("2026-11-21", 3.5, "今天 开会", Some(40)),
            record("2026-11-21", 1.25, "hello world", None),
            record("2026-11-19", 10.0, "前天", Some(100)),
            record("2026-11-10", 99.0, "范围外", Some(999)),
        ];
        let stats = summarize(&records, StatsRange::Days7, day("2026-11-21"));
        assert_eq!(stats.range, "7d");
        assert_eq!(stats.from.as_deref(), Some("2026-11-15"));
        assert_eq!(stats.to, "2026-11-21");
        assert_eq!(stats.days.len(), 7, "七天每天一条，没记录的为 0");
        assert_eq!(stats.days[0].date, "2026-11-15");
        assert_eq!(stats.days[0].usage, UsageTotals::default());
        assert_eq!(stats.days[4].usage, UsageTotals { count: 1, duration: 10.0, chars: 2, tokens: 100 });
        // 字数不算空白：「今天开会」4 + 「helloworld」10
        assert_eq!(stats.today, UsageTotals { count: 2, duration: 4.75, chars: 14, tokens: 40 });
        assert_eq!(stats.total, UsageTotals { count: 3, duration: 14.75, chars: 16, tokens: 140 });
    }

    #[test]
    fn 全部_从最早一天开始_旧记录没有token按0计() {
        let mut old = record("2026-11-18", 2.0, "旧", None);
        old.raw_text = None;
        let records = vec![old, record("2026-11-20", 1.0, "新", Some(7))];
        let stats = summarize(&records, StatsRange::All, day("2026-11-21"));
        assert_eq!(stats.from.as_deref(), Some("2026-11-18"));
        assert_eq!(stats.days.len(), 4);
        assert_eq!(stats.total, UsageTotals { count: 2, duration: 3.0, chars: 1, tokens: 7 });
        assert_eq!(stats.today, UsageTotals::default());
    }

    #[test]
    fn 没有记录_全部范围为空_时间读不出来和未来的不计() {
        let empty: Vec<Record> = Vec::new();
        let stats = summarize(&empty, StatsRange::All, day("2026-11-21"));
        assert_eq!(stats.from, None);
        assert!(stats.days.is_empty());

        let mut broken = record("2026-11-21", 5.0, "坏时间", Some(1));
        broken.created_at = "not-a-time".into();
        let records = vec![broken, record("2026-11-25", 5.0, "未来", Some(1))];
        let stats = summarize(&records, StatsRange::Days30, day("2026-11-21"));
        assert_eq!(stats.days.len(), 30);
        assert_eq!(stats.total, UsageTotals::default());
    }

    #[test]
    fn 旧版无时区时间计入全部统计() {
        let mut legacy = record("2025-06-01", 2.0, "旧 记录", Some(9));
        legacy.created_at = "2025-06-01T09:30:12.345678".into();
        let stats = summarize(&[legacy], StatsRange::All, day("2025-06-01"));
        assert_eq!(stats.from.as_deref(), Some("2025-06-01"));
        assert_eq!(stats.total, UsageTotals { count: 1, duration: 2.0, chars: 3, tokens: 9 });
        assert_eq!(stats.today, stats.total);
    }

    #[test]
    fn 范围参数解析() {
        assert_eq!(StatsRange::parse("7d"), Some(StatsRange::Days7));
        assert_eq!(StatsRange::parse("30d"), Some(StatsRange::Days30));
        assert_eq!(StatsRange::parse("all"), Some(StatsRange::All));
        assert_eq!(StatsRange::parse("1y"), None);
    }

    #[test]
    fn 响应字段扁平() {
        let stats = summarize(&[record("2026-11-21", 1.0, "字", Some(3))], StatsRange::Days7, day("2026-11-21"));
        let value = serde_json::to_value(&stats).unwrap();
        assert_eq!(value["days"][6]["date"], "2026-11-21");
        assert_eq!(value["days"][6]["tokens"], 3);
        assert_eq!(value["today"]["chars"], 1);
    }
}
