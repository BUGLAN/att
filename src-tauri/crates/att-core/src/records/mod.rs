//! 记录域：Record 模型 + JSON 仓库（`data/records/{id}.json`，与 Python 版布局兼容）。
//!
//! 保真点（与旧 Python 版数据格式一致）：
//! - 字段名/取值与 Python `Record.to_dict` 完全一致（前端依赖）
//! - 旧数据缺 `error`/`revision` 字段可读（serde 默认兜底）
//! - revision 乐观锁：PUT 冲突返回 [`RecordError::Conflict`]（HTTP 409）
//!
//! 「重新识别」（读记录音频 → 选引擎整段识别 → 写回）见 [`retranscribe`]；批量导出的文件格式见 [`export`]；
//! 按天汇总的用量统计见 [`stats`]。

pub mod export;
pub mod retranscribe;
pub mod stats;

use crate::now_rfc3339;
use crate::storage::dir_index::DirIndex;
use crate::storage::json_store;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// 记录状态机（与 Python app/records/models.py 一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordStatus {
    Pending,
    Transcribing,
    Transcribed,
    Optimizing,
    Optimized,
    Failed,
}

impl RecordStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            RecordStatus::Pending => "pending",
            RecordStatus::Transcribing => "transcribing",
            RecordStatus::Transcribed => "transcribed",
            RecordStatus::Optimizing => "optimizing",
            RecordStatus::Optimized => "optimized",
            RecordStatus::Failed => "failed",
        }
    }
}

/// 转写记录（字段名与 Python 版逐一对齐；`optimize_time` 为可选扩展，旧数据缺字段按 None 读）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub audio_path: String,
    pub duration: f64,
    pub raw_text: Option<String>,
    pub optimized_text: Option<String>,
    pub status: RecordStatus,
    /// 停止录音 → 识别结果落地；之后的优化不再改动这个值
    pub processing_time: Option<f64>,
    /// 优化耗时，与识别阶段的 `processing_time` 分开记录
    #[serde(default)]
    pub optimize_time: Option<f64>,
    pub is_modified: bool,
    pub is_favorite: bool,
    pub error: Option<String>,
    pub revision: u64,
    /// 识别引擎（provider 标识，如 `doubao_asr`、`whisper`）；旧记录与文本记录没有这个字段
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    /// 识别语言（录音时的设置 `asr_language`，如 `zh`）；旧记录与文本记录没有这个字段
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// 「转文字」页转写的原文件名（不含目录）；录音与旧记录没有这个字段
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_file: Option<String>,
    /// 这条记录的 LLM 优化累计用掉的 token（每次优化成功都累加，重新识别不清零：用掉的就是用掉了）；
    /// 旧记录、没优化过或服务商没报用量时没有这个字段，统计按 0 计
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optimize_tokens: Option<u64>,
}

impl Default for Record {
    fn default() -> Self {
        Self {
            id: String::new(),
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
            audio_path: String::new(),
            duration: 0.0,
            raw_text: None,
            optimized_text: None,
            status: RecordStatus::Pending,
            processing_time: None,
            optimize_time: None,
            is_modified: false,
            is_favorite: false,
            error: None,
            revision: 0,
            engine: None,
            language: None,
            source_file: None,
            optimize_tokens: None,
        }
    }
}

impl Record {
    /// GET /api/records/{id}/status 的最小载荷（前端轮询契约）。
    pub fn status_view(&self) -> serde_json::Value {
        serde_json::json!({
            "record_id": self.id,
            "status": self.status.as_str(),
            "raw_text": self.raw_text,
            "optimized_text": self.optimized_text,
            "duration": self.duration,
            "processing_time": self.processing_time,
            "optimize_time": self.optimize_time,
            "is_favorite": self.is_favorite,
        })
    }

    /// 时间排序键：优先解析 RFC3339，失败退化字符串比较（兼容旧格式时间戳）。
    fn sort_key(&self) -> (i64, String) {
        let parsed = chrono::DateTime::parse_from_rfc3339(&self.created_at).map(|t| t.timestamp_micros()).unwrap_or(0);
        (parsed, self.created_at.clone())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("记录不存在")]
    NotFound,
    #[error("记录已被其他端修改，请刷新后重试")]
    Conflict,
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
}

/// 非法标识符防护：只允许字母/数字/`-`/`_`（对应 Python is_safe_identifier）。
pub fn is_safe_identifier(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 归一化历史 audio_path 为「相对 audio_dir 的扁平文件名」。
///
/// 规则：取最后一个 `records/` 段之后的内容；没有该段则取末段文件名。
/// 归一化结果天然不含目录分隔符，无路径穿越风险（配合调用侧 starts_with 校验双保险）。
pub fn normalize_audio_rel(raw: &str) -> String {
    let normalized = raw.trim().replace('\\', "/");
    let trimmed = normalized.trim_start_matches("./");
    if let Some(pos) = trimmed.rfind("records/") {
        return trimmed[pos + "records/".len()..].to_string();
    }
    trimmed.rsplit('/').next().unwrap_or(trimmed).to_string()
}

/// 记录仓库：每记录一个 JSON 文件；目录级互斥串行化读-改-写。
///
/// 列表 / 搜索走内存索引（[`DirIndex`]）：整目录只在首次读取或目录有外部变化时扫描一次，
/// 本仓库的写入在同一把锁下同步更新索引。单条读取（`get` 与读-改-写）仍以文件为准。
pub struct RecordRepo {
    records_dir: PathBuf,
    audio_dir: PathBuf,
    index: Mutex<DirIndex<Record>>,
}

/// PUT /api/records/{id} 的更新载荷。
#[derive(Debug, Default)]
pub struct RecordUpdate {
    pub raw_text: Option<String>,
    pub optimized_text: Option<String>,
    pub is_modified: Option<bool>,
    pub is_favorite: Option<bool>,
    /// 显式携带的版本号（None = 不校验）。
    pub revision: Option<u64>,
}

/// 按 created_at 倒序排列（新的在前）。
fn sort_records(records: &mut [Record]) {
    records.sort_by_cached_key(|r| std::cmp::Reverse(r.sort_key()));
}

/// 写入成功后把这条记录同步进索引：已有则替换（created_at 变了才重排），没有则插入后重排。
fn upsert(records: &mut Vec<Record>, record: &Record) {
    match records.iter().position(|r| r.id == record.id) {
        Some(index) if records[index].created_at == record.created_at => records[index] = record.clone(),
        Some(index) => {
            records[index] = record.clone();
            sort_records(records);
        }
        None => {
            records.push(record.clone());
            sort_records(records);
        }
    }
}

/// 扫描记录目录（损坏条目跳过并计数——绝不因单条坏数据让列表 500）。
fn scan_records(dir: &Path) -> (Vec<Record>, usize) {
    let mut records = Vec::new();
    let mut skipped = 0usize;
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return (records, skipped),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match json_store::read_json_file::<Record>(&path) {
            Ok(record) if is_safe_identifier(&record.id) => records.push(record),
            Ok(_) => {
                skipped += 1;
                tracing::warn!("记录 id 非法，跳过: {}", path.display());
            }
            Err(error) => {
                skipped += 1;
                tracing::error!("记录损坏，跳过: {} ({error})", path.display());
            }
        }
    }
    sort_records(&mut records);
    (records, skipped)
}

/// 记录集的筛选条件（搜索、收藏、状态、本地日期范围），列表分页与批量导出共用。
#[derive(Debug, Clone, Default)]
pub struct RecordFilter {
    /// 关键词（不区分大小写；空 = 不限）
    pub query: String,
    /// 搜索范围：`all` / `raw` / `optimized`
    pub search_type: String,
    pub favorite_only: bool,
    /// 只要这个状态（`transcribed` / `optimized` / `failed` …）；None = 不限
    pub status: Option<String>,
    /// 创建日期范围（本机时区的日期，含两端）；None = 不限
    pub date_from: Option<chrono::NaiveDate>,
    pub date_to: Option<chrono::NaiveDate>,
    /// 只要「转文字」页文件转写的记录（带 `source_file`）
    pub file_only: bool,
}

impl RecordFilter {
    fn matches(&self, record: &Record, q: &str) -> bool {
        if self.favorite_only && !record.is_favorite {
            return false;
        }
        if self.status.as_deref().is_some_and(|status| record.status.as_str() != status) {
            return false;
        }
        if self.file_only && record.source_file.is_none() {
            return false;
        }
        if self.date_from.is_some() || self.date_to.is_some() {
            // 创建时间读不出来的记录不落在任何日期范围里
            let Some(day) = local_date(&record.created_at) else { return false };
            if self.date_from.is_some_and(|from| day < from) || self.date_to.is_some_and(|to| day > to) {
                return false;
            }
        }
        matches_query(record, q, &self.search_type)
    }
}

/// 记录创建时间对应的本机日期（按日期筛选用；时间读不出来时为 None）。
pub fn local_date(created_at: &str) -> Option<chrono::NaiveDate> {
    chrono::DateTime::parse_from_rfc3339(created_at).ok().map(|time| time.with_timezone(&chrono::Local).date_naive())
}

/// 搜索命中判定（`q` 已转小写；空串视为全部命中）。
fn matches_query(record: &Record, q: &str, search_type: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    let hit = |text: &Option<String>| text.as_deref().map(|t| t.to_lowercase().contains(q)).unwrap_or(false);
    match search_type {
        "raw" => hit(&record.raw_text),
        "optimized" => hit(&record.optimized_text),
        _ => hit(&record.raw_text) || hit(&record.optimized_text),
    }
}

impl RecordRepo {
    pub fn new(data_dir: &Path) -> Self {
        let records_dir = data_dir.join("records");
        Self {
            index: Mutex::new(DirIndex::new(records_dir.clone())),
            records_dir,
            audio_dir: data_dir.join("audio").join("records"),
        }
    }

    pub fn records_dir(&self) -> &Path {
        &self.records_dir
    }

    pub fn audio_dir(&self) -> &Path {
        &self.audio_dir
    }

    /// 把记录里的 audio_path 解析为绝对路径（历史数据兼容）。
    ///
    /// 旧数据三种形态（见仓库 915 条记录实测分布）：
    /// - `data\audio\records\<uuid>.wav`（Python 版按「相对仓库根」落盘）
    /// - `/app/data/audio/records/<uuid>.wav`（docker 部署时代）
    /// - `<uuid>.wav`（迁移后的规范形态）
    ///
    /// 直接 `audio_dir.join(raw)` 会拼出 `...\records\data\audio\records\x.wav`
    /// 导致所有旧音频 404、删除清理失效、迁移误判未认领（已发生过 886 个 wav
    /// 被整体隔离进 orphans/ 的事故）。这里统一归一化为相对 audio_dir 的文件名。
    pub fn resolve_audio_path(&self, audio_path: &str) -> PathBuf {
        self.audio_dir.join(normalize_audio_rel(audio_path))
    }

    fn record_path(&self, id: &str) -> PathBuf {
        self.records_dir.join(format!("{id}.json"))
    }

    fn guard(&self) -> MutexGuard<'_, DirIndex<Record>> {
        self.index.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 锁内写一条记录并同步索引（写入前先核对目录是否有外部变化）。
    fn write_locked(&self, index: &mut DirIndex<Record>, record: &Record) -> Result<(), RecordError> {
        if !is_safe_identifier(&record.id) {
            return Err(RecordError::NotFound);
        }
        index.before_write();
        json_store::write_json_atomic(&self.record_path(&record.id), record)?;
        index.after_write(|records| upsert(records, record));
        Ok(())
    }

    /// 确保目录存在（首次启动）。
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.records_dir)?;
        std::fs::create_dir_all(&self.audio_dir)?;
        Ok(())
    }

    /// 读取全部记录（按 created_at 倒序；第二项为跳过的损坏条目数）。
    pub fn load_all(&self) -> (Vec<Record>, usize) {
        let mut index = self.guard();
        index.refresh(scan_records);
        (index.items().to_vec(), index.skipped())
    }

    /// GET /api/records：分页列表（裸数组响应 + 按 created_at 倒序）。
    pub fn list(&self, status: Option<&str>, limit: usize, offset: usize) -> Vec<Record> {
        self.list_page(status, limit, offset).0
    }

    /// GET /api/records?with_total=true：分页列表 + 过滤后的总数（单次加锁/扫描）。
    pub fn list_page(&self, status: Option<&str>, limit: usize, offset: usize) -> (Vec<Record>, usize) {
        let mut index = self.guard();
        index.refresh(scan_records);
        let matched: Vec<&Record> =
            index.items().iter().filter(|r| status.map(|s| r.status.as_str() == s).unwrap_or(true)).collect();
        let total = matched.len();
        let items = matched.into_iter().skip(offset).take(limit).cloned().collect();
        (items, total)
    }

    /// GET /api/records/search：文本 + 收藏搜索。
    pub fn search(
        &self,
        query: &str,
        search_type: &str,
        favorite_only: bool,
        page: usize,
        page_size: usize,
    ) -> (Vec<Record>, usize) {
        let filter = RecordFilter {
            query: query.to_string(),
            search_type: search_type.to_string(),
            favorite_only,
            ..Default::default()
        };
        self.search_filtered(&filter, page, page_size)
    }

    /// GET /api/records/search（带状态 / 日期筛选）：按 created_at 倒序分页，第二项是筛选后的总数。
    pub fn search_filtered(&self, filter: &RecordFilter, page: usize, page_size: usize) -> (Vec<Record>, usize) {
        let q = filter.query.to_lowercase();
        let mut index = self.guard();
        index.refresh(scan_records);
        let matched: Vec<&Record> = index.items().iter().filter(|r| filter.matches(r, &q)).collect();
        let total = matched.len();
        let items = matched.into_iter().skip((page.max(1) - 1) * page_size).take(page_size).cloned().collect();
        (items, total)
    }

    /// 筛选结果的全部记录（批量导出「当前结果」用，不分页；按 created_at 倒序）。
    pub fn filtered(&self, filter: &RecordFilter) -> Vec<Record> {
        let q = filter.query.to_lowercase();
        let mut index = self.guard();
        index.refresh(scan_records);
        index.items().iter().filter(|r| filter.matches(r, &q)).cloned().collect()
    }

    /// 按 id 取多条记录（批量导出「选中」用；不存在的跳过，按 created_at 倒序）。
    pub fn get_many(&self, ids: &[String]) -> Vec<Record> {
        let wanted: std::collections::HashSet<&str> = ids.iter().map(String::as_str).collect();
        let mut index = self.guard();
        index.refresh(scan_records);
        index.items().iter().filter(|r| wanted.contains(r.id.as_str())).cloned().collect()
    }

    pub fn get(&self, id: &str) -> Result<Record, RecordError> {
        if !is_safe_identifier(id) {
            return Err(RecordError::NotFound);
        }
        let _guard = self.guard();
        json_store::read_json_file::<Record>(&self.record_path(id)).map_err(|_| RecordError::NotFound)
    }

    /// 新建记录（录音链路使用；id 由调用方生成）。
    pub fn insert(&self, mut record: Record) -> Result<(), RecordError> {
        record.revision = 0;
        record.created_at = now_rfc3339();
        record.updated_at = record.created_at.clone();
        let mut index = self.guard();
        std::fs::create_dir_all(&self.records_dir)?;
        self.write_locked(&mut index, &record)
    }

    /// 保存（内部写，已持有锁或新建场景）。
    pub fn save(&self, record: &Record) -> Result<(), RecordError> {
        let mut index = self.guard();
        self.write_locked(&mut index, record)
    }

    /// PUT 更新：revision 乐观锁 + 字段应用 + revision 自增。
    pub fn update(&self, id: &str, update: RecordUpdate) -> Result<Record, RecordError> {
        if !is_safe_identifier(id) {
            return Err(RecordError::NotFound);
        }
        let mut index = self.guard();
        let path = self.record_path(id);
        let mut record: Record = json_store::read_json_file(&path).map_err(|_| RecordError::NotFound)?;

        if let Some(expected) = update.revision {
            if expected != record.revision {
                return Err(RecordError::Conflict);
            }
        }
        if let Some(raw) = update.raw_text {
            record.raw_text = Some(raw);
        }
        if let Some(opt) = update.optimized_text {
            record.optimized_text = Some(opt);
        }
        if let Some(modified) = update.is_modified {
            record.is_modified = modified;
        }
        if let Some(favorite) = update.is_favorite {
            record.is_favorite = favorite;
        }
        record.revision += 1;
        record.updated_at = now_rfc3339();
        self.write_locked(&mut index, &record)?;
        Ok(record)
    }

    /// 内部更新（状态机推进 / 优化结果落盘）：锁内读-改-写，不做乐观锁校验，revision 照常自增
    /// （与 Python `update_record` 一致：任何写入都让前端持有的旧 revision 失效）。
    pub fn patch(&self, id: &str, mutate: impl FnOnce(&mut Record)) -> Result<Record, RecordError> {
        if !is_safe_identifier(id) {
            return Err(RecordError::NotFound);
        }
        let mut index = self.guard();
        let path = self.record_path(id);
        let mut record: Record = json_store::read_json_file(&path).map_err(|_| RecordError::NotFound)?;
        mutate(&mut record);
        record.revision += 1;
        record.updated_at = now_rfc3339();
        self.write_locked(&mut index, &record)?;
        Ok(record)
    }

    /// 删除记录 + 对应音频（音频路径必须落在 audio_dir 内，防路径穿越）。
    pub fn delete(&self, id: &str) -> Result<(), RecordError> {
        if !is_safe_identifier(id) {
            return Err(RecordError::NotFound);
        }
        let mut index = self.guard();
        let path = self.record_path(id);
        let record: Record = json_store::read_json_file(&path).map_err(|_| RecordError::NotFound)?;
        if !record.audio_path.is_empty() {
            let audio = self.resolve_audio_path(&record.audio_path);
            if audio.starts_with(&self.audio_dir) && audio.is_file() {
                let _ = std::fs::remove_file(&audio);
            }
        }
        index.before_write();
        std::fs::remove_file(&path).map_err(|_| RecordError::NotFound)?;
        index.after_write(|records| records.retain(|r| r.id != id));
        Ok(())
    }

    /// 音频文件绝对路径（供播放端点；None = 无音频或文件不存在）。
    pub fn audio_file(&self, id: &str) -> Result<Option<PathBuf>, RecordError> {
        let record = self.get(id)?;
        if record.audio_path.is_empty() {
            return Ok(None);
        }
        let path = self.resolve_audio_path(&record.audio_path);
        if path.starts_with(&self.audio_dir) && path.is_file() {
            Ok(Some(path))
        } else {
            Ok(None)
        }
    }

    /// 录音音频保留期：删除 `days` 天前（按创建时间）建的非收藏记录的音频文件，记录与文字保留（`audio_path` 置空）。
    /// 音频文件本来就不在的同样把 `audio_path` 置空；创建时间读不出来的不动。返回删掉的音频文件数。
    pub fn prune_audio(&self, days: i64, now: chrono::DateTime<chrono::Utc>) -> Result<usize, RecordError> {
        if days <= 0 {
            return Ok(0);
        }
        let cutoff = now - chrono::Duration::days(days);
        let expired = |record: &Record| {
            !record.is_favorite
                && !record.audio_path.is_empty()
                && chrono::DateTime::parse_from_rfc3339(&record.created_at).is_ok_and(|created| created < cutoff)
        };
        let mut index = self.guard();
        index.refresh(scan_records);
        let ids: Vec<String> = index.items().iter().filter(|record| expired(record)).map(|r| r.id.clone()).collect();
        let mut removed = 0;
        for id in ids {
            // 锁内按文件再核对一次（索引可能比文件旧）
            let Ok(mut record) = json_store::read_json_file::<Record>(&self.record_path(&id)) else { continue };
            if !expired(&record) {
                continue;
            }
            let audio = self.resolve_audio_path(&record.audio_path);
            if audio.starts_with(&self.audio_dir) && audio.is_file() {
                if let Err(error) = std::fs::remove_file(&audio) {
                    tracing::warn!("按保留期删除录音失败 {}: {error}", audio.display());
                    continue;
                }
                removed += 1;
            }
            record.audio_path = String::new();
            record.revision += 1;
            record.updated_at = now_rfc3339();
            self.write_locked(&mut index, &record)?;
        }
        Ok(removed)
    }

    /// GET /api/stats：按本机日期汇总用量（走内存索引，见 [`stats`]）。
    pub fn usage_stats(&self, range: stats::StatsRange, today: chrono::NaiveDate) -> stats::UsageStats {
        let mut index = self.guard();
        index.refresh(scan_records);
        stats::summarize(index.items(), range, today)
    }

    /// 列出处于「识别中」的记录 id（启动时恢复被中断的重新识别用）。
    pub fn transcribing_ids(&self) -> Vec<String> {
        let mut index = self.guard();
        index.refresh(scan_records);
        index.items().iter().filter(|r| r.status == RecordStatus::Transcribing).map(|r| r.id.clone()).collect()
    }

    /// 音频错位修复用（migration 模块）：按 id 直接改写 audio_path 字段。
    pub fn set_audio_path(&self, id: &str, audio_file_name: &str) -> Result<(), RecordError> {
        if !is_safe_identifier(id) {
            return Err(RecordError::NotFound);
        }
        let mut index = self.guard();
        let path = self.record_path(id);
        let mut record: Record = json_store::read_json_file(&path).map_err(|_| RecordError::NotFound)?;
        record.audio_path = audio_file_name.to_string();
        record.revision += 1;
        record.updated_at = now_rfc3339();
        self.write_locked(&mut index, &record)
    }
}

/// 生成新记录 id（UUID v4，保持文件名安全且不依赖时间精度）。
pub fn new_record_id() -> String {
    format!("rec-{}", uuid::Uuid::new_v4().simple())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(tag: &str) -> (RecordRepo, PathBuf) {
        let dir = std::env::temp_dir().join(format!("att-records-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = RecordRepo::new(&dir);
        repo.ensure_dirs().unwrap();
        (repo, dir)
    }

    fn sample(id: &str, text: &str) -> Record {
        Record { id: id.into(), raw_text: Some(text.into()), ..Record::default() }
    }

    #[test]
    fn 增查改删_与乐观锁() {
        let (repo, dir) = repo("crud");
        repo.insert(sample("rec-1", "你好世界")).unwrap();

        let got = repo.get("rec-1").unwrap();
        assert_eq!(got.raw_text.as_deref(), Some("你好世界"));
        assert_eq!(got.revision, 0);

        // 更新（带正确 revision）
        let updated = repo
            .update(
                "rec-1",
                RecordUpdate {
                    optimized_text: Some("你好，世界".into()),
                    is_favorite: Some(true),
                    revision: Some(0),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(updated.revision, 1);
        assert!(updated.is_favorite);

        // 过期 revision → 冲突
        let conflict =
            repo.update("rec-1", RecordUpdate { raw_text: Some("x".into()), revision: Some(0), ..Default::default() });
        assert!(matches!(conflict, Err(RecordError::Conflict)));

        // 不带 revision 的更新直接生效
        repo.update("rec-1", RecordUpdate { raw_text: Some("新原文".into()), ..Default::default() }).unwrap();

        assert!(matches!(repo.get("missing"), Err(RecordError::NotFound)));
        repo.delete("rec-1").unwrap();
        assert!(matches!(repo.get("rec-1"), Err(RecordError::NotFound)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 新记录id_同一毫秒内批量生成不重复() {
        let ids: std::collections::HashSet<String> = (0..1_000).map(|_| new_record_id()).collect();
        assert_eq!(ids.len(), 1_000);
        assert!(ids.iter().all(|id| {
            id.strip_prefix("rec-")
                .and_then(|hex| uuid::Uuid::parse_str(hex).ok().filter(|uuid| uuid.get_version_num() == 4))
                .is_some()
                && is_safe_identifier(id)
        }));
    }

    #[test]
    fn 写入口拒绝非法id且不改写记录目录外的json() {
        let (repo, dir) = repo("unsafe-id");
        let settings_path = dir.join("settings.json");
        let original = br#"{"theme":"dark","revision":7}"#;
        std::fs::write(&settings_path, original).unwrap();

        for id in ["../settings", r"..\..\settings", "nested/settings"] {
            let result = repo.update(id, RecordUpdate { raw_text: Some("被穿越写入".into()), ..Default::default() });
            assert!(matches!(result, Err(RecordError::NotFound)), "update 必须拒绝非法 id: {id}");
            assert!(
                matches!(repo.insert(Record { id: id.into(), ..Default::default() }), Err(RecordError::NotFound)),
                "insert 必须拒绝非法 id: {id}"
            );
            assert!(
                matches!(repo.save(&Record { id: id.into(), ..Default::default() }), Err(RecordError::NotFound)),
                "save 必须拒绝非法 id: {id}"
            );
        }

        let absolute = settings_path.to_string_lossy();
        let result =
            repo.update(&absolute, RecordUpdate { raw_text: Some("被绝对路径写入".into()), ..Default::default() });
        assert!(matches!(result, Err(RecordError::NotFound)), "update 必须拒绝绝对路径 id");

        assert!(matches!(repo.set_audio_path("../settings", "replacement.wav"), Err(RecordError::NotFound)));
        assert_eq!(std::fs::read(&settings_path).unwrap(), original, "记录目录外的 settings.json 字节不得变化");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 旧数据_缺error与revision字段可读() {
        let (repo, dir) = repo("legacy");
        std::fs::create_dir_all(repo.records_dir()).unwrap();
        // 模拟 Python 早期版本的记录（无 error / revision / is_favorite）
        std::fs::write(
            repo.records_dir().join("rec-old.json"),
            r#"{"id":"rec-old","created_at":"2026-10-01T00:00:00Z","updated_at":"2026-10-01T00:00:00Z","audio_path":"rec-old.wav","duration":3.2,"raw_text":"旧文本","optimized_text":null,"status":"transcribed","processing_time":1.1}"#,
        ).unwrap();
        let record = repo.get("rec-old").unwrap();
        assert_eq!(record.raw_text.as_deref(), Some("旧文本"));
        assert_eq!(record.processing_time, Some(1.1));
        assert_eq!(record.optimize_time, None, "旧记录没有 optimize_time 时按 None 读");
        assert!(record.error.is_none());
        assert_eq!(record.revision, 0);
        assert_eq!(record.engine, None, "旧记录没有识别引擎字段");
        assert_eq!(record.language, None, "旧记录没有识别语言字段");
        assert_eq!(record.source_file, None, "旧记录没有来源文件字段");
        assert_eq!(record.optimize_tokens, None, "旧记录没有优化 token 字段");
        assert!(serde_json::to_value(&record).unwrap().get("optimize_tokens").is_none(), "没有时不写出");
        // 没有来源文件时不写出这个字段（旧版本读得回来）
        assert!(serde_json::to_value(&record).unwrap().get("source_file").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 筛选_只要文件转写的记录() {
        let (repo, dir) = repo("file-only");
        repo.insert(sample("rec-voice", "口述")).unwrap();
        repo.insert(Record { source_file: Some("会议.mp4".into()), ..sample("rec-file", "文件") }).unwrap();
        let filter = RecordFilter { file_only: true, ..Default::default() };
        let (items, total) = repo.search_filtered(&filter, 1, 20);
        assert_eq!(total, 1);
        assert_eq!(items[0].id, "rec-file");
        assert_eq!(items[0].source_file.as_deref(), Some("会议.mp4"));
        assert_eq!(repo.search_filtered(&RecordFilter::default(), 1, 20).1, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 音频保留期_只删过期非收藏记录的音频_文字保留() {
        let (repo, dir) = repo("audio-retention");
        let now = chrono::Utc::now();
        let put = |id: &str, days_ago: i64, favorite: bool| {
            let name = format!("{id}.flac");
            std::fs::write(repo.audio_dir().join(&name), b"fLaC").unwrap();
            repo.insert(Record {
                id: id.into(),
                audio_path: name,
                raw_text: Some(format!("{id} 的文字")),
                is_favorite: favorite,
                ..Default::default()
            })
            .unwrap();
            let created = (now - chrono::Duration::days(days_ago)).to_rfc3339();
            repo.patch(id, |r| r.created_at = created.clone()).unwrap();
        };
        put("rec-old", 2, false);
        put("rec-old-fav", 2, true);
        put("rec-new", 0, false);

        assert_eq!(repo.prune_audio(0, now).unwrap(), 0, "0 = 永久保留，不删任何文件");
        assert!(repo.audio_dir().join("rec-old.flac").exists());

        assert_eq!(repo.prune_audio(1, now).unwrap(), 1);
        let old = repo.get("rec-old").unwrap();
        assert_eq!(old.audio_path, "", "音频删掉后 audio_path 置空");
        assert_eq!(old.raw_text.as_deref(), Some("rec-old 的文字"), "文字保留");
        assert!(!repo.audio_dir().join("rec-old.flac").exists());
        assert_eq!(repo.get("rec-old-fav").unwrap().audio_path, "rec-old-fav.flac", "收藏的不删");
        assert!(repo.audio_dir().join("rec-old-fav.flac").exists());
        assert_eq!(repo.get("rec-new").unwrap().audio_path, "rec-new.flac", "没过期的不删");
        assert_eq!(repo.prune_audio(1, now).unwrap(), 0, "再跑一次没有可删的");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 识别引擎与语言_有值才写入json() {
        let plain = serde_json::to_value(sample("rec-x", "文本")).unwrap();
        assert!(plain.get("engine").is_none() && plain.get("language").is_none(), "没有值时不写这两个字段");

        let record =
            Record { engine: Some("whisper".into()), language: Some("zh".into()), ..sample("rec-y", "文本") };
        let value = serde_json::to_value(&record).unwrap();
        assert_eq!(value["engine"], "whisper");
        assert_eq!(value["language"], "zh");
        let back: Record = serde_json::from_value(value).unwrap();
        assert_eq!(back.engine.as_deref(), Some("whisper"));
        assert_eq!(back.language.as_deref(), Some("zh"));
    }

    /// 回归锁：历史 audio_path 三种形态必须归一化到 audio_dir 下的同一文件
    /// （旧版直接 join 导致所有旧音频 404、迁移误判未认领）。
    #[test]
    fn 音频路径归一化_历史前缀形态可解析() {
        assert_eq!(normalize_audio_rel("uuid-x.wav"), "uuid-x.wav");
        assert_eq!(normalize_audio_rel("data\\audio\\records\\uuid-x.wav"), "uuid-x.wav", "Python 版相对仓库根形态");
        assert_eq!(normalize_audio_rel("data/audio/records/uuid-x.wav"), "uuid-x.wav");
        assert_eq!(normalize_audio_rel("/app/data/audio/records/uuid-x.wav"), "uuid-x.wav", "docker 部署时代形态");
        assert_eq!(normalize_audio_rel("test/path.wav"), "path.wav");

        // 端到端：带前缀 audio_path 的记录能拿到音频文件
        let (repo, dir) = repo("audio-path");
        repo.ensure_dirs().unwrap();
        std::fs::write(repo.audio_dir().join("uuid-y.wav"), b"RIFF").unwrap();
        repo.insert(Record {
            id: "rec-y".into(),
            audio_path: "data\\audio\\records\\uuid-y.wav".into(),
            ..Default::default()
        })
        .unwrap();
        let audio = repo.audio_file("rec-y").unwrap().expect("带前缀路径必须解析到文件");
        assert!(audio.ends_with("uuid-y.wav"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 列表_按时间倒序_损坏条目跳过() {
        let (repo, dir) = repo("list");
        for (id, created) in [("rec-a", "2026-10-01T00:00:00Z"), ("rec-b", "2026-10-02T00:00:00Z")] {
            let mut record = sample(id, id);
            record.created_at = created.into();
            record.updated_at = created.into();
            repo.insert(record).unwrap();
        }
        std::fs::write(repo.records_dir().join("broken.json"), "{ 坏数据").unwrap();

        let (records, skipped) = repo.load_all();
        assert_eq!(records.len(), 2);
        assert_eq!(skipped, 1);
        assert_eq!(records[0].id, "rec-b", "按 created_at 倒序");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 搜索_范围与收藏过滤() {
        let (repo, dir) = repo("search");
        let mut a = sample("rec-a", "会议记录原文");
        a.optimized_text = Some("会议纪要".to_string());
        let mut b = sample("rec-b", "无关内容");
        b.is_favorite = true;
        repo.insert(a).unwrap();
        repo.insert(b).unwrap();

        let (items, total) = repo.search("会议", "all", false, 1, 100);
        assert_eq!(total, 1);
        assert_eq!(items[0].id, "rec-a");

        let (_, fav_total) = repo.search("", "all", true, 1, 100);
        assert_eq!(fav_total, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 筛选_状态与本地日期范围_与关键词收藏组合() {
        let (repo, dir) = repo("filter");
        let today = chrono::Local::now();
        let put = |id: &str, days_ago: i64, status: RecordStatus, text: &str| {
            repo.insert(Record { id: id.into(), raw_text: Some(text.into()), status, ..Default::default() }).unwrap();
            let created = (today - chrono::Duration::days(days_ago)).with_timezone(&chrono::Utc).to_rfc3339();
            repo.patch(id, |r| r.created_at = created.clone()).unwrap();
        };
        put("rec-today", 0, RecordStatus::Transcribed, "今天的会议");
        put("rec-week", 5, RecordStatus::Failed, "");
        put("rec-month", 20, RecordStatus::Optimized, "上个月的会议");
        let ids = |filter: &RecordFilter| -> Vec<String> {
            let (items, total) = repo.search_filtered(filter, 1, 100);
            assert_eq!(items.len(), total);
            items.into_iter().map(|r| r.id).collect()
        };
        let day = |n: i64| (today - chrono::Duration::days(n)).date_naive();

        let failed = RecordFilter { status: Some("failed".into()), ..Default::default() };
        assert_eq!(ids(&failed), ["rec-week"], "选「失败」能列出识别失败的记录");
        let last_7 = RecordFilter { date_from: Some(day(6)), date_to: Some(day(0)), ..Default::default() };
        assert_eq!(ids(&last_7), ["rec-today", "rec-week"]);
        let only_today = RecordFilter { date_from: Some(day(0)), date_to: Some(day(0)), ..Default::default() };
        assert_eq!(ids(&only_today), ["rec-today"]);
        let older = RecordFilter { date_to: Some(day(10)), query: "会议".into(), ..Default::default() };
        assert_eq!(ids(&older), ["rec-month"], "日期与关键词组合");
        assert_eq!(repo.filtered(&RecordFilter::default()).len(), 3, "导出当前结果不分页");
        assert_eq!(
            repo.get_many(&["rec-month".into(), "missing".into(), "rec-today".into()])
                .into_iter()
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            ["rec-today", "rec-month"],
            "按 id 取多条：跳过不存在的，按时间倒序"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 列表索引_随增改删同步() {
        let (repo, dir) = repo("index");
        repo.insert(sample("rec-1", "第一条")).unwrap();
        assert_eq!(repo.list(None, 10, 0).len(), 1, "首次读取建立索引");

        repo.insert(sample("rec-2", "第二条")).unwrap();
        let mut ids: Vec<String> = repo.list(None, 10, 0).into_iter().map(|r| r.id).collect();
        ids.sort();
        assert_eq!(ids, ["rec-1", "rec-2"], "新增记录同步进索引");

        repo.update("rec-1", RecordUpdate { is_favorite: Some(true), ..Default::default() }).unwrap();
        let (favorites, total) = repo.search("", "all", true, 1, 10);
        assert_eq!(total, 1);
        assert_eq!(favorites[0].id, "rec-1", "更新后的字段同步进索引");
        assert_eq!(favorites[0].revision, 1);

        repo.patch("rec-2", |r| r.optimized_text = Some("整理后".into())).unwrap();
        assert_eq!(repo.search("整理", "optimized", false, 1, 10).1, 1, "patch 同步进索引");

        repo.delete("rec-2").unwrap();
        let ids: Vec<String> = repo.list(None, 10, 0).into_iter().map(|r| r.id).collect();
        assert_eq!(ids, ["rec-1"], "删除同步移出索引");

        // 另一个仓库实例（如启动迁移）写入：目录变化后本实例重扫可见
        let other = RecordRepo::new(&dir);
        other.insert(sample("rec-3", "另一实例写入")).unwrap();
        assert_eq!(repo.search("另一实例", "all", false, 1, 10).1, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 标识符防护() {
        assert!(is_safe_identifier("rec-abc_123"));
        assert!(!is_safe_identifier("../etc/passwd"));
        assert!(!is_safe_identifier(""));
    }
}
