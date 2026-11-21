//! 剪贴板历史（移植自 Python `app/clipboard/manager.py`）：`data/clipboard/<id>.json` 一条一文件，
//! 图片落 `data/clipboard/images/<sha256>.png`。文件格式与 Python 版一致（旧数据直接可读）。
//!
//! - 捕获去重：规范化文本（CRLF→LF、trim）的 SHA-256 相同即视为重复，只累加 `duplicate_count`；
//! - 类型识别：链接 / 文件路径 / 代码 / 纯文本（图片由监控方显式指定）；
//! - 图片路径一律解析到 `data/clipboard/images/` 内，越界拒绝（防借图片接口读任意文件）；
//! - 缩略图：`data/clipboard/thumbnails/<原图文件名>`，长边不超过 [`THUMBNAIL_MAX`]，
//!   新图入库时顺手生成，旧图首次请求时生成；原图本身够小就直接用原图。
//! - 删除（单条、批量、清空、按上限自动清理）连同不再被其他条目引用的图片与缩略图一起删；
//!   图片按内容哈希命名、可能被多条共用，按引用判断。置顶和收藏的条目永不自动删除。

use crate::records::is_safe_identifier;
use crate::storage::dir_index::DirIndex;
use crate::storage::json_store;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^https?://").expect("正则合法"));
static WINDOWS_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-zA-Z]:[\\/]").expect("正则合法"));
static UNC_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\\\\[^\\]+\\[^\\]+").expect("正则合法"));
static CODE_WORDS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(function|const|let|class|def|import|return)\b").expect("正则合法"));

pub const ITEM_TYPES: [&str; 5] = ["text", "link", "code", "file", "image"];

/// 缩略图长边上限（像素）：悬浮框卡片约 250px 宽、剪贴板页悬停预览最大 280px，按 2 倍屏留够
pub const THUMBNAIL_MAX: u32 = 560;

/// 按长边等比缩到 [`THUMBNAIL_MAX`] 以内；本来就不超过时返回 None（直接用原图）。
pub fn make_thumbnail(image: &image::RgbaImage) -> Option<image::RgbaImage> {
    let (width, height) = image.dimensions();
    let long = width.max(height);
    if long <= THUMBNAIL_MAX {
        return None;
    }
    let scale = THUMBNAIL_MAX as f64 / long as f64;
    let target = |side: u32| ((side as f64 * scale).round() as u32).max(1);
    Some(image::imageops::thumbnail(image, target(width), target(height)))
}

/// 写 PNG：同目录临时文件 + 改名（并发生成同一张缩略图时互不踩踏，读到的总是完整文件）。
fn write_png_atomic(path: &Path, image: &image::RgbaImage) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    let result = image
        .save_with_format(&tmp, image::ImageFormat::Png)
        .map_err(std::io::Error::other)
        .and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// 与 Python `datetime.isoformat()` 一致的本地无时区时间（旧数据两版互读）。
fn now_iso() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.6f").to_string()
}

fn parse_time(value: &str) -> Option<chrono::NaiveDateTime> {
    chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f").ok().or_else(|| {
        chrono::DateTime::parse_from_rfc3339(value).ok().map(|t| t.with_timezone(&chrono::Local).naive_local())
    })
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
            if name.is_empty() {
                take(72)
            } else {
                name.to_string()
            }
        }
        _ => take(36),
    }
}

fn build_summary(content: &str) -> String {
    let normalized = normalize_text(content);
    let single_line = normalized.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
    single_line.chars().take(240).collect()
}

/// 剪贴板历史的自动清理上限（来自设置；0 = 不限）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RetentionLimits {
    /// 最多保留多少条：置顶、收藏也占名额，但永不自动删除
    pub max_items: usize,
    /// 超过多少天没再复制过的删除
    pub retention_days: i64,
}

impl RetentionLimits {
    pub fn is_unlimited(&self) -> bool {
        self.max_items == 0 && self.retention_days <= 0
    }
}

/// 置顶或收藏：永不自动删除，「清空历史」也保留。
fn is_protected(item: &ClipboardItem) -> bool {
    item.is_pinned || item.is_favorite
}

/// 图片条目引用的图片文件名（`images/` 下的单层文件名，缩略图同名）；不是图片或认不出时返回 None。
fn image_file_name(item: &ClipboardItem) -> Option<String> {
    if item.item_type != "image" {
        return None;
    }
    let text = |key: &str| item.metadata.get(key).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let stored = text("stored_file_name");
    let name = if stored.is_empty() {
        text("image_path").replace('\\', "/").rsplit('/').next().unwrap_or("").to_string()
    } else {
        stored
    };
    is_safe_file_name(&name).then_some(name)
}

/// 按上限要自动删除的条目 id（纯函数）：超过天数没再复制过的、超出条数的最旧条目；
/// 置顶和收藏永不删除（但占条数名额），最新的一条非置顶非收藏条目总会留下（刚复制的不会立刻消失）。
fn prune_candidates(items: &[ClipboardItem], limits: RetentionLimits, now: chrono::NaiveDateTime) -> Vec<String> {
    let mut normal: Vec<&ClipboardItem> = items.iter().filter(|item| !is_protected(item)).collect();
    // 新的在前；时间解析不了的排到最后（当作最旧）
    normal.sort_by_cached_key(|item| std::cmp::Reverse(parse_time(&item.updated_at)));
    let mut doomed = HashSet::new();
    if limits.retention_days > 0 {
        let cutoff = now - chrono::Duration::days(limits.retention_days);
        for item in &normal {
            // 时间解析不了的不按天数删（宁可留着）
            if parse_time(&item.updated_at).is_some_and(|time| time < cutoff) {
                doomed.insert(item.id.as_str());
            }
        }
    }
    if limits.max_items > 0 {
        let protected = items.len() - normal.len();
        let keep = limits.max_items.saturating_sub(protected).max(1);
        doomed.extend(normal.iter().skip(keep).map(|item| item.id.as_str()));
    }
    normal.iter().filter(|item| doomed.contains(item.id.as_str())).map(|item| item.id.clone()).collect()
}

/// 剪贴板仓库：列表 / 去重 / 统计走内存索引（[`DirIndex`]），写入在同一把锁下同步索引；
/// 单条读取仍以文件为准。
pub struct ClipboardRepo {
    dir: PathBuf,
    index: Mutex<DirIndex<ClipboardItem>>,
}

/// 扫描剪贴板目录（损坏条目跳过并计数）。
fn scan_items(dir: &Path) -> (Vec<ClipboardItem>, usize) {
    let Ok(entries) = std::fs::read_dir(dir) else { return (Vec::new(), 0) };
    let mut skipped = 0usize;
    let items = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter_map(|path| match json_store::read_json_file::<ClipboardItem>(&path) {
            Ok(item) => Some(item),
            Err(error) => {
                // 旧版这里静默吞异常（已知缺陷 #6），损坏文件必须可观测
                skipped += 1;
                tracing::warn!("跳过无法解析的剪贴板条目 {}: {error}", path.display());
                None
            }
        })
        .collect();
    (items, skipped)
}

/// 写入成功后把条目同步进索引（已有则替换，没有则追加；顺序在列表时再排）。
fn upsert(items: &mut Vec<ClipboardItem>, item: &ClipboardItem) {
    match items.iter_mut().find(|i| i.id == item.id) {
        Some(slot) => *slot = item.clone(),
        None => items.push(item.clone()),
    }
}

impl ClipboardRepo {
    pub fn new(data_dir: &Path) -> Self {
        let dir = data_dir.join("clipboard");
        Self { index: Mutex::new(DirIndex::new(dir.clone())), dir }
    }

    pub fn images_dir(&self) -> PathBuf {
        self.dir.join("images")
    }

    pub fn thumbnails_dir(&self) -> PathBuf {
        self.dir.join("thumbnails")
    }

    /// 新图入库时由监控方调用：内存里已有解码好的整图，直接缩出缩略图（够小则不生成）。
    pub fn save_thumbnail(&self, stored_file_name: &str, image: &image::RgbaImage) -> std::io::Result<()> {
        let Some(thumbnail) = make_thumbnail(image) else { return Ok(()) };
        if !is_safe_file_name(stored_file_name) {
            return Err(std::io::Error::other("非法图片文件名"));
        }
        std::fs::create_dir_all(self.thumbnails_dir())?;
        write_png_atomic(&self.thumbnails_dir().join(stored_file_name), &thumbnail)
    }

    /// 图片条目的缩略图路径：已生成的直接用；没有就从原图生成（阻塞，调用方放到阻塞线程）；
    /// 原图够小、或格式解不开（旧版的 jpg / gif 等）时返回原图。
    pub fn thumbnail_path(&self, id: &str) -> Option<PathBuf> {
        let original = self.image_path(id)?;
        let file_name = original.file_name()?.to_str()?.to_string();
        let cached = self.thumbnails_dir().join(&file_name);
        if cached.is_file() {
            return Some(cached);
        }
        let decoded = image::open(&original).map(|image| image.to_rgba8());
        let thumbnail = match decoded {
            Ok(image) => make_thumbnail(&image),
            Err(error) => {
                tracing::debug!("剪贴板图片无法生成缩略图，改用原图 {}: {error}", original.display());
                None
            }
        };
        let Some(thumbnail) = thumbnail else { return Some(original) };
        let written =
            std::fs::create_dir_all(self.thumbnails_dir()).and_then(|()| write_png_atomic(&cached, &thumbnail));
        match written {
            Ok(()) => Some(cached),
            Err(error) => {
                tracing::warn!("写入剪贴板缩略图失败，改用原图: {error}");
                Some(original)
            }
        }
    }

    fn item_path(&self, id: &str) -> Option<PathBuf> {
        is_safe_identifier(id).then(|| self.dir.join(format!("{id}.json")))
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, DirIndex<ClipboardItem>> {
        self.index.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn load(&self, id: &str) -> Option<ClipboardItem> {
        let path = self.item_path(id)?;
        json_store::read_json_file(&path).ok()
    }

    /// 锁内保存并同步索引（刷新 updated_at；写入前先核对目录是否有外部变化）。
    fn save(&self, index: &mut DirIndex<ClipboardItem>, item: &mut ClipboardItem) -> std::io::Result<()> {
        item.updated_at = now_iso();
        let path = self.item_path(&item.id).ok_or_else(|| std::io::Error::other("非法剪贴板条目 id"))?;
        index.before_write();
        json_store::write_json_atomic(&path, item)?;
        index.after_write(|items| upsert(items, item));
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<ClipboardItem> {
        self.load(id)
    }

    /// 捕获（去重）：同内容只刷新时间并累加重复次数。
    pub fn capture(&self, request: CaptureRequest) -> std::io::Result<ClipboardItem> {
        let normalized = normalize_text(&request.content);
        let hash = content_hash(&normalized);
        let mut index = self.guard();
        if !normalized.is_empty() {
            index.refresh(scan_items);
            let duplicate = index
                .items()
                .iter()
                .find(|item| {
                    (item.content_hash.is_empty() || item.content_hash == hash)
                        && normalize_text(&item.content) == normalized
                })
                .cloned();
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
                self.save(&mut index, &mut item)?;
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
            title: request
                .title
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| build_title(&item_type, &normalized)),
            summary: request
                .summary
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| build_summary(&normalized)),
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
        self.save(&mut index, &mut item)?;
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
        let hit = |text: &str| text.to_lowercase().contains(&needle);
        let mut index = self.guard();
        index.refresh(scan_items);
        let mut items: Vec<&ClipboardItem> = index
            .items()
            .iter()
            .filter(|item| !query.favorite_only || item.is_favorite)
            .filter(|item| !query.pinned_only || item.is_pinned)
            .filter(|item| types.is_empty() || types.contains(&item.item_type.as_str()))
            .filter(|item| needle.is_empty() || hit(&item.title) || hit(&item.summary) || hit(&item.content))
            .collect();
        items.sort_by_cached_key(|item| std::cmp::Reverse((item.is_pinned, parse_time(&item.updated_at))));
        let total = items.len();
        let page = items.into_iter().skip(query.offset).take(query.limit.max(1)).cloned().collect();
        (page, total)
    }

    pub fn update(&self, id: &str, request: UpdateRequest) -> std::io::Result<Option<ClipboardItem>> {
        let mut index = self.guard();
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
        self.save(&mut index, &mut item)?;
        Ok(Some(item))
    }

    pub fn delete(&self, id: &str) -> bool {
        let mut index = self.guard();
        self.remove_locked(&mut index, &HashSet::from([id.to_string()])) == 1
    }

    /// 批量删除（剪贴板页「删除选中」）；返回删掉的条数。
    pub fn delete_many(&self, ids: &[String]) -> usize {
        let ids: HashSet<String> = ids.iter().cloned().collect();
        let mut index = self.guard();
        self.remove_locked(&mut index, &ids)
    }

    /// 清空历史：置顶和收藏的保留。返回（删掉的条数, 保留的条数）。
    pub fn clear(&self) -> (usize, usize) {
        let mut index = self.guard();
        index.refresh(scan_items);
        let ids: HashSet<String> =
            index.items().iter().filter(|item| !is_protected(item)).map(|item| item.id.clone()).collect();
        let removed = self.remove_locked(&mut index, &ids);
        index.refresh(scan_items);
        (removed, index.items().len())
    }

    /// 按上限自动清理（入库后与每小时一次）；不限时什么都不做。返回删掉的条数。
    pub fn prune(&self, limits: RetentionLimits) -> usize {
        if limits.is_unlimited() {
            return 0;
        }
        let mut index = self.guard();
        index.refresh(scan_items);
        let ids: HashSet<String> =
            prune_candidates(index.items(), limits, chrono::Local::now().naive_local()).into_iter().collect();
        self.remove_locked(&mut index, &ids)
    }

    /// 锁内删除一批条目，再删掉其中不再被剩余条目引用的图片与缩略图。返回删掉的条数。
    fn remove_locked(&self, index: &mut DirIndex<ClipboardItem>, ids: &HashSet<String>) -> usize {
        if ids.is_empty() {
            return 0;
        }
        index.refresh(scan_items);
        let doomed: Vec<ClipboardItem> = index.items().iter().filter(|item| ids.contains(&item.id)).cloned().collect();
        if doomed.is_empty() {
            return 0;
        }
        index.before_write();
        let mut removed = HashSet::new();
        for item in &doomed {
            let Some(path) = self.item_path(&item.id) else { continue };
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    removed.insert(item.id.clone());
                }
                Err(error) => tracing::warn!("删除剪贴板条目失败 {}: {error}", path.display()),
            }
        }
        index.after_write(|items| items.retain(|item| !removed.contains(&item.id)));
        // 写入前发现外部变化时缓存已作废：重新扫一遍再判断引用，免得误删别的条目还在用的图片
        index.refresh(scan_items);
        let still_used: HashSet<String> = index.items().iter().filter_map(image_file_name).collect();
        let orphans: HashSet<String> = doomed
            .iter()
            .filter(|item| removed.contains(&item.id))
            .filter_map(image_file_name)
            .filter(|name| !still_used.contains(name))
            .collect();
        for name in orphans {
            for dir in [self.images_dir(), self.thumbnails_dir()] {
                let path = dir.join(&name);
                if path.is_file() {
                    if let Err(error) = std::fs::remove_file(&path) {
                        tracing::warn!("删除剪贴板图片失败 {}: {error}", path.display());
                    }
                }
            }
        }
        removed.len()
    }

    pub fn stats(&self) -> Value {
        let mut index = self.guard();
        index.refresh(scan_items);
        let items = index.items();
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

/// 缩略图沿用原图文件名：只允许单层文件名（不含路径分隔符、不以点开头）。
fn is_safe_file_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\']) && name != ".."
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
        let first = repo
            .capture(CaptureRequest {
                content: "你好\r\n世界 ".into(), source: "monitor".into(), ..Default::default()
            })
            .unwrap();
        assert_eq!(first.content, "你好\n世界");
        let again = repo
            .capture(CaptureRequest {
                content: "你好\n世界".into(), source: "monitor".into(), ..Default::default()
            })
            .unwrap();
        assert_eq!(again.id, first.id);
        assert_eq!(again.duplicate_count, 2);
        let link =
            repo.capture(CaptureRequest { content: "https://example.com".into(), ..Default::default() }).unwrap();
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
        let (_, link_or_text) =
            repo.list_page(&ListQuery { item_type: Some("link, text".into()), limit: 10, ..Default::default() });
        let singles = ["link", "text"]
            .iter()
            .map(|t| repo.list(&ListQuery { item_type: Some((*t).into()), limit: 100, ..Default::default() }).len())
            .sum::<usize>();
        assert_eq!(link_or_text, singles);
        let stats = repo.stats();
        assert_eq!(stats["total"], 2);
        assert_eq!(stats["today"], 2);
        assert_eq!(stats["pinned"], 1);
        assert!(repo.delete(&link.id));
        assert_eq!(repo.stats()["total"], 1, "删除同步移出索引");
        assert!(!repo.delete("../etc/passwd"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn image_item(repo: &ClipboardRepo, file_name: &str) -> ClipboardItem {
        repo.capture(CaptureRequest {
            content: format!("image:{file_name}"),
            item_type: Some("image".into()),
            metadata: Some(serde_json::from_value(json!({"image_path": format!("images/{file_name}")})).unwrap()),
            ..Default::default()
        })
        .unwrap()
    }

    #[test]
    fn 缩略图_大图按长边缩到上限并缓存_小图与解不开的用原图() {
        let (repo, dir) = repo("thumb");
        image::RgbaImage::from_pixel(1200, 720, image::Rgba([10, 20, 30, 255]))
            .save(repo.images_dir().join("big.png"))
            .unwrap();
        image::RgbaImage::from_pixel(300, 200, image::Rgba([1, 2, 3, 255]))
            .save(repo.images_dir().join("small.png"))
            .unwrap();
        std::fs::write(repo.images_dir().join("broken.png"), b"not a png").unwrap();

        let big = image_item(&repo, "big.png");
        let thumb = repo.thumbnail_path(&big.id).unwrap();
        assert!(thumb.starts_with(repo.thumbnails_dir()), "{}", thumb.display());
        assert_eq!(image::image_dimensions(&thumb).unwrap(), (560, 336), "长边 560，等比");
        let modified = std::fs::metadata(&thumb).unwrap().modified().unwrap();
        assert_eq!(repo.thumbnail_path(&big.id).unwrap(), thumb);
        assert_eq!(std::fs::metadata(&thumb).unwrap().modified().unwrap(), modified, "第二次直接用缓存");

        let small = image_item(&repo, "small.png");
        assert!(repo.thumbnail_path(&small.id).unwrap().ends_with("images/small.png"), "够小直接用原图");
        let broken = image_item(&repo, "broken.png");
        assert!(repo.thumbnail_path(&broken.id).unwrap().ends_with("images/broken.png"), "解不开退回原图");

        // 入库时由监控方用内存里的整图生成；文件名不能带路径
        let wide = image::RgbaImage::from_pixel(2000, 100, image::Rgba([0, 0, 0, 255]));
        repo.save_thumbnail("wide.png", &wide).unwrap();
        assert_eq!(image::image_dimensions(repo.thumbnails_dir().join("wide.png")).unwrap(), (560, 28));
        assert!(repo.save_thumbnail("../escape.png", &wide).is_err());
        assert!(make_thumbnail(&image::RgbaImage::new(THUMBNAIL_MAX, 100)).is_none(), "正好等于上限不缩");
        let leftovers = std::fs::read_dir(repo.thumbnails_dir())
            .unwrap()
            .flatten()
            .filter(|e| e.path().to_string_lossy().contains(".tmp"))
            .count();
        assert_eq!(leftovers, 0, "不留临时文件");
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
                metadata: Some(
                    serde_json::from_value(json!({"image_path": "/old/machine/data/clipboard/images/abc.png"}))
                        .unwrap(),
                ),
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

    /// 直接写一条条目文件（指定更新时间 / 置顶 / 收藏 / 图片），模拟已有的历史。
    fn put(repo_dir: &Path, id: &str, minutes_ago: i64, pinned: bool, favorite: bool, image: Option<&str>) {
        let time = (chrono::Local::now() - chrono::Duration::minutes(minutes_ago)).format("%Y-%m-%dT%H:%M:%S%.6f");
        let mut metadata = Map::new();
        if let Some(name) = image {
            metadata.insert("image_path".into(), json!(format!("images/{name}")));
            metadata.insert("stored_file_name".into(), json!(name));
        }
        let item = ClipboardItem {
            id: id.into(),
            created_at: time.to_string(),
            updated_at: time.to_string(),
            item_type: if image.is_some() { "image".into() } else { "text".into() },
            content: if image.is_some() { format!("image:{id}") } else { format!("内容 {id}") },
            title: id.into(),
            summary: String::new(),
            source: "test".into(),
            is_favorite: favorite,
            is_pinned: pinned,
            content_size: 1,
            duplicate_count: 1,
            content_hash: String::new(),
            metadata,
        };
        json_store::write_json_atomic(&repo_dir.join("clipboard").join(format!("{id}.json")), &item).unwrap();
    }

    fn put_image_files(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir.join("clipboard").join("thumbnails")).unwrap();
        std::fs::write(dir.join("clipboard").join("images").join(name), b"png").unwrap();
        std::fs::write(dir.join("clipboard").join("thumbnails").join(name), b"png").unwrap();
    }

    fn ids(repo: &ClipboardRepo) -> Vec<String> {
        let mut ids: Vec<String> =
            repo.list(&ListQuery { limit: 1000, ..Default::default() }).into_iter().map(|item| item.id).collect();
        ids.sort();
        ids
    }

    #[test]
    fn 条数上限_删最旧的非置顶非收藏_连同它独占的图片() {
        let (repo, dir) = repo("prune-count");
        // c-0 最新 … c-49 最旧；c-49 是独占图片的图片条目，c-48 置顶、c-47 收藏（都更旧但不能删）
        for n in 0..47 {
            put(&dir, &format!("c-{n:02}"), n, false, false, None);
        }
        put(&dir, "c-47", 47, false, true, None);
        put(&dir, "c-48", 48, true, false, None);
        put(&dir, "c-49", 49, false, false, Some("old.png"));
        put_image_files(&dir, "old.png");
        assert_eq!(repo.prune(RetentionLimits { max_items: 50, retention_days: 0 }), 0, "没超出不删");

        // 复制第 51 条
        repo.capture(CaptureRequest { content: "第 51 条".into(), ..Default::default() }).unwrap();
        assert_eq!(repo.prune(RetentionLimits { max_items: 50, retention_days: 0 }), 1);
        let remaining = ids(&repo);
        assert_eq!(remaining.len(), 50);
        assert!(!remaining.contains(&"c-49".to_string()), "最旧的非置顶非收藏条目被删");
        assert!(remaining.contains(&"c-48".to_string()) && remaining.contains(&"c-47".to_string()));
        assert!(!dir.join("clipboard/images/old.png").exists(), "独占的图片一并删除");
        assert!(!dir.join("clipboard/thumbnails/old.png").exists(), "缩略图一并删除");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 保留天数_置顶收藏不受影响_默认不限不删() {
        let (repo, dir) = repo("prune-days");
        let day = 24 * 60;
        put(&dir, "fresh", 10, false, false, None);
        put(&dir, "old", 3 * day, false, false, None);
        put(&dir, "old-pinned", 3 * day, true, false, None);
        put(&dir, "old-favorite", 3 * day, false, true, None);
        assert_eq!(repo.prune(RetentionLimits::default()), 0, "默认不限：不删任何条目");
        assert_eq!(ids(&repo).len(), 4);

        assert_eq!(repo.prune(RetentionLimits { max_items: 0, retention_days: 2 }), 1);
        assert_eq!(ids(&repo), ["fresh", "old-favorite", "old-pinned"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 上限候选_置顶收藏占名额但不删_最新一条总会留下() {
        let now = chrono::Local::now().naive_local();
        let item = |id: &str, minutes: i64, protected: bool| ClipboardItem {
            id: id.into(),
            created_at: String::new(),
            updated_at: (now - chrono::Duration::minutes(minutes)).format("%Y-%m-%dT%H:%M:%S%.6f").to_string(),
            item_type: "text".into(),
            content: String::new(),
            title: String::new(),
            summary: String::new(),
            source: String::new(),
            is_favorite: protected,
            is_pinned: false,
            content_size: 0,
            duplicate_count: 1,
            content_hash: String::new(),
            metadata: Map::new(),
        };
        let items = vec![item("p1", 5, true), item("p2", 6, true), item("n1", 1, false), item("n2", 2, false)];
        // 上限 2、置顶收藏已有 2 条：普通条目也至少留最新一条
        assert_eq!(prune_candidates(&items, RetentionLimits { max_items: 2, retention_days: 0 }, now), ["n2"]);
        assert_eq!(prune_candidates(&items, RetentionLimits { max_items: 3, retention_days: 0 }, now), ["n2"]);
        assert!(prune_candidates(&items, RetentionLimits { max_items: 4, retention_days: 0 }, now).is_empty());
    }

    #[test]
    fn 批量删除与清空_图片按引用判断_清空保留置顶收藏() {
        let (repo, dir) = repo("bulk-delete");
        put_image_files(&dir, "shared.png");
        put(&dir, "img-a", 1, false, false, Some("shared.png"));
        put(&dir, "img-b", 2, false, false, Some("shared.png"));
        put(&dir, "text-a", 3, false, false, None);
        put(&dir, "pinned", 4, true, false, None);
        put(&dir, "favorite", 5, false, true, None);

        assert_eq!(repo.delete_many(&["img-a".into(), "missing".into()]), 1);
        assert!(dir.join("clipboard/images/shared.png").exists(), "还被 img-b 引用的图片不能删");
        assert!(repo.delete("img-b"));
        assert!(!dir.join("clipboard/images/shared.png").exists(), "最后一个引用删掉后图片一并删除");
        assert!(!dir.join("clipboard/thumbnails/shared.png").exists());
        assert!(!repo.delete("img-b"), "已删的再删返回 false（404）");

        assert_eq!(repo.clear(), (1, 2), "清空只删普通条目");
        assert_eq!(ids(&repo), ["favorite", "pinned"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
