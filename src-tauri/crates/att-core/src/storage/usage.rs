//! 数据占用统计（设置 → 诊断 →「存储」）：录音音频、记录、剪贴板、日志、孤儿音频、本地模型各占多少。
//!
//! 统计要整目录遍历（模型目录可能有几个 GB 的大文件，但文件数不多），调用方放到阻塞线程；
//! 结果缓存 1 分钟，清理孤儿音频、按保留期删音频后作废缓存。

use crate::state::CoreState;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 统计结果的缓存时长。
pub const CACHE_TTL: Duration = Duration::from_secs(60);

/// 一项占用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UsageItem {
    pub key: &'static str,
    pub label: &'static str,
    pub path: String,
    pub files: u64,
    pub bytes: u64,
}

/// 一次统计的结果。
#[derive(Debug, Clone, Serialize)]
pub struct UsageReport {
    /// 数据目录（界面据此把目录内的路径显示成相对路径）
    pub data_dir: String,
    pub items: Vec<UsageItem>,
    pub total_bytes: u64,
    /// 统计完成的时间（RFC3339）
    pub measured_at: String,
}

/// 统计结果缓存（只在内存里）。
#[derive(Default)]
pub struct UsageCache {
    slot: Mutex<Option<(Instant, UsageReport)>>,
}

impl UsageCache {
    /// 1 分钟内统计过就直接用；`force` 时重新统计。阻塞（整目录遍历）。
    pub fn get_or_measure(&self, core: &CoreState, force: bool) -> UsageReport {
        if !force {
            let slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((_, report)) = slot.as_ref().filter(|(at, _)| at.elapsed() < CACHE_TTL) {
                return report.clone();
            }
        }
        let report = measure(core);
        *self.slot.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), report.clone()));
        report
    }

    /// 文件有增删（清理孤儿音频、按保留期删音频）后作废，下次重新统计。
    pub fn invalidate(&self) {
        *self.slot.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// 孤儿音频目录：一次性迁移把没有记录认领的录音隔离在这里（只隔离不删除）。
pub fn orphan_audio_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("audio").join("orphans")
}

/// 统计各项占用（阻塞）。
pub fn measure(core: &CoreState) -> UsageReport {
    let data_dir = &core.data_dir;
    let entries: [(&'static str, &'static str, PathBuf); 6] = [
        ("audio", "录音音频", core.records.audio_dir().to_path_buf()),
        ("records", "记录", core.records.records_dir().to_path_buf()),
        ("clipboard", "剪贴板（含图片）", data_dir.join("clipboard")),
        ("logs", "运行日志", data_dir.join("logs")),
        ("orphans", "孤儿音频", orphan_audio_dir(data_dir)),
        ("models", "本地模型", core.models_dir.clone()),
    ];
    let items: Vec<UsageItem> = entries
        .into_iter()
        .map(|(key, label, path)| {
            let (files, bytes) = dir_usage(&path);
            UsageItem { key, label, path: path.display().to_string(), files, bytes }
        })
        .collect();
    let total_bytes = items.iter().map(|item| item.bytes).sum();
    UsageReport { data_dir: data_dir.display().to_string(), items, total_bytes, measured_at: crate::now_rfc3339() }
}

/// 目录下（递归）普通文件的个数与字节数；不跟随符号链接，目录不存在算 0。
pub fn dir_usage(dir: &Path) -> (u64, u64) {
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else { continue };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else { continue };
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                files += 1;
                bytes += entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            }
        }
    }
    (files, bytes)
}

/// 一次「清理孤儿音频」的结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct OrphanCleanup {
    /// 删掉的文件数
    pub deleted: u64,
    /// 删掉的字节数
    pub bytes: u64,
    /// 仍被记录引用、已移回录音目录的文件数
    pub restored: u64,
    /// 仍被记录引用、但录音目录里已有同名文件：两份都不动，留在孤儿目录
    pub kept: u64,
}

/// 清理孤儿目录：仍被记录引用的文件移回 `audio_dir`，其余删除，目录本身保留。
///
/// `referenced` 是全部记录 `normalize_audio_rel(audio_path)` 的集合（相对 `audio_dir` 的文件名）。
/// 孤儿是迁移时判定的，解析失败的记录会被跳过、它的录音被当成孤儿隔离（发生过 886 个 WAV 整批误隔离）；
/// 记录后来修好或从备份恢复后，删除前必须再核对一次引用，否则会永久删掉仍有记录指向的录音。
pub fn clear_orphan_files(
    dir: &Path,
    audio_dir: &Path,
    referenced: &HashSet<String>,
) -> std::io::Result<OrphanCleanup> {
    let mut result = OrphanCleanup::default();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        let entries = match std::fs::read_dir(&current) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else { continue };
            if file_type.is_dir() {
                pending.push(entry.path());
                continue;
            }
            if !(file_type.is_file() || file_type.is_symlink()) {
                continue;
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if referenced.contains(&name) {
                let target = audio_dir.join(&name);
                if target.exists() {
                    result.kept += 1;
                } else {
                    std::fs::create_dir_all(audio_dir)?;
                    std::fs::rename(&path, &target)?;
                    result.restored += 1;
                }
                continue;
            }
            let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            std::fs::remove_file(&path)?;
            result.deleted += 1;
            result.bytes += size;
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 目录占用_递归计数_不存在算0() {
        let dir = std::env::temp_dir().join(format!("att-usage-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.bin"), vec![0u8; 1000]).unwrap();
        std::fs::write(dir.join("sub").join("b.bin"), vec![0u8; 24]).unwrap();
        assert_eq!(dir_usage(&dir), (2, 1024));
        assert_eq!(dir_usage(&dir.join("missing")), (0, 0));

        let none = HashSet::new();
        let cleared = clear_orphan_files(&dir, &dir.join("records"), &none).unwrap();
        assert_eq!((cleared.deleted, cleared.bytes, cleared.restored), (2, 1024, 0));
        assert_eq!(dir_usage(&dir), (0, 0));
        assert!(dir.is_dir(), "目录本身保留");
        assert_eq!(clear_orphan_files(&dir.join("missing"), &dir, &none).unwrap(), OrphanCleanup::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 清理孤儿_仍被引用的移回_其余删除() {
        let dir = std::env::temp_dir().join(format!("att-orphans-{}", uuid::Uuid::new_v4()));
        let (orphans, audio) = (dir.join("orphans"), dir.join("records"));
        std::fs::create_dir_all(orphans.join("sub")).unwrap();
        std::fs::create_dir_all(&audio).unwrap();
        std::fs::write(orphans.join("a.wav"), vec![1u8; 100]).unwrap();
        std::fs::write(orphans.join("dup.wav"), vec![2u8; 10]).unwrap();
        std::fs::write(audio.join("dup.wav"), vec![3u8; 20]).unwrap();
        std::fs::write(orphans.join("old.wav"), vec![0u8; 1000]).unwrap();
        std::fs::write(orphans.join("sub").join("x.wav"), vec![0u8; 24]).unwrap();
        let referenced: HashSet<String> = ["a.wav", "dup.wav"].into_iter().map(String::from).collect();

        let result = clear_orphan_files(&orphans, &audio, &referenced).unwrap();
        assert_eq!(result, OrphanCleanup { deleted: 2, bytes: 1024, restored: 1, kept: 1 });
        assert_eq!(std::fs::read(audio.join("a.wav")).unwrap(), vec![1u8; 100], "被引用的移回录音目录");
        assert_eq!(std::fs::read(audio.join("dup.wav")).unwrap(), vec![3u8; 20], "录音目录里的同名文件不被覆盖");
        assert!(orphans.join("dup.wav").is_file(), "同名冲突时孤儿副本也不删");
        assert!(!orphans.join("old.wav").exists() && !orphans.join("sub").join("x.wav").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 统计各项_缓存一分钟_作废后重算() {
        let dir = std::env::temp_dir().join(format!("att-usage-core-{}", uuid::Uuid::new_v4()));
        let core = CoreState::init(&dir, crate::Platform::current()).unwrap();
        std::fs::write(core.records.audio_dir().join("x.flac"), vec![0u8; 300]).unwrap();
        std::fs::create_dir_all(orphan_audio_dir(&dir)).unwrap();
        std::fs::write(orphan_audio_dir(&dir).join("old.wav"), vec![0u8; 50]).unwrap();

        let cache = UsageCache::default();
        let report = cache.get_or_measure(&core, false);
        let keys: Vec<&str> = report.items.iter().map(|item| item.key).collect();
        assert_eq!(keys, ["audio", "records", "clipboard", "logs", "orphans", "models"]);
        let audio = report.items.iter().find(|item| item.key == "audio").unwrap();
        assert_eq!((audio.files, audio.bytes), (1, 300));
        let orphans = report.items.iter().find(|item| item.key == "orphans").unwrap();
        assert_eq!((orphans.files, orphans.bytes), (1, 50));

        // 缓存期内不重算：新增的文件要等作废或强制刷新才看得到
        std::fs::write(core.records.audio_dir().join("y.flac"), vec![0u8; 100]).unwrap();
        assert_eq!(cache.get_or_measure(&core, false).items[0].bytes, 300);
        cache.invalidate();
        assert_eq!(cache.get_or_measure(&core, false).items[0].bytes, 400);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
