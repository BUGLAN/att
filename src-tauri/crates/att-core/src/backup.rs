//! 数据备份与恢复：把用户数据打成一个 zip，或从 zip 覆盖恢复。
//!
//! - 只处理白名单里的数据：设置、记录、录音、剪贴板、自定义提示词、转写缓存、LLM 配置；
//!   日志与临时文件永不进入备份。
//! - `.encryption_key` 默认不导出；包含密钥时恢复后已保存的凭据可直接使用。
//! - 导入前把当前数据目录完整复制到 `data/.pre-import-<时间>/`；解压或覆盖失败时回滚。
//! - 不含密钥的备份会检查密文能否用本机当前密钥解开；解不开的凭据按「未设置」处理并
//!   写回文件，重启后设置页显示未设置、LLM 档案提示重新填写，而不是启动报错。
//!
//! 导入成功后当前进程仍持有旧的设置 / 目录索引，因此调用方必须提示用户重启应用。

use crate::storage::crypto::Crypto;
use crate::storage::fernet::Fernet;
use crate::storage::json_store;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const BACKUP_FORMAT: &str = "att-backup";
const BACKUP_FORMAT_VERSION: u32 = 1;
const MANIFEST_NAME: &str = "manifest.json";
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_DOCUMENT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_KEY_BYTES: u64 = 4096;
const MAX_ARCHIVE_ENTRIES: usize = 200_000;
const MAX_ARCHIVE_BYTES: u64 = 100 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum BackupPathKind {
    Export,
    Import,
}

/// 桌面对话框确认过的一次性备份路径。
///
/// 数据面只接受这里登记过的路径，避免拿到本地令牌的调用方绕过系统对话框读写任意文件。
#[derive(Debug, Default)]
pub struct BackupPathGrants {
    grants: Mutex<HashSet<(BackupPathKind, PathBuf)>>,
}

impl BackupPathGrants {
    pub fn authorize_export(&self, path: &Path) -> bool {
        self.authorize(BackupPathKind::Export, path)
    }

    pub fn authorize_import(&self, path: &Path) -> bool {
        self.authorize(BackupPathKind::Import, path)
    }

    pub fn consume_export(&self, path: &Path) -> bool {
        self.consume(BackupPathKind::Export, path)
    }

    pub fn consume_import(&self, path: &Path) -> bool {
        self.consume(BackupPathKind::Import, path)
    }

    fn authorize(&self, kind: BackupPathKind, path: &Path) -> bool {
        let Some(path) = normalized_absolute_path(path) else {
            return false;
        };
        let mut grants = self.grants.lock().unwrap_or_else(|error| error.into_inner());
        grants.insert((kind, path));
        true
    }

    fn consume(&self, kind: BackupPathKind, path: &Path) -> bool {
        let Some(path) = normalized_absolute_path(path) else {
            return false;
        };
        let mut grants = self.grants.lock().unwrap_or_else(|error| error.into_inner());
        grants.remove(&(kind, path))
    }
}

fn normalized_absolute_path(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    if let Ok(path) = fs::canonicalize(path) {
        return Some(path);
    }
    let parent = path.parent()?;
    let parent = fs::canonicalize(parent).ok()?;
    Some(parent.join(path.file_name()?))
}

/// 备份里允许出现的顶层路径。其余文件（日志、迁移标记、端口文件等）一律忽略。
const MANAGED_ROOTS: [&str; 8] = [
    "settings.json",
    "records",
    "audio",
    "llm_v2.json",
    "clipboard",
    "custom_prompts.json",
    "transcribe_file_cache.json",
    ".encryption_key",
];

const DIRECTORY_ROOTS: [&str; 3] = ["records", "audio", "clipboard"];
const FILE_ROOTS: [&str; 4] = ["settings.json", "llm_v2.json", "custom_prompts.json", "transcribe_file_cache.json"];
const KEY_ROOT: &str = ".encryption_key";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SectionManifest {
    pub files: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupManifest {
    pub format: String,
    pub format_version: u32,
    #[serde(default)]
    pub app_version: String,
    pub created_at: String,
    #[serde(default)]
    pub includes_key: bool,
    #[serde(default = "default_true")]
    pub includes_audio: bool,
    #[serde(default)]
    pub sections: BTreeMap<String, SectionManifest>,
}

#[derive(Debug, Clone, Copy)]
pub struct ExportOptions {
    pub include_key: bool,
    pub include_audio: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self { include_key: false, include_audio: true }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportResult {
    pub path: String,
    pub bytes: u64,
    pub manifest: BackupManifest,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportResult {
    pub pre_import_backup: String,
    /// 因备份不含密钥、本机又解不开而按「未设置」处理的凭据数量。
    pub credentials_reset: u64,
    pub manifest: BackupManifest,
    pub restart_required: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("{0}")]
    Invalid(String),
    #[error("备份压缩包损坏：{0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("备份文件读写失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("导入失败，原数据已回滚：{cause}")]
    RolledBack { cause: String },
    #[error("导入失败，已回滚：{cause}；回滚失败：{rollback}；导入前数据保留在 {snapshot}")]
    Rollback { cause: String, rollback: String, snapshot: String },
}

fn default_true() -> bool {
    true
}

/// 导出与导入互斥，避免一个备份在覆盖而另一个正在打包。
fn backup_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Debug)]
struct ArchiveEntry {
    name: String,
    source: PathBuf,
    is_dir: bool,
    size: u64,
}

/// 导出备份。`data_dir` 是应用数据目录，`destination` 是用户通过保存对话框选定的 zip 路径。
pub fn export_backup(
    data_dir: &Path,
    app_version: &str,
    destination: &Path,
    options: ExportOptions,
) -> Result<ExportResult, BackupError> {
    let _guard = backup_lock().lock().unwrap_or_else(|error| error.into_inner());
    if destination.as_os_str().is_empty() {
        return Err(BackupError::Invalid("备份保存路径不能为空".into()));
    }
    if destination.is_dir() {
        return Err(BackupError::Invalid("备份保存路径是一个目录，请选择 zip 文件".into()));
    }

    let parent = destination.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".att-backup-{}.tmp", uuid::Uuid::new_v4().simple()));
    let file = OpenOptions::new().write(true).create_new(true).open(&temp)?;
    if let Err(error) = restrict_backup_file_permissions(&temp) {
        let _ = fs::remove_file(&temp);
        return Err(error.into());
    }

    let outcome = write_archive(data_dir, app_version, file, options);
    let (bytes, manifest) = match outcome {
        Ok(value) => value,
        Err(error) => {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
    };

    let previous = parent.join(format!(".att-backup-{}.old", uuid::Uuid::new_v4().simple()));
    let had_destination = destination.exists();
    if had_destination {
        if let Err(error) = fs::rename(destination, &previous) {
            let _ = fs::remove_file(&temp);
            return Err(error.into());
        }
    }
    if let Err(error) = fs::rename(&temp, destination) {
        if had_destination {
            let _ = fs::rename(&previous, destination);
        }
        let _ = fs::remove_file(&temp);
        return Err(error.into());
    }
    if let Err(error) = restrict_backup_file_permissions(destination) {
        let _ = fs::remove_file(destination);
        if had_destination {
            let _ = fs::rename(&previous, destination);
        }
        return Err(error.into());
    }
    if had_destination {
        let _ = fs::remove_file(&previous);
    }
    Ok(ExportResult { path: destination.display().to_string(), bytes, manifest })
}

fn write_archive(
    data_dir: &Path,
    app_version: &str,
    file: File,
    options: ExportOptions,
) -> Result<(u64, BackupManifest), BackupError> {
    let (entries, manifest) = collect_entries(data_dir, app_version, options)?;
    let mut writer = ZipWriter::new(file);
    let manifest_bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|error| BackupError::Invalid(error.to_string()))?;
    writer.start_file(MANIFEST_NAME, entry_options(false, true))?;
    writer.write_all(&manifest_bytes)?;

    for entry in entries {
        if entry.is_dir {
            writer.add_directory(format!("{}/", entry.name), entry_options(true, false))?;
            continue;
        }
        let compressed = !entry.name.starts_with("audio/") && entry.name != KEY_ROOT;
        writer.start_file(entry.name.as_str(), entry_options(false, compressed))?;
        let mut source = File::open(&entry.source)?;
        let copied = io::copy(&mut source, &mut writer)?;
        if copied != entry.size {
            return Err(BackupError::Invalid(format!("读取备份内容时文件大小发生变化：{}", entry.source.display())));
        }
    }

    let file = writer.finish()?;
    file.sync_all()?;
    let bytes = file.metadata()?.len();
    Ok((bytes, manifest))
}

fn entry_options(is_dir: bool, compressed: bool) -> SimpleFileOptions {
    let method = if compressed { CompressionMethod::Deflated } else { CompressionMethod::Stored };
    let permissions = if is_dir { 0o755 } else { 0o644 };
    SimpleFileOptions::default().compression_method(method).unix_permissions(permissions)
}

fn collect_entries(
    data_dir: &Path,
    app_version: &str,
    options: ExportOptions,
) -> Result<(Vec<ArchiveEntry>, BackupManifest), BackupError> {
    let mut entries = Vec::new();
    let mut sections = BTreeMap::new();

    for root in FILE_ROOTS {
        let path = data_dir.join(root);
        let Some(metadata) = managed_root_metadata(&path, root)? else {
            sections.insert(root.to_string(), SectionManifest::default());
            continue;
        };
        if !metadata.is_file() {
            return Err(BackupError::Invalid(format!("数据根路径 {root} 必须是普通文件")));
        }
        let size = metadata.len();
        entries.push(ArchiveEntry { name: root.to_string(), source: path, is_dir: false, size });
        sections.insert(root.to_string(), SectionManifest { files: 1, bytes: size });
    }

    for root in DIRECTORY_ROOTS {
        if root == "audio" && !options.include_audio {
            continue;
        }
        let path = data_dir.join(root);
        let Some(metadata) = managed_root_metadata(&path, root)? else {
            sections.insert(root.to_string(), SectionManifest::default());
            continue;
        };
        if !metadata.is_dir() {
            return Err(BackupError::Invalid(format!("数据根路径 {root} 必须是目录")));
        }
        let mut section_entries = Vec::new();
        walk_export_dir(&path, root, &mut section_entries)?;
        let files = section_entries.iter().filter(|entry| !entry.is_dir).count() as u64;
        let bytes = section_entries.iter().map(|entry| entry.size).sum();
        entries.extend(section_entries);
        sections.insert(root.to_string(), SectionManifest { files, bytes });
    }

    if options.include_key {
        let path = data_dir.join(KEY_ROOT);
        let Some(metadata) = managed_root_metadata(&path, KEY_ROOT)? else {
            return Err(BackupError::Invalid("数据目录缺少 .encryption_key，无法包含密钥导出".into()));
        };
        if !metadata.is_file() {
            return Err(BackupError::Invalid(".encryption_key 必须是普通文件".into()));
        }
        let size = metadata.len();
        entries.push(ArchiveEntry { name: KEY_ROOT.to_string(), source: path, is_dir: false, size });
        sections.insert(KEY_ROOT.to_string(), SectionManifest { files: 1, bytes: size });
    }

    let manifest = BackupManifest {
        format: BACKUP_FORMAT.to_string(),
        format_version: BACKUP_FORMAT_VERSION,
        app_version: app_version.to_string(),
        created_at: crate::now_rfc3339(),
        includes_key: options.include_key,
        includes_audio: options.include_audio,
        sections,
    };
    Ok((entries, manifest))
}

fn managed_root_metadata(path: &Path, label: &str) -> Result<Option<fs::Metadata>, BackupError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() {
        return Err(BackupError::Invalid(format!("数据根路径 {label} 不能是符号链接")));
    }
    Ok(Some(metadata))
}

fn walk_export_dir(dir: &Path, rel_prefix: &str, entries: &mut Vec<ArchiveEntry>) -> io::Result<()> {
    entries.push(ArchiveEntry { name: rel_prefix.to_string(), source: dir.to_path_buf(), is_dir: true, size: 0 });
    let mut children = fs::read_dir(dir)?.flatten().collect::<Vec<_>>();
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        let name = child.file_name().to_string_lossy().into_owned();
        let rel = format!("{rel_prefix}/{name}");
        if should_skip_relative(&rel) {
            continue;
        }
        let path = child.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            tracing::warn!("备份跳过符号链接：{}", path.display());
            continue;
        }
        if metadata.is_dir() {
            walk_export_dir(&path, &rel, entries)?;
        } else if metadata.is_file() {
            entries.push(ArchiveEntry { name: rel, source: path, is_dir: false, size: metadata.len() });
        }
    }
    Ok(())
}

fn should_skip_relative(rel: &str) -> bool {
    if rel == "audio/temp" || rel.starts_with("audio/temp/") {
        return true;
    }
    let name = rel.rsplit('/').next().unwrap_or(rel);
    name.ends_with(".tmp")
        || name == ".DS_Store"
        || name == "Thumbs.db"
        || name.starts_with("._")
        || name.starts_with(".att-backup-")
}

/// 导入备份：先校验并解压到数据目录下的临时 staging，再快照当前数据并覆盖。
///
/// `current_crypto` 用于判断「不带密钥的备份」里的密文本机能否解开；传 `None` 时
/// 所有密文字段都会按无法解密处理。
pub fn import_backup(
    data_dir: &Path,
    source: &Path,
    current_crypto: Option<&Crypto>,
) -> Result<ImportResult, BackupError> {
    import_backup_inner(data_dir, source, current_crypto, None)
}

fn import_backup_inner(
    data_dir: &Path,
    source: &Path,
    current_crypto: Option<&Crypto>,
    fail_after_sections: Option<usize>,
) -> Result<ImportResult, BackupError> {
    let _guard = backup_lock().lock().unwrap_or_else(|error| error.into_inner());
    if !source.is_file() {
        return Err(BackupError::Invalid(format!("备份文件不存在：{}", source.display())));
    }

    let file = File::open(source)?;
    let mut archive = ZipArchive::new(file)?;
    let manifest = read_manifest(&mut archive)?;
    validate_manifest(&manifest)?;
    preflight_archive(&mut archive, &manifest)?;

    fs::create_dir_all(data_dir)?;
    let stage = data_dir.join(format!(".backup-import-{}", uuid::Uuid::new_v4().simple()));
    fs::create_dir(&stage)?;
    if let Err(error) = extract_archive(&mut archive, &stage, &manifest) {
        let _ = remove_path(&stage);
        return Err(error);
    }
    if manifest.includes_key {
        if let Err(error) = validate_extracted_key(&stage) {
            let _ = remove_path(&stage);
            return Err(error);
        }
        if let Err(error) = validate_encrypted_credentials_with_key(&stage) {
            let _ = remove_path(&stage);
            return Err(error);
        }
    }

    let credentials_reset = if manifest.includes_key {
        0
    } else {
        match sanitize_credentials_without_key(&stage, current_crypto) {
            Ok(count) => count,
            Err(error) => {
                let _ = remove_path(&stage);
                return Err(error);
            }
        }
    };

    let snapshot = match create_snapshot(data_dir) {
        Ok(path) => path,
        Err(error) => {
            let _ = remove_path(&stage);
            return Err(error.into());
        }
    };

    // 录音不在清单里时必须清掉本机旧录音，否则恢复后的记录可能播放到旧文件。
    let restore_audio = manifest.sections.contains_key("audio");
    let mut touched = manifest.sections.keys().cloned().collect::<Vec<_>>();
    if !restore_audio {
        touched.push("audio".into());
    }
    if let Err(error) = apply_import(data_dir, &stage, &manifest, restore_audio, fail_after_sections) {
        let rollback = rollback_import(data_dir, &snapshot, &touched);
        let _ = remove_path(&stage);
        match rollback {
            Ok(()) => {
                let _ = remove_path(&snapshot);
                Err(BackupError::RolledBack { cause: error.to_string() })
            }
            Err(rollback_error) => Err(BackupError::Rollback {
                cause: error.to_string(),
                rollback: rollback_error.to_string(),
                snapshot: snapshot.display().to_string(),
            }),
        }
    } else {
        let _ = remove_path(&stage);
        remove_old_snapshots(data_dir, &snapshot);
        Ok(ImportResult {
            pre_import_backup: snapshot.display().to_string(),
            credentials_reset,
            manifest,
            restart_required: true,
        })
    }
}

fn read_manifest(archive: &mut ZipArchive<File>) -> Result<BackupManifest, BackupError> {
    let file = archive
        .by_name(MANIFEST_NAME)
        .map_err(|_| BackupError::Invalid("不是有效的 ATT 备份：压缩包根目录缺少 manifest.json".into()))?;
    if file.is_dir() || !file.is_file() {
        return Err(BackupError::Invalid("manifest.json 必须是普通文件".into()));
    }
    if file.size() > MAX_MANIFEST_BYTES {
        return Err(BackupError::Invalid("manifest.json 异常过大".into()));
    }
    let mut raw = Vec::with_capacity(file.size() as usize);
    file.take(MAX_MANIFEST_BYTES + 1).read_to_end(&mut raw)?;
    if raw.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(BackupError::Invalid("manifest.json 异常过大".into()));
    }
    serde_json::from_slice(&raw).map_err(|error| BackupError::Invalid(format!("manifest.json 格式错误：{error}")))
}

fn validate_manifest(manifest: &BackupManifest) -> Result<(), BackupError> {
    if manifest.format != BACKUP_FORMAT {
        return Err(BackupError::Invalid(format!(
            "不是 ATT 备份：manifest.format 应为 {BACKUP_FORMAT}，实际为 {}",
            manifest.format
        )));
    }
    if manifest.format_version != BACKUP_FORMAT_VERSION {
        return Err(BackupError::Invalid(format!(
            "不支持的备份格式版本：{}（当前支持 {}）",
            manifest.format_version, BACKUP_FORMAT_VERSION
        )));
    }
    for root in FILE_ROOTS {
        if !manifest.sections.contains_key(root) {
            return Err(BackupError::Invalid(format!("备份清单缺少数据段：{root}")));
        }
    }
    for root in DIRECTORY_ROOTS {
        if (root != "audio" || manifest.includes_audio) && !manifest.sections.contains_key(root) {
            return Err(BackupError::Invalid(format!("备份清单缺少数据段：{root}")));
        }
    }
    for (section, meta) in &manifest.sections {
        if !MANAGED_ROOTS.contains(&section.as_str()) {
            return Err(BackupError::Invalid(format!("备份包含未知数据目录：{section}")));
        }
        if FILE_ROOTS.contains(&section.as_str()) && meta.files > 1 {
            return Err(BackupError::Invalid(format!("{section} 的文件数应为 0 或 1，实际为 {}", meta.files)));
        }
        if section == KEY_ROOT && meta.files != 1 {
            return Err(BackupError::Invalid(".encryption_key 的文件数应为 1".into()));
        }
    }
    if manifest.includes_key {
        if !manifest.sections.contains_key(KEY_ROOT) {
            return Err(BackupError::Invalid("manifest 标记包含密钥，但缺少 .encryption_key".into()));
        }
    } else if manifest.sections.contains_key(KEY_ROOT) {
        return Err(BackupError::Invalid("manifest 标记不包含密钥，但备份里出现了 .encryption_key".into()));
    }
    if !manifest.includes_audio && manifest.sections.contains_key("audio") {
        return Err(BackupError::Invalid("manifest 标记不包含录音，但备份里出现了 audio 目录".into()));
    }
    Ok(())
}

fn preflight_archive(archive: &mut ZipArchive<File>, manifest: &BackupManifest) -> Result<(), BackupError> {
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(BackupError::Invalid("备份文件条目过多，已拒绝导入".into()));
    }
    let mut names = HashSet::new();
    let mut actual: BTreeMap<String, SectionManifest> = BTreeMap::new();
    let mut total_bytes: u64 = 0;

    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        let raw_name = entry.name().to_string();
        if !names.insert(raw_name.clone()) {
            return Err(BackupError::Invalid(format!("备份包含重复路径：{raw_name}")));
        }
        if entry.is_symlink() {
            return Err(BackupError::Invalid(format!("备份包含符号链接，已拒绝导入：{raw_name}")));
        }
        if raw_name == MANIFEST_NAME {
            if entry.is_dir() {
                return Err(BackupError::Invalid("manifest.json 必须是普通文件".into()));
            }
            continue;
        }
        let relative = normalize_archive_path(&raw_name)?;
        if should_skip_relative(&relative) {
            continue;
        }
        let top = top_level(&relative)?;
        validate_archive_entry_shape(&relative, &top, entry.is_dir())?;
        if top == KEY_ROOT {
            if relative != KEY_ROOT {
                return Err(BackupError::Invalid(format!("非法密钥路径：{raw_name}")));
            }
            if !manifest.includes_key {
                return Err(BackupError::Invalid("备份不含密钥，但压缩包里出现了 .encryption_key".into()));
            }
        }
        if top == "audio" && !manifest.includes_audio {
            return Err(BackupError::Invalid("备份不含录音，但压缩包里出现了 audio".into()));
        }
        if !MANAGED_ROOTS.contains(&top.as_str()) {
            return Err(BackupError::Invalid(format!("备份包含不支持的路径：{raw_name}")));
        }
        total_bytes = total_bytes.saturating_add(entry.size());
        if total_bytes > MAX_ARCHIVE_BYTES {
            return Err(BackupError::Invalid("备份解压后体积过大，已拒绝导入".into()));
        }
        let section = actual.entry(top).or_default();
        if !entry.is_dir() {
            section.files += 1;
            section.bytes += entry.size();
        }
    }

    for (section, expected) in &manifest.sections {
        match actual.get(section) {
            Some(found) if found == expected => {}
            Some(found) => {
                return Err(BackupError::Invalid(format!(
                    "备份清单与内容不一致：{section} 清单 {} 个文件 / {} 字节，实际 {} 个文件 / {} 字节",
                    expected.files, expected.bytes, found.files, found.bytes
                )));
            }
            None if expected.files == 0 => {}
            None => {
                return Err(BackupError::Invalid(format!("备份缺少清单里的 {section}（{} 个文件）", expected.files)));
            }
        }
    }
    for section in actual.keys() {
        if !manifest.sections.contains_key(section) {
            return Err(BackupError::Invalid(format!("备份内容包含清单外的 {section}")));
        }
    }
    Ok(())
}

fn validate_archive_entry_shape(relative: &str, top: &str, is_dir: bool) -> Result<(), BackupError> {
    if FILE_ROOTS.contains(&top) {
        if relative != top || is_dir {
            return Err(BackupError::Invalid(format!("{top} 必须是一个普通文件，不能包含子路径")));
        }
    } else if DIRECTORY_ROOTS.contains(&top) && relative == top && !is_dir {
        return Err(BackupError::Invalid(format!("{top} 必须是一个目录")));
    }
    Ok(())
}

fn extract_archive(
    archive: &mut ZipArchive<File>,
    stage: &Path,
    _manifest: &BackupManifest,
) -> Result<(), BackupError> {
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let raw_name = entry.name().to_string();
        if raw_name == MANIFEST_NAME {
            continue;
        }
        let relative = normalize_archive_path(&raw_name)?;
        if should_skip_relative(&relative) {
            continue;
        }
        let target = stage.join(&relative);
        if entry.is_dir() {
            fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = OpenOptions::new().write(true).create_new(true).open(&target)?;
        let expected = entry.size();
        let copied = io::copy(&mut entry.by_ref().take(expected + 1), &mut output)?;
        if copied != expected {
            let _ = remove_path(&target);
            return Err(BackupError::Invalid(format!("备份内容损坏：{raw_name} 大小与清单不一致")));
        }
        output.sync_all()?;
        if relative == KEY_ROOT {
            restrict_key_permissions(&target);
        }
    }
    Ok(())
}

/// 校验并规范化 zip 内路径：只允许相对、正斜杠分隔、无 `.` / `..` 的普通路径。
fn normalize_archive_path(raw: &str) -> Result<String, BackupError> {
    let trimmed = raw.trim_end_matches('/');
    if trimmed.is_empty() || raw.contains('\\') || raw.starts_with('/') {
        return Err(BackupError::Invalid(format!("备份包含非法路径：{raw}")));
    }
    let mut parts = Vec::new();
    for part in trimmed.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(BackupError::Invalid(format!("备份包含非法路径：{raw}")));
        }
        parts.push(part);
    }
    let path = parts.iter().collect::<PathBuf>();
    if !path.components().all(|component| matches!(component, Component::Normal(_))) {
        return Err(BackupError::Invalid(format!("备份包含非法路径：{raw}")));
    }
    Ok(relative_slashes(&path))
}

fn top_level(relative: &str) -> Result<String, BackupError> {
    relative
        .split('/')
        .next()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| BackupError::Invalid("备份包含空路径".into()))
}

fn relative_slashes(path: &Path) -> String {
    path.components().filter_map(|component| component.as_os_str().to_str()).collect::<Vec<_>>().join("/")
}

/// 不含密钥的备份：本机密钥解不开的密文字段一律按「未设置」清理。
fn sanitize_credentials_without_key(stage: &Path, crypto: Option<&Crypto>) -> Result<u64, BackupError> {
    let mut reset = 0;
    reset += sanitize_settings_credentials(&stage.join("settings.json"), crypto)?;
    reset += sanitize_llm_credentials(&stage.join("llm_v2.json"), crypto)?;
    Ok(reset)
}

fn decryptable(crypto: Option<&Crypto>, token: &str) -> bool {
    !token.trim().is_empty() && crypto.map(|crypto| crypto.decrypt(token).is_some()).unwrap_or(false)
}

fn sanitize_settings_credentials(path: &Path, crypto: Option<&Crypto>) -> Result<u64, BackupError> {
    let Some(mut value) = read_backup_json(path, "settings.json")? else {
        return Ok(0);
    };
    let Some(object) = value.as_object_mut() else {
        return Err(BackupError::Invalid("备份里的 settings.json 顶层必须是对象".into()));
    };
    let mut reset = 0;
    let mut changed = false;
    for field in ENCRYPTED_SETTINGS_FIELDS {
        match object.get(field) {
            None | Some(Value::Null) => {}
            Some(Value::String(token)) if token.trim().is_empty() => {
                object.remove(field);
                changed = true;
            }
            Some(Value::String(token)) if !decryptable(crypto, token) => {
                object.remove(field);
                changed = true;
                reset += 1;
            }
            Some(Value::String(_)) => {}
            Some(_) => {
                object.remove(field);
                changed = true;
                reset += 1;
            }
        };
    }
    if changed {
        json_store::write_json_atomic(path, &value)?;
    }
    Ok(reset)
}

fn sanitize_llm_credentials(path: &Path, crypto: Option<&Crypto>) -> Result<u64, BackupError> {
    let Some(mut value) = read_backup_json(path, "llm_v2.json")? else {
        return Ok(0);
    };
    let Some(profiles) = value.get_mut("profiles") else {
        return Ok(0);
    };
    let Some(profiles) = profiles.as_array_mut() else {
        return Err(BackupError::Invalid("备份里的 llm_v2.json 的 profiles 必须是数组".into()));
    };
    let mut reset = 0;
    for profile in profiles {
        let Some(object) = profile.as_object_mut() else {
            continue;
        };
        let clear = match object.get("api_key_encrypted") {
            None | Some(Value::Null) => false,
            Some(Value::String(token)) if token.trim().is_empty() => false,
            Some(Value::String(token)) => !decryptable(crypto, token),
            Some(_) => true,
        };
        if !clear {
            continue;
        }
        object.insert("api_key_encrypted".into(), Value::String(String::new()));
        object.insert("api_key_hint".into(), Value::String(String::new()));
        object.insert(
            "status".into(),
            json!({
                "state": "decrypt_failed",
                "checked_at": crate::now_rfc3339(),
                "latency_ms": null,
                "error_code": crate::llm::errors::CODE_AUTH,
                "error": "备份不含加密密钥，已保存的密钥无法解密，请重新填写",
            }),
        );
        reset += 1;
    }
    if reset > 0 {
        json_store::write_json_atomic(path, &value)?;
    }
    Ok(reset)
}

fn create_snapshot(data_dir: &Path) -> io::Result<PathBuf> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let mut snapshot = data_dir.join(format!(".pre-import-{stamp}"));
    let mut suffix = 1;
    while snapshot.exists() {
        snapshot = data_dir.join(format!(".pre-import-{stamp}-{suffix}"));
        suffix += 1;
    }
    fs::create_dir(&snapshot)?;
    let outcome = (|| {
        let mut children = fs::read_dir(data_dir)?.flatten().collect::<Vec<_>>();
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            let name = child.file_name().to_string_lossy().into_owned();
            if is_internal_snapshot_name(&name) {
                continue;
            }
            copy_path(&child.path(), &snapshot.join(name.as_str()))?;
        }
        Ok(())
    })();
    if let Err(error) = outcome {
        let _ = remove_path(&snapshot);
        return Err(error);
    }
    Ok(snapshot)
}

fn is_internal_snapshot_name(name: &str) -> bool {
    name.starts_with(".pre-import-") || name.starts_with(".backup-import-")
}

/// 导入前先确认密钥文件本身合法，避免恢复后应用下次启动才因密钥损坏失败。
fn validate_extracted_key(stage: &Path) -> Result<(), BackupError> {
    let path = stage.join(KEY_ROOT);
    let size = fs::metadata(&path)?.len();
    if size > MAX_KEY_BYTES {
        return Err(BackupError::Invalid(".encryption_key 异常过大，已拒绝导入".into()));
    }
    let raw = fs::read(&path)?;
    let key = std::str::from_utf8(&raw)
        .map_err(|_| BackupError::Invalid("备份里的 .encryption_key 不是合法的 Fernet 密钥".into()))?;
    if Fernet::new(key).is_none() {
        return Err(BackupError::Invalid("备份里的 .encryption_key 不是合法的 Fernet 密钥".into()));
    }
    Ok(())
}

/// 包含密钥时必须确认密钥确实能解开备份中的凭据，避免把不匹配的包覆盖到本机后再暴露问题。
fn validate_encrypted_credentials_with_key(stage: &Path) -> Result<(), BackupError> {
    let key = fs::read_to_string(stage.join(KEY_ROOT))?;
    let fernet = Fernet::new(key.trim())
        .ok_or_else(|| BackupError::Invalid("备份里的 .encryption_key 不是合法的 Fernet 密钥".into()))?;

    if let Some(value) = read_backup_json(&stage.join("settings.json"), "settings.json")? {
        if let Some(object) = value.as_object() {
            for field in ENCRYPTED_SETTINGS_FIELDS {
                match object.get(field) {
                    None | Some(Value::Null) => {}
                    Some(Value::String(token)) if token.trim().is_empty() => {}
                    Some(Value::String(token)) if fernet.decrypt(token).is_ok() => {}
                    Some(_) => {
                        return Err(BackupError::Invalid(format!(
                            "备份密钥与 settings.json 的 {field} 不匹配，已拒绝导入"
                        )));
                    }
                }
            }
        }
    }

    if let Some(value) = read_backup_json(&stage.join("llm_v2.json"), "llm_v2.json")? {
        let profiles = value.get("profiles").map(|profiles| {
            profiles.as_array().ok_or_else(|| BackupError::Invalid("llm_v2.json 的 profiles 必须是数组".into()))
        });
        if let Some(profiles) = profiles {
            for (index, profile) in profiles?.iter().enumerate() {
                let Some(token) = profile.get("api_key_encrypted") else {
                    continue;
                };
                match token {
                    Value::Null => {}
                    Value::String(token) if token.trim().is_empty() => {}
                    Value::String(token) if fernet.decrypt(token).is_ok() => {}
                    _ => {
                        return Err(BackupError::Invalid(format!(
                            "备份密钥与 llm_v2.json 第 {} 个档案的密钥不匹配，已拒绝导入",
                            index + 1
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

const ENCRYPTED_SETTINGS_FIELDS: [&str; 3] =
    ["doubao_asr_api_key_encrypted", "chatgpt_asr_api_key_encrypted", "deepseek_asr_api_key_encrypted"];

fn read_backup_json(path: &Path, label: &str) -> Result<Option<Value>, BackupError> {
    if !path.is_file() {
        return Ok(None);
    }
    if fs::metadata(path)?.len() > MAX_DOCUMENT_BYTES {
        return Err(BackupError::Invalid(format!("备份里的 {label} 异常过大，已拒绝导入")));
    }
    let raw = fs::read_to_string(path)?;
    serde_json::from_str(&raw)
        .map(Some)
        .map_err(|error| BackupError::Invalid(format!("备份里的 {label} 格式错误：{error}")))
}

fn apply_import(
    data_dir: &Path,
    stage: &Path,
    manifest: &BackupManifest,
    restore_audio: bool,
    fail_after_sections: Option<usize>,
) -> Result<(), BackupError> {
    if !restore_audio {
        remove_path(&data_dir.join("audio"))?;
    }
    for (processed, (section, expected)) in manifest.sections.iter().enumerate() {
        let target = data_dir.join(section);
        remove_path(&target)?;
        let source = stage.join(section);
        if source.exists() {
            fs::rename(&source, &target)?;
        } else if expected.files == 0 {
            if DIRECTORY_ROOTS.contains(&section.as_str()) {
                fs::create_dir_all(&target)?;
            }
        } else {
            return Err(BackupError::Invalid(format!("备份缺少 {section} 的解压内容")));
        }
        if section == KEY_ROOT {
            restrict_key_permissions(&target);
        }
        if fail_after_sections == Some(processed + 1) {
            return Err(io::Error::other("测试注入：导入中途失败").into());
        }
    }
    Ok(())
}

fn rollback_import(data_dir: &Path, snapshot: &Path, touched: &[String]) -> io::Result<()> {
    for section in touched {
        let target = data_dir.join(section);
        remove_path(&target)?;
        let source = snapshot.join(section);
        if source.exists() {
            copy_path(&source, &target)?;
        }
    }
    Ok(())
}

fn remove_old_snapshots(data_dir: &Path, keep: &Path) {
    let Ok(entries) = fs::read_dir(data_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if name.starts_with(".pre-import-") && path != keep {
            let _ = remove_path(&path);
        }
    }
}

fn copy_path(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("导入前快照不支持符号链接：{}", source.display()),
        ));
    }
    if metadata.is_dir() {
        fs::create_dir_all(destination)?;
        let mut children = fs::read_dir(source)?.flatten().collect::<Vec<_>>();
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            copy_path(&child.path(), &destination.join(child.file_name()))?;
        }
        Ok(())
    } else if metadata.is_file() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, destination)?;
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::Unsupported, format!("导入前快照不支持特殊文件：{}", source.display())))
    }
}

fn remove_path(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn restrict_key_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(path) {
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o600);
            let _ = fs::set_permissions(path, permissions);
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn restrict_backup_file_permissions(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(path, permissions)?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::fernet::Fernet;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("att-backup-{label}-{}", uuid::Uuid::new_v4()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, content: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn seed_data(dir: &Path) {
        write(&dir.join("settings.json"), r#"{"asr_provider":"doubao_asr","asr_hotwords":["甲"]}"#.as_bytes());
        write(&dir.join("records/r1.json"), r#"{"id":"r1","raw_text":"你好"}"#.as_bytes());
        write(&dir.join("audio/records/r1.flac"), b"flac-bytes");
        write(&dir.join("audio/temp/recording.wav"), b"temp");
        write(&dir.join("clipboard/items/c1.json"), br#"{"id":"c1"}"#);
        write(&dir.join("clipboard/images/c1.png"), b"png");
        write(&dir.join("llm_v2.json"), br#"{"version":2,"profiles":[],"models":[]}"#);
        write(&dir.join("custom_prompts.json"), br#"{"presets":[]}"#);
        write(&dir.join("transcribe_file_cache.json"), br#"{"v1":{}}"#);
        write(&dir.join("logs/att-app.log"), b"private log");
        write(&dir.join("settings.json.tmp"), b"temp json");
        Crypto::load_or_create(dir).unwrap();
    }

    fn read_zip(path: &Path) -> (Value, Vec<String>) {
        let file = File::open(path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        let names = archive.file_names().map(str::to_string).collect::<Vec<_>>();
        let manifest: Value = {
            let mut entry = archive.by_name(MANIFEST_NAME).unwrap();
            let mut raw = String::new();
            entry.read_to_string(&mut raw).unwrap();
            serde_json::from_str(&raw).unwrap()
        };
        (manifest, names)
    }

    fn empty_sections(include_audio: bool, include_key: bool) -> serde_json::Map<String, Value> {
        let mut sections = serde_json::Map::new();
        for root in FILE_ROOTS {
            sections.insert(root.into(), json!({"files": 0, "bytes": 0}));
        }
        for root in DIRECTORY_ROOTS {
            if root != "audio" || include_audio {
                sections.insert(root.into(), json!({"files": 0, "bytes": 0}));
            }
        }
        if include_key {
            sections.insert(KEY_ROOT.into(), json!({"files": 1, "bytes": 3}));
        }
        sections
    }

    fn manifest_json(include_audio: bool, include_key: bool) -> Value {
        json!({
            "format": BACKUP_FORMAT,
            "format_version": 1,
            "created_at": "2026-11-21T00:00:00Z",
            "includes_key": include_key,
            "includes_audio": include_audio,
            "sections": empty_sections(include_audio, include_key),
        })
    }

    #[test]
    fn 导出清单_文件数_排除日志临时目录且可选密钥() {
        let dir = temp_dir("manifest");
        seed_data(&dir);
        let zip = dir.join("att-backup.zip");
        let result =
            export_backup(&dir, "2.9.0", &zip, ExportOptions { include_key: true, include_audio: true }).unwrap();
        assert!(zip.is_file());
        assert!(result.bytes > 0);

        let (manifest, names) = read_zip(&zip);
        assert_eq!(manifest["format"], json!(BACKUP_FORMAT));
        assert_eq!(manifest["format_version"], json!(1));
        assert_eq!(manifest["app_version"], json!("2.9.0"));
        assert_eq!(manifest["includes_key"], json!(true));
        assert_eq!(manifest["includes_audio"], json!(true));
        let record_bytes = fs::metadata(dir.join("records/r1.json")).unwrap().len();
        assert_eq!(manifest["sections"]["records"], json!({"files": 1, "bytes": record_bytes}));
        assert_eq!(manifest["sections"]["audio"]["files"], json!(1));
        assert_eq!(manifest["sections"][".encryption_key"]["files"], json!(1));
        assert!(!names.iter().any(|name| name.contains("logs/")), "日志不能进备份");
        assert!(!names.iter().any(|name| name.contains("audio/temp")), "临时录音不能进备份");
        assert!(!names.iter().any(|name| name.ends_with(".tmp")), "临时文件不能进备份");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 导入校验_缺清单_路径穿越与损坏包都不改动原数据() {
        let source = temp_dir("bad-source");
        let target = temp_dir("bad-target");
        write(&target.join("records/keep.json"), b"keep");

        let missing_manifest = source.join("missing-manifest.zip");
        let file = File::create(&missing_manifest).unwrap();
        let mut writer = ZipWriter::new(file);
        writer.start_file("settings.json", SimpleFileOptions::default()).unwrap();
        writer.write_all(b"{}").unwrap();
        writer.finish().unwrap();
        let error = import_backup(&target, &missing_manifest, None).unwrap_err();
        assert!(error.to_string().contains("manifest.json"));
        assert_eq!(fs::read(target.join("records/keep.json")).unwrap(), b"keep");

        let traversal = source.join("traversal.zip");
        let file = File::create(&traversal).unwrap();
        let mut writer = ZipWriter::new(file);
        let manifest = manifest_json(false, false);
        writer.start_file(MANIFEST_NAME, SimpleFileOptions::default()).unwrap();
        writer.write_all(serde_json::to_string(&manifest).unwrap().as_bytes()).unwrap();
        writer.start_file("../evil.json", SimpleFileOptions::default()).unwrap();
        writer.write_all(b"evil").unwrap();
        writer.finish().unwrap();
        let error = import_backup(&target, &traversal, None).unwrap_err();
        assert!(error.to_string().contains("非法路径"));
        assert!(!source.join("evil.json").exists());
        assert_eq!(fs::read(target.join("records/keep.json")).unwrap(), b"keep");

        let valid = source.join("valid.zip");
        export_backup(&source, "2.9.0", &valid, ExportOptions { include_key: false, include_audio: false }).unwrap();
        let mut bytes = fs::read(&valid).unwrap();
        bytes.truncate(bytes.len() - 12);
        let corrupt = source.join("corrupt.zip");
        fs::write(&corrupt, bytes).unwrap();
        assert!(import_backup(&target, &corrupt, None).is_err());
        assert_eq!(fs::read(target.join("records/keep.json")).unwrap(), b"keep");
        let _ = fs::remove_dir_all(&source);
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn 导入恢复_覆盖数据_留存最近一份导入前快照() {
        let source = temp_dir("restore-source");
        seed_data(&source);
        let zip = source.join("att-backup.zip");
        export_backup(&source, "2.9.0", &zip, ExportOptions { include_key: false, include_audio: true }).unwrap();

        let target = temp_dir("restore-target");
        write(&target.join("records/old.json"), b"old");
        write(&target.join("settings.json"), br#"{"asr_provider":"qwen_asr"}"#);
        let original_key = Crypto::load_or_create(&target).unwrap();
        let original_key_text = fs::read_to_string(original_key.key_path()).unwrap();

        let result = import_backup(&target, &zip, Some(&original_key)).unwrap();
        assert_eq!(result.credentials_reset, 0);
        assert!(result.restart_required);
        assert!(!target.join("records/old.json").exists());
        assert_eq!(fs::read(target.join("records/r1.json")).unwrap(), r#"{"id":"r1","raw_text":"你好"}"#.as_bytes());
        assert_eq!(fs::read(target.join("audio/records/r1.flac")).unwrap(), b"flac-bytes");
        assert_eq!(fs::read_to_string(original_key.key_path()).unwrap(), original_key_text);
        let snapshot = PathBuf::from(&result.pre_import_backup);
        assert_eq!(snapshot.parent(), Some(target.as_path()));
        assert!(snapshot.file_name().unwrap().to_string_lossy().starts_with(".pre-import-"));
        assert_eq!(fs::read(snapshot.join("records/old.json")).unwrap(), b"old");

        // 第二次导入仍只保留一份导入前快照
        import_backup(&target, &zip, Some(&original_key)).unwrap();
        let snapshots = fs::read_dir(&target)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".pre-import-"))
            .count();
        assert_eq!(snapshots, 1);
        let _ = fs::remove_dir_all(&source);
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn 不含密钥导入_解不开的凭据显示未设置并提示重填() {
        let source = temp_dir("nokey-source");
        let source_crypto = Crypto::load_or_create(&source).unwrap();
        let doubao_token = source_crypto.encrypt("doubao-secret");
        let llm_token = source_crypto.encrypt("sk-secret");
        write(
            &source.join("settings.json"),
            json!({
                "asr_provider": "doubao_asr",
                "asr_hotwords": ["保留"],
                "doubao_asr_api_key_encrypted": doubao_token,
            })
            .to_string()
            .as_bytes(),
        );
        write(
            &source.join("llm_v2.json"),
            json!({
                "version": 2,
                "profiles": [{
                    "id": "p1",
                    "provider": "deepseek",
                    "name": "DS",
                    "api_key_encrypted": llm_token,
                    "api_key_hint": "sk-****cret",
                    "status": {"state": "ok"}
                }],
                "models": []
            })
            .to_string()
            .as_bytes(),
        );
        let zip = source.join("no-key.zip");
        export_backup(&source, "2.9.0", &zip, ExportOptions { include_key: false, include_audio: false }).unwrap();

        let target = temp_dir("nokey-target");
        let target_crypto = Crypto::load_or_create(&target).unwrap();
        let result = import_backup(&target, &zip, Some(&target_crypto)).unwrap();
        assert_eq!(result.credentials_reset, 2);

        let settings: Value = serde_json::from_str(&fs::read_to_string(target.join("settings.json")).unwrap()).unwrap();
        assert!(settings.get("doubao_asr_api_key_encrypted").is_none());
        assert_eq!(settings["asr_hotwords"], json!(["保留"]));
        let llm: Value = serde_json::from_str(&fs::read_to_string(target.join("llm_v2.json")).unwrap()).unwrap();
        assert_eq!(llm["profiles"][0]["api_key_encrypted"], json!(""));
        assert_eq!(llm["profiles"][0]["api_key_hint"], json!(""));
        assert_eq!(llm["profiles"][0]["status"]["state"], json!("decrypt_failed"));
        assert!(llm["profiles"][0]["status"]["error"].as_str().unwrap().contains("重新填写"));

        let core = crate::state::CoreState::init(&target, crate::Platform::Macos).unwrap();
        let public = core.public_settings();
        assert_eq!(public["doubao_asr_api_key_set"], json!(false));
        assert_eq!(public["doubao_web_timeout_seconds"], json!(30), "旧备份恢复后补齐新设置默认值");
        assert!(public.get("doubao_web_asr_api_key_set").is_none(), "匿名引擎没有凭据字段");
        assert!(core.llm.list_profiles()[0].api_key_encrypted.is_empty());
        let _ = fs::remove_dir_all(&source);
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn 含密钥导入_替换密钥文件() {
        let source = temp_dir("key-source");
        seed_data(&source);
        let zip = source.join("with-key.zip");
        export_backup(&source, "2.9.0", &zip, ExportOptions { include_key: true, include_audio: false }).unwrap();
        let source_key = fs::read_to_string(source.join(KEY_ROOT)).unwrap();

        let target = temp_dir("key-target");
        let target_crypto = Crypto::load_or_create(&target).unwrap();
        assert_ne!(fs::read_to_string(target_crypto.key_path()).unwrap(), source_key);
        import_backup(&target, &zip, Some(&target_crypto)).unwrap();
        assert_eq!(fs::read_to_string(target.join(KEY_ROOT)).unwrap(), source_key);
        let reloaded = Crypto::load_or_create(&target).unwrap();
        assert_eq!(reloaded.decrypt(&reloaded.encrypt("回程")).as_deref(), Some("回程"));
        let _ = fs::remove_dir_all(&source);
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn 未设置的空凭据不影响含密钥导入且不计入无密钥重置数量() {
        let source = temp_dir("null-credentials-source");
        Crypto::load_or_create(&source).unwrap();
        write(
            &source.join("settings.json"),
            json!({
                "doubao_asr_api_key_encrypted": null,
                "chatgpt_asr_api_key_encrypted": "",
                "deepseek_asr_api_key_encrypted": null,
            })
            .to_string()
            .as_bytes(),
        );
        write(
            &source.join("llm_v2.json"),
            json!({
                "version": 2,
                "profiles": [{"id": "p1", "provider": "deepseek", "name": "DS", "api_key_encrypted": null}],
                "models": [],
            })
            .to_string()
            .as_bytes(),
        );

        let with_key = source.join("with-key.zip");
        export_backup(&source, "2.9.0", &with_key, ExportOptions { include_key: true, include_audio: false }).unwrap();
        let keyed_target = temp_dir("null-credentials-keyed-target");
        let keyed_crypto = Crypto::load_or_create(&keyed_target).unwrap();
        let result = import_backup(&keyed_target, &with_key, Some(&keyed_crypto)).unwrap();
        assert_eq!(result.credentials_reset, 0);

        let without_key = source.join("without-key.zip");
        export_backup(&source, "2.9.0", &without_key, ExportOptions { include_key: false, include_audio: false })
            .unwrap();
        let plain_target = temp_dir("null-credentials-plain-target");
        let plain_crypto = Crypto::load_or_create(&plain_target).unwrap();
        let result = import_backup(&plain_target, &without_key, Some(&plain_crypto)).unwrap();
        assert_eq!(result.credentials_reset, 0);
        let settings: Value =
            serde_json::from_str(&fs::read_to_string(plain_target.join("settings.json")).unwrap()).unwrap();
        assert!(settings.get("chatgpt_asr_api_key_encrypted").is_none());
        let llm: Value = serde_json::from_str(&fs::read_to_string(plain_target.join("llm_v2.json")).unwrap()).unwrap();
        assert_eq!(llm["profiles"][0]["api_key_encrypted"], Value::Null);
        let core = crate::state::CoreState::init(&plain_target, crate::Platform::Macos).unwrap();
        assert_eq!(core.public_settings()["chatgpt_asr_api_key_set"], json!(false));

        let _ = fs::remove_dir_all(&source);
        let _ = fs::remove_dir_all(&keyed_target);
        let _ = fs::remove_dir_all(&plain_target);
    }

    #[test]
    fn 含密钥导入_密钥非法时拒绝且不改动原数据() {
        let source = temp_dir("bad-key-source");
        let zip = source.join("bad-key.zip");
        let manifest = manifest_json(false, true);
        let file = File::create(&zip).unwrap();
        let mut writer = ZipWriter::new(file);
        writer.start_file(MANIFEST_NAME, SimpleFileOptions::default()).unwrap();
        writer.write_all(serde_json::to_string(&manifest).unwrap().as_bytes()).unwrap();
        writer.start_file(KEY_ROOT, SimpleFileOptions::default()).unwrap();
        writer.write_all(b"bad").unwrap();
        writer.finish().unwrap();

        let target = temp_dir("bad-key-target");
        write(&target.join("records/keep.json"), b"keep");
        let original_key = Crypto::load_or_create(&target).unwrap();
        let original_key_text = fs::read_to_string(original_key.key_path()).unwrap();

        let error = import_backup(&target, &zip, Some(&original_key)).unwrap_err();
        assert!(error.to_string().contains("Fernet"));
        assert_eq!(fs::read(target.join("records/keep.json")).unwrap(), b"keep");
        assert_eq!(fs::read_to_string(target.join(KEY_ROOT)).unwrap(), original_key_text);
        let _ = fs::remove_dir_all(&source);
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn 不含录音导入_清理旧录音并保留在导入前快照() {
        let source = temp_dir("no-audio-source");
        seed_data(&source);
        let zip = source.join("no-audio.zip");
        export_backup(&source, "2.9.0", &zip, ExportOptions { include_key: false, include_audio: false }).unwrap();

        let target = temp_dir("no-audio-target");
        write(&target.join("records/old.json"), b"old");
        write(&target.join("audio/records/old.flac"), b"old-audio");
        write(&target.join("audio/temp/old.wav"), b"old-temp");
        let target_crypto = Crypto::load_or_create(&target).unwrap();

        let result = import_backup(&target, &zip, Some(&target_crypto)).unwrap();
        assert!(!target.join("audio").exists(), "备份不含录音时不能保留旧录音");
        assert!(target.join("records/r1.json").is_file());
        let snapshot = PathBuf::from(&result.pre_import_backup);
        assert_eq!(fs::read(snapshot.join("audio/records/old.flac")).unwrap(), b"old-audio");
        assert_eq!(fs::read(snapshot.join("audio/temp/old.wav")).unwrap(), b"old-temp");
        let _ = fs::remove_dir_all(&source);
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn 导出的密钥与清单格式可被独立解析() {
        let dir = temp_dir("fernet");
        seed_data(&dir);
        let zip = dir.join("one.zip");
        export_backup(&dir, "2.9.0", &zip, ExportOptions { include_key: true, include_audio: false }).unwrap();
        let file = File::open(&zip).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        let mut entry = archive.by_name(KEY_ROOT).unwrap();
        let mut key = String::new();
        entry.read_to_string(&mut key).unwrap();
        assert!(Fernet::new(&key).is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 导出路径不存在时可登记且授权只能消费一次() {
        let dir = temp_dir("grants");
        let export = dir.join("backup.zip");
        let grants = BackupPathGrants::default();
        assert!(grants.authorize_export(&export));
        assert!(grants.authorize_export(&export));
        assert!(grants.consume_export(&export));
        assert!(!grants.consume_export(&export));
        assert!(!grants.consume_import(&export));
        assert!(!grants.authorize_import(Path::new("relative.zip")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 导出覆盖已有备份文件() {
        let dir = temp_dir("overwrite");
        seed_data(&dir);
        let zip = dir.join("att-backup.zip");
        fs::write(&zip, b"old backup").unwrap();

        let result =
            export_backup(&dir, "2.9.0", &zip, ExportOptions { include_key: false, include_audio: true }).unwrap();
        assert!(result.bytes > b"old backup".len() as u64);
        let (manifest, _) = read_zip(&zip);
        assert_eq!(manifest["format"], json!(BACKUP_FORMAT));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 导入校验_文件根不能伪装成包含子路径的目录() {
        let source = temp_dir("wrong-root-type-source");
        let target = temp_dir("wrong-root-type-target");
        write(&target.join("settings.json"), br#"{"keep":true}"#);
        let zip = source.join("wrong-root-type.zip");
        let file = File::create(&zip).unwrap();
        let mut writer = ZipWriter::new(file);
        writer.start_file(MANIFEST_NAME, SimpleFileOptions::default()).unwrap();
        writer.write_all(serde_json::to_string(&manifest_json(false, false)).unwrap().as_bytes()).unwrap();
        writer.add_directory("settings.json/", SimpleFileOptions::default()).unwrap();
        writer.start_file("settings.json/evil.json", SimpleFileOptions::default()).unwrap();
        writer.write_all(b"{}").unwrap();
        writer.finish().unwrap();

        let error = import_backup(&target, &zip, None).unwrap_err();
        assert!(error.to_string().contains("普通文件"));
        assert_eq!(fs::read(target.join("settings.json")).unwrap(), br#"{"keep":true}"#);
        let _ = fs::remove_dir_all(&source);
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn 含密钥导入_密钥与密文不匹配时拒绝覆盖() {
        let source = temp_dir("mismatch-source");
        let old_key = Crypto::load_or_create(&source).unwrap();
        write(
            &source.join("settings.json"),
            json!({"doubao_asr_api_key_encrypted": old_key.encrypt("secret")}).to_string().as_bytes(),
        );
        fs::write(source.join(KEY_ROOT), Fernet::generate_key()).unwrap();
        let zip = source.join("mismatch.zip");
        export_backup(&source, "2.9.0", &zip, ExportOptions { include_key: true, include_audio: false }).unwrap();

        let target = temp_dir("mismatch-target");
        write(&target.join("records/keep.json"), b"keep");
        let target_crypto = Crypto::load_or_create(&target).unwrap();
        let error = import_backup(&target, &zip, Some(&target_crypto)).unwrap_err();
        assert!(error.to_string().contains("不匹配"), "{error}");
        assert_eq!(fs::read(target.join("records/keep.json")).unwrap(), b"keep");
        let _ = fs::remove_dir_all(&source);
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn 导入空备份会清空所有受管数据段并保留本机密钥() {
        let source = temp_dir("empty-source");
        let zip = source.join("empty.zip");
        export_backup(&source, "2.9.0", &zip, ExportOptions { include_key: false, include_audio: true }).unwrap();

        let target = temp_dir("empty-target");
        for path in [
            "settings.json",
            "llm_v2.json",
            "custom_prompts.json",
            "transcribe_file_cache.json",
            "records/old.json",
            "audio/records/old.flac",
            "clipboard/items/old.json",
        ] {
            write(&target.join(path), b"old");
        }
        let key = Crypto::load_or_create(&target).unwrap();
        let key_text = fs::read_to_string(key.key_path()).unwrap();

        import_backup(&target, &zip, Some(&key)).unwrap();
        for path in [
            "settings.json",
            "llm_v2.json",
            "custom_prompts.json",
            "transcribe_file_cache.json",
            "records/old.json",
            "audio/records/old.flac",
            "clipboard/items/old.json",
        ] {
            assert!(!target.join(path).exists(), "空备份必须清掉旧路径 {path}");
        }
        for dir in DIRECTORY_ROOTS {
            assert!(target.join(dir).is_dir(), "{dir} 应在恢复后存在为空目录");
        }
        assert_eq!(fs::read_to_string(key.key_path()).unwrap(), key_text);
        let _ = fs::remove_dir_all(&source);
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn 导入中途失败会从快照完整回滚() {
        let source = temp_dir("rollback-source");
        seed_data(&source);
        let zip = source.join("rollback.zip");
        export_backup(&source, "2.9.0", &zip, ExportOptions { include_key: false, include_audio: true }).unwrap();

        let target = temp_dir("rollback-target");
        write(&target.join("settings.json"), br#"{"asr_provider":"qwen_asr"}"#);
        write(&target.join("records/keep.json"), b"keep");
        write(&target.join("audio/records/keep.flac"), b"keep-audio");
        write(&target.join("clipboard/items/keep.json"), b"keep-clipboard");
        let key = Crypto::load_or_create(&target).unwrap();

        let error = import_backup_inner(&target, &zip, Some(&key), Some(1)).unwrap_err();
        assert!(matches!(error, BackupError::RolledBack { .. }), "{error}");
        assert_eq!(fs::read(target.join("settings.json")).unwrap(), br#"{"asr_provider":"qwen_asr"}"#);
        assert_eq!(fs::read(target.join("records/keep.json")).unwrap(), b"keep");
        assert_eq!(fs::read(target.join("audio/records/keep.flac")).unwrap(), b"keep-audio");
        assert_eq!(fs::read(target.join("clipboard/items/keep.json")).unwrap(), b"keep-clipboard");
        let _ = fs::remove_dir_all(&source);
        let _ = fs::remove_dir_all(&target);
    }

    #[cfg(unix)]
    #[test]
    fn 导出不跟随顶层数据目录符号链接() {
        use std::os::unix::fs::symlink;

        let data = temp_dir("symlink-data");
        let outside = temp_dir("symlink-outside");
        write(&outside.join("secret.json"), b"outside");
        symlink(&outside, data.join("records")).unwrap();
        let zip = temp_dir("symlink-output").join("backup.zip");
        let error = export_backup(&data, "2.9.0", &zip, ExportOptions { include_key: false, include_audio: false })
            .unwrap_err();
        assert!(error.to_string().contains("符号链接"), "{error}");
        assert!(!zip.exists());
        let _ = fs::remove_dir_all(&data);
        let _ = fs::remove_dir_all(&outside);
        let _ = fs::remove_dir_all(zip.parent().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn 导出文件权限收紧为仅当前用户可读写() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("permissions");
        seed_data(&dir);
        let zip = dir.join("private.zip");
        export_backup(&dir, "2.9.0", &zip, ExportOptions { include_key: true, include_audio: false }).unwrap();
        assert_eq!(fs::metadata(&zip).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = fs::remove_dir_all(&dir);
    }
}
