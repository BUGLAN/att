//! 记录域：Record 模型 + JSON 仓库（`data/records/{id}.json`，与 Python 版布局兼容）。
//!
//! 保真点（阶段① §3.9）：
//! - 字段名/取值与 Python `Record.to_dict` 完全一致（前端依赖）
//! - 旧数据缺 `error`/`revision` 字段可读（serde 默认兜底）
//! - revision 乐观锁：PUT 冲突返回 [`RecordError::Conflict`]（HTTP 409）

use crate::storage::dir_index::DirIndex;
use crate::storage::json_store;
use crate::now_rfc3339;
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

/// 转写记录（13 字段，字段名与 Python 版逐一对齐）。
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
    pub processing_time: Option<f64>,
    pub is_modified: bool,
    pub is_favorite: bool,
    pub error: Option<String>,
    pub revision: u64,
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
            is_modified: false,
            is_favorite: false,
            error: None,
            revision: 0,
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
            "is_favorite": self.is_favorite,
        })
    }

    /// 时间排序键：优先解析 RFC3339，失败退化字符串比较（兼容旧格式时间戳）。
    fn sort_key(&self) -> (i64, String) {
        let parsed = chrono::DateTime::parse_from_rfc3339(&self.created_at)
            .map(|t| t.timestamp_micros())
            .unwrap_or(0);
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
    !id.is_empty()
        && id.len() <= 128
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
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
        let mut index = self.guard();
        index.refresh(scan_records);
        index
            .items()
            .iter()
            .filter(|r| status.map(|s| r.status.as_str() == s).unwrap_or(true))
            .skip(offset)
            .take(limit)
            .cloned()
            .collect()
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
        let q = query.to_lowercase();
        let mut index = self.guard();
        index.refresh(scan_records);
        let matched: Vec<&Record> = index
            .items()
            .iter()
            .filter(|r| !favorite_only || r.is_favorite)
            .filter(|r| matches_query(r, &q, search_type))
            .collect();
        let total = matched.len();
        let items = matched.into_iter().skip((page.max(1) - 1) * page_size).take(page_size).cloned().collect();
        (items, total)
    }

    pub fn get(&self, id: &str) -> Result<Record, RecordError> {
        if !is_safe_identifier(id) {
            return Err(RecordError::NotFound);
        }
        let _guard = self.guard();
        json_store::read_json_file::<Record>(&self.record_path(id)).map_err(|_| RecordError::NotFound)
    }

    /// 新建记录（3b 录音链路使用；id 由调用方生成）。
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
        let mut index = self.guard();
        let path = self.record_path(id);
        let mut record: Record =
            json_store::read_json_file(&path).map_err(|_| RecordError::NotFound)?;

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
        let record: Record =
            json_store::read_json_file(&path).map_err(|_| RecordError::NotFound)?;
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

    /// 音频错位修复用（migration 模块）：按 id 直接改写 audio_path 字段。
    pub fn set_audio_path(&self, id: &str, audio_file_name: &str) -> Result<(), RecordError> {
        let mut index = self.guard();
        let path = self.record_path(id);
        let mut record: Record =
            json_store::read_json_file(&path).map_err(|_| RecordError::NotFound)?;
        record.audio_path = audio_file_name.to_string();
        record.revision += 1;
        record.updated_at = now_rfc3339();
        self.write_locked(&mut index, &record)
    }
}

/// 生成新记录 id（时间前缀 + 随机后缀，无 uuid 依赖；3b 起使用）。
pub fn new_record_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let millis = SystemTime::now().duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    // 简单 xorshift：避免引入 uuid 依赖，id 只需唯一即可
    let mut state = millis as u64 ^ 0x9E3779B97F4A7C15;
    let mut rand = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let (r1, r2) = (rand(), rand());
    format!("rec-{millis:x}-{r1:08x}{r2:08x}")
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
        Record {
            id: id.into(),
            raw_text: Some(text.into()),
            ..Record::default()
        }
    }

    #[test]
    fn 增查改删_与乐观锁() {
        let (repo, dir) = repo("crud");
        repo.insert(sample("rec-1", "你好世界")).unwrap();

        let got = repo.get("rec-1").unwrap();
        assert_eq!(got.raw_text.as_deref(), Some("你好世界"));
        assert_eq!(got.revision, 0);

        // 更新（带正确 revision）
        let updated = repo.update("rec-1", RecordUpdate {
            optimized_text: Some("你好，世界".into()),
            is_favorite: Some(true),
            revision: Some(0),
            ..Default::default()
        }).unwrap();
        assert_eq!(updated.revision, 1);
        assert!(updated.is_favorite);

        // 过期 revision → 冲突
        let conflict = repo.update("rec-1", RecordUpdate {
            raw_text: Some("x".into()),
            revision: Some(0),
            ..Default::default()
        });
        assert!(matches!(conflict, Err(RecordError::Conflict)));

        // 不带 revision 的更新直接生效
        repo.update("rec-1", RecordUpdate {
            raw_text: Some("新原文".into()),
            ..Default::default()
        }).unwrap();

        assert!(matches!(repo.get("missing"), Err(RecordError::NotFound)));
        repo.delete("rec-1").unwrap();
        assert!(matches!(repo.get("rec-1"), Err(RecordError::NotFound)));
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
        assert!(record.error.is_none());
        assert_eq!(record.revision, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 回归锁：历史 audio_path 三种形态必须归一化到 audio_dir 下的同一文件
    /// （旧版直接 join 导致所有旧音频 404、迁移误判未认领）。
    #[test]
    fn 音频路径归一化_历史前缀形态可解析() {
        assert_eq!(normalize_audio_rel("uuid-x.wav"), "uuid-x.wav");
        assert_eq!(
            normalize_audio_rel("data\\audio\\records\\uuid-x.wav"),
            "uuid-x.wav",
            "Python 版相对仓库根形态"
        );
        assert_eq!(
            normalize_audio_rel("data/audio/records/uuid-x.wav"),
            "uuid-x.wav"
        );
        assert_eq!(
            normalize_audio_rel("/app/data/audio/records/uuid-x.wav"),
            "uuid-x.wav",
            "docker 部署时代形态"
        );
        assert_eq!(normalize_audio_rel("test/path.wav"), "path.wav");

        // 端到端：带前缀 audio_path 的记录能拿到音频文件
        let (repo, dir) = repo("audio-path");
        repo.ensure_dirs().unwrap();
        std::fs::write(repo.audio_dir().join("uuid-y.wav"), b"RIFF").unwrap();
        repo.insert(Record {
            id: "rec-y".into(),
            audio_path: "data\\audio\\records\\uuid-y.wav".into(),
            ..Default::default()
        }).unwrap();
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
