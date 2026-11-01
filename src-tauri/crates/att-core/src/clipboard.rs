//! 剪贴板历史（移植自 Python `app/clipboard/manager.py`）：`data/clipboard/<id>.json` 一条一文件，
//! 图片落 `data/clipboard/images/<sha256>.png`。文件格式与 Python 版一致（旧数据直接可读）。
//!
//! - 捕获去重：规范化文本（CRLF→LF、trim）的 SHA-256 相同即视为重复，只累加 `duplicate_count`；
//! - 类型识别：链接 / 文件路径 / 代码 / 纯文本（图片由监控方显式指定）；
//! - 图片路径一律解析到 `data/clipboard/images/` 内，越界拒绝（防借图片接口读任意文件）。

use crate::records::is_safe_identifier;
use crate::storage::json_store;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^https?://").expect("正则合法"));
static WINDOWS_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-zA-Z]:[\\/]").expect("正则合法"));
static UNC_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\\\\[^\\]+\\[^\\]+").expect("正则合法"));
static CODE_WORDS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(function|const|let|class|def|import|return)\b").expect("正则合法"));

pub const ITEM_TYPES: [&str; 5] = ["text", "link", "code", "file", "image"];

/// 与 Python `datetime.isoformat()` 一致的本地无时区时间（旧数据两版互读）。
fn now_iso() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.6f").to_string()
}

fn parse_time(value: &str) -> Option<chrono::NaiveDateTime> {
    chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
        .ok()
        .or_else(|| chrono::DateTime::parse_from_rfc3339(value).ok().map(|t| t.with_timezone(&chrono::Local).naive_local()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardItem {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default = "default_type")]
    pub item_type: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default)]
    pub is_favorite: bool,
    #[serde(default)]
    pub is_pinned: bool,
    #[serde(default)]
    pub content_size: i64,
    #[serde(default = "one")]
    pub duplicate_count: i64,
    #[serde(default)]
    pub content_hash: String,
    #[serde(default)]
    pub metadata: Map<String, Value>,
}

fn default_type() -> String {
    "text".into()
}
fn default_source() -> String {
    "manual".into()
}
fn one() -> i64 {
    1
}

/// 新建/捕获请求（POST /api/clipboard/items）。
#[derive(Debug, Default, Deserialize)]
pub struct CaptureRequest {
    pub content: String,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default)]
    pub item_type: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub metadata: Option<Map<String, Value>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdateRequest {
    pub content: Option<String>,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub is_favorite: Option<bool>,
    pub is_pinned: Option<bool>,
}

#[derive(Debug, Default)]
pub struct ListQuery {
    pub query: Option<String>,
    /// 类型筛选；逗号分隔表示「任一」（如 `text,code`）
    pub item_type: Option<String>,
    pub favorite_only: bool,
    pub pinned_only: bool,
    /// 跳过前多少条（分段加载）
    pub offset: usize,
    pub limit: usize,
}

pub fn normalize_text(content: &str) -> String {
    content.replace("\r\n", "\n").trim().to_string()
}

pub fn detect_type(content: &str) -> &'static str {
    let normalized = normalize_text(content);
    if normalized.is_empty() {
        return "text";
    }
    if URL.is_match(&normalized) {
        return "link";
    }
    if WINDOWS_PATH.is_match(&normalized) || UNC_PATH.is_match(&normalized) {
        return "file";
    }
    if normalized.contains('\n') && (normalized.contains("```") || CODE_WORDS.is_match(&normalized)) {
        return "code";
    }
    if normalized.chars().count() > 280 || normalized.matches('\n').count() >= 3 {
        return "code";
    }
    "text"
}

pub fn content_hash(content: &str) -> String {
    let normalized = normalize_text(content);
    if normalized.is_empty() {
        return String::new();
    }
    hex(&Sha256::digest(normalized.as_bytes()))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn build_title(item_type: &str, content: &str) -> String {
    let normalized = normalize_text(content);
    if normalized.is_empty() {
        return "空剪切板记录".into();
    }
    let first_line = normalized.lines().next().unwrap_or("").trim();
    let take = |n: usize| first_line.chars().take(n).collect::<String>();
    match item_type {
        "link" => take(72),
        "file" => {
            let name = first_line.rsplit(['/', '\\']).next().unwrap_or("");
            if name.is_empty() { take(72) } else { name.to_string() }
        }
        _ => take(36),
    }
}

fn build_summary(content: &str) -> String {
    let normalized = normalize_text(content);
    let single_line = normalized.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
    single_line.chars().take(240).collect()
}

pub struct ClipboardRepo {
    dir: PathBuf,
    lock: Mutex<()>,
}

impl ClipboardRepo {
    pub fn new(data_dir: &Path) -> Self {
        Self { dir: data_dir.join("clipboard"), lock: Mutex::new(()) }
    }

    pub fn images_dir(&self) -> PathBuf {
        self.dir.join("images")
    }

    fn item_path(&self, id: &str) -> Option<PathBuf> {
        is_safe_identifier(id).then(|| self.dir.join(format!("{id}.json")))
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn load(&self, id: &str) -> Option<ClipboardItem> {
        let path = self.item_path(id)?;
        json_store::read_json_file(&path).ok()
    }

    fn save(&self, item: &mut ClipboardItem) -> std::io::Result<()> {
        item.updated_at = now_iso();
        let path = self.item_path(&item.id).ok_or_else(|| std::io::Error::other("非法剪贴板条目 id"))?;
        json_store::write_json_atomic(&path, item)
    }

    fn load_all(&self) -> Vec<ClipboardItem> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .filter_map(|path| match json_store::read_json_file::<ClipboardItem>(&path) {
                Ok(item) => Some(item),
                Err(error) => {
                    // 旧版这里静默吞异常（已知缺陷 #6），损坏文件必须可观测
                    tracing::warn!("跳过无法解析的剪贴板条目 {}: {error}", path.display());
                    None
                }
            })
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<ClipboardItem> {
        self.load(id)
    }

    /// 捕获（去重）：同内容只刷新时间并累加重复次数。
    pub fn capture(&self, request: CaptureRequest) -> std::io::Result<ClipboardItem> {
        let normalized = normalize_text(&request.content);
        let hash = content_hash(&normalized);
        let _guard = self.guard();
        if !normalized.is_empty() {
            let duplicate = self.load_all().into_iter().find(|item| {
                (item.content_hash.is_empty() || item.content_hash == hash) && normalize_text(&item.content) == normalized
            });
            if let Some(mut item) = duplicate {
                item.source = if request.source.is_empty() { item.source } else { request.source };
                item.content_size = normalized.chars().count() as i64;
                if item.content_hash.is_empty() {
                    item.content_hash = hash;
                }
                item.duplicate_count = item.duplicate_count.max(1) + 1;
                if let Some(metadata) = request.metadata {
                    item.metadata.extend(metadata);
                }
                self.save(&mut item)?;
                return Ok(item);
            }
        }
        let item_type = request
            .item_type
            .filter(|t| ITEM_TYPES.contains(&t.as_str()))
            .unwrap_or_else(|| detect_type(&normalized).to_string());
        let now = now_iso();
        let mut item = ClipboardItem {
            id: uuid::Uuid::new_v4().to_string(),
            created_at: now.clone(),
            updated_at: now,
            title: request.title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).unwrap_or_else(|| build_title(&item_type, &normalized)),
            summary: request.summary.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| build_summary(&normalized)),
            item_type,
            content_size: normalized.chars().count() as i64,
            content: normalized,
            source: request.source,
            is_favorite: false,
            is_pinned: false,
            duplicate_count: 1,
            content_hash: hash,
            metadata: request.metadata.unwrap_or_default(),
        };
        std::fs::create_dir_all(&self.dir)?;
        self.save(&mut item)?;
        Ok(item)
    }

    /// 置顶在前，其余按更新时间倒序。
    pub fn list(&self, query: &ListQuery) -> Vec<ClipboardItem> {
        self.list_page(query).0
    }

    /// 分段加载：按 offset / limit 取一页，同时返回筛选后的总条数。
    pub fn list_page(&self, query: &ListQuery) -> (Vec<ClipboardItem>, usize) {
        let types: Vec<&str> = query
            .item_type
            .as_deref()
            .map(|t| t.split(',').map(str::trim).filter(|t| !t.is_empty()).collect())
            .unwrap_or_default();
        let needle = query.query.as_deref().map(|q| q.trim().to_lowercase()).unwrap_or_default();
        let mut items: Vec<ClipboardItem> = self
            .load_all()
            .into_iter()
            .filter(|item| !query.favorite_only || item.is_favorite)
            .filter(|item| !query.pinned_only || item.is_pinned)
            .filter(|item| types.is_empty() || types.contains(&item.item_type.as_str()))
            .filter(|item| needle.is_empty() || format!("{} {} {}", item.title, item.summary, item.content).to_lowercase().contains(&needle))
            .collect();
        items.sort_by(|a, b| {
            b.is_pinned
                .cmp(&a.is_pinned)
                .then_with(|| parse_time(&b.updated_at).cmp(&parse_time(&a.updated_at)))
        });
        let total = items.len();
        let page = items.into_iter().skip(query.offset).take(query.limit.max(1)).collect();
        (page, total)
    }

    pub fn update(&self, id: &str, request: UpdateRequest) -> std::io::Result<Option<ClipboardItem>> {
        let _guard = self.guard();
        let Some(mut item) = self.load(id) else { return Ok(None) };
        let content_changed = request.content.is_some();
        if let Some(content) = request.content {
            item.content = normalize_text(&content);
            item.item_type = detect_type(&item.content).to_string();
            item.content_size = item.content.chars().count() as i64;
            item.content_hash = content_hash(&item.content);
        }
        match request.title {
            Some(title) => item.title = title.trim().to_string(),
            None if content_changed => item.title = build_title(&item.item_type, &item.content),
            None => {}
        }
        match request.summary {
            Some(summary) => item.summary = summary.trim().to_string(),
            None if content_changed => item.summary = build_summary(&item.content),
            None => {}
        }
        if let Some(favorite) = request.is_favorite {
            item.is_favorite = favorite;
        }
        if let Some(pinned) = request.is_pinned {
            item.is_pinned = pinned;
        }
        self.save(&mut item)?;
        Ok(Some(item))
    }

    pub fn delete(&self, id: &str) -> bool {
        let Some(path) = self.item_path(id) else { return false };
        let _guard = self.guard();
        std::fs::remove_file(path).is_ok()
    }

    pub fn stats(&self) -> Value {
        let items = self.load_all();
        let today = chrono::Local::now().date_naive();
        json!({
            "total": items.len(),
            "today": items.iter().filter(|i| parse_time(&i.created_at).map(|t| t.date() == today).unwrap_or(false)).count(),
            "favorites": items.iter().filter(|i| i.is_favorite).count(),
            "pinned": items.iter().filter(|i| i.is_pinned).count(),
        })
    }

    /// 图片条目的本地文件：历史多种写法（相对/绝对/带 data 前缀/纯文件名）统一重定位到 images 目录，
    /// 且必须落在 images 目录内。
    pub fn image_path(&self, id: &str) -> Option<PathBuf> {
        let item = self.load(id).filter(|i| i.item_type == "image")?;
        let images = self.images_dir();
        let images_root = std::fs::canonicalize(&images).ok()?;
        let text = |key: &str| item.metadata.get(key).and_then(Value::as_str).unwrap_or("").trim().to_string();
        let raw = text("image_path").replace('\\', "/");
        let mut candidates = Vec::new();
        if !raw.is_empty() {
            if Path::new(&raw).is_absolute() {
                candidates.push(PathBuf::from(&raw));
            } else {
                candidates.push(self.dir.join(&raw));
            }
            let lowered = raw.to_lowercase();
            if let Some(index) = lowered.find("clipboard/images/") {
                candidates.push(images.join(&raw[index + "clipboard/images/".len()..]));
            } else if let Some(rest) = raw.strip_prefix("images/") {
                candidates.push(images.join(rest));
            } else if !raw.contains('/') {
                candidates.push(images.join(&raw));
            }
        }
        let stored = text("stored_file_name");
        if !stored.is_empty() {
            candidates.push(images.join(stored));
        }
        candidates.into_iter().find_map(|candidate| {
            let resolved = std::fs::canonicalize(&candidate).ok()?;
            if !resolved.starts_with(&images_root) {
                tracing::warn!("拒绝越界的剪贴板图片路径: {}", candidate.display());
                return None;
            }
            resolved.is_file().then_some(resolved)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(tag: &str) -> (ClipboardRepo, PathBuf) {
        let dir = std::env::temp_dir().join(format!("att-clip-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("clipboard").join("images")).unwrap();
        (ClipboardRepo::new(&dir), dir)
    }

    #[test]
    fn 类型识别与标题摘要() {
        assert_eq!(detect_type("https://a.com/x"), "link");
        assert_eq!(detect_type(r"C:\Users\a.txt"), "file");
        assert_eq!(detect_type("line1\nconst a = 1"), "code");
        assert_eq!(detect_type(&"字".repeat(281)), "code");
        assert_eq!(detect_type("你好"), "text");
        assert_eq!(build_title("file", r"C:\Users\report.pdf"), "report.pdf");
        assert_eq!(build_summary(" a \n\n b "), "a b");
    }

    #[test]
    fn 捕获去重_列表排序_更新与统计() {
        let (repo, dir) = repo("crud");
        let first = repo.capture(CaptureRequest { content: "你好\r\n世界 ".into(), source: "monitor".into(), ..Default::default() }).unwrap();
        assert_eq!(first.content, "你好\n世界");
        let again = repo.capture(CaptureRequest { content: "你好\n世界".into(), source: "monitor".into(), ..Default::default() }).unwrap();
        assert_eq!(again.id, first.id);
        assert_eq!(again.duplicate_count, 2);
        let link = repo.capture(CaptureRequest { content: "https://example.com".into(), ..Default::default() }).unwrap();
        assert_eq!(link.item_type, "link");
        repo.update(&first.id, UpdateRequest { is_pinned: Some(true), ..Default::default() }).unwrap();
        let items = repo.list(&ListQuery { limit: 100, ..Default::default() });
        assert_eq!(items[0].id, first.id, "置顶在前");
        assert_eq!(repo.list(&ListQuery { item_type: Some("link".into()), limit: 10, ..Default::default() }).len(), 1);
        assert_eq!(repo.list(&ListQuery { query: Some("EXAMPLE".into()), limit: 10, ..Default::default() }).len(), 1);

        // 分段加载：offset 跳过、total 是筛选后的总数；逗号分隔的类型取并集
        let (first, total) = repo.list_page(&ListQuery { limit: 1, ..Default::default() });
        let (second, _) = repo.list_page(&ListQuery { offset: 1, limit: 1, ..Default::default() });
        assert_eq!(total, items.len());
        assert_eq!(first.len(), 1);
        assert_ne!(first[0].id, second.first().map(|i| i.id.clone()).unwrap_or_default());
        let (beyond, _) = repo.list_page(&ListQuery { offset: total, limit: 10, ..Default::default() });
        assert!(beyond.is_empty());
        let (_, link_or_text) = repo.list_page(&ListQuery { item_type: Some("link, text".into()), limit: 10, ..Default::default() });
        let singles = ["link", "text"].iter().map(|t| repo.list(&ListQuery { item_type: Some((*t).into()), limit: 100, ..Default::default() }).len()).sum::<usize>();
        assert_eq!(link_or_text, singles);
        let stats = repo.stats();
        assert_eq!(stats["total"], 2);
        assert_eq!(stats["today"], 2);
        assert_eq!(stats["pinned"], 1);
        assert!(repo.delete(&link.id));
        assert!(!repo.delete("../etc/passwd"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 图片路径_重定位且拒绝越界() {
        let (repo, dir) = repo("image");
        std::fs::write(repo.images_dir().join("abc.png"), b"png").unwrap();
        std::fs::write(dir.join("secret.png"), b"x").unwrap();
        let ok = repo
            .capture(CaptureRequest {
                content: "image:abc".into(),
                item_type: Some("image".into()),
                metadata: Some(serde_json::from_value(json!({"image_path": "/old/machine/data/clipboard/images/abc.png"})).unwrap()),
                ..Default::default()
            })
            .unwrap();
        assert!(repo.image_path(&ok.id).unwrap().ends_with("abc.png"));
        let evil = repo
            .capture(CaptureRequest {
                content: "image:evil".into(),
                item_type: Some("image".into()),
                metadata: Some(serde_json::from_value(json!({"image_path": "../../secret.png"})).unwrap()),
                ..Default::default()
            })
            .unwrap();
        assert!(repo.image_path(&evil.id).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
