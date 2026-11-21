//! 数据占用统计（设置 → 诊断 →「存储」）：录音音频、记录、剪贴板、日志、孤儿音频、本地模型各占多少。
//!
//! 统计要整目录遍历（模型目录可能有几个 GB 的大文件，但文件数不多），调用方放到阻塞线程；
//! 结果缓存 1 分钟，清理孤儿音频、按保留期删音频后作废缓存。

use crate::state::CoreState;
use serde::Serialize;
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

/// 清空孤儿音频目录里的文件（含子目录），目录本身保留。返回（删掉的文件数, 字节数）。
pub fn clear_dir_files(dir: &Path) -> std::io::Result<(u64, u64)> {
    let mut deleted = 0u64;
    let mut bytes = 0u64;
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
            } else if file_type.is_file() || file_type.is_symlink() {
                let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
                std::fs::remove_file(entry.path())?;
                deleted += 1;
                bytes += size;
            }
        }
    }
    Ok((deleted, bytes))
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

        assert_eq!(clear_dir_files(&dir).unwrap(), (2, 1024));
        assert_eq!(dir_usage(&dir), (0, 0));
        assert!(dir.is_dir(), "目录本身保留");
        assert_eq!(clear_dir_files(&dir.join("missing")).unwrap(), (0, 0));
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
