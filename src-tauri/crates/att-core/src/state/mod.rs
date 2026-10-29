//! CoreState：服务层共享的核心句柄（axum 与 Tauri 命令共用）。

use crate::events::EventBus;
use crate::records::RecordRepo;
use crate::settings::{self, UserSettings};
use crate::storage::crypto::Crypto;
use crate::Platform;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("数据目录初始化失败: {0}")]
    Io(#[from] std::io::Error),
}

/// 核心状态：settings 读写锁持有当前用户设置；其余组件无内部可变态。
pub struct CoreState {
    pub data_dir: PathBuf,
    pub settings_path: PathBuf,
    pub platform: Platform,
    settings: Mutex<UserSettings>,
    pub crypto: Crypto,
    pub records: RecordRepo,
    pub events: EventBus,
}

impl CoreState {
    /// 初始化（迁移已在此前完成）：建目录 → 加载密钥 → 加载设置。
    pub fn init(data_dir: &Path, platform: Platform) -> Result<Self, CoreError> {
        let records = RecordRepo::new(data_dir);
        records.ensure_dirs()?;
        for dir in ["audio/temp", "audio/permanent", "clipboard/images", "sessions"] {
            std::fs::create_dir_all(data_dir.join(dir))?;
        }
        let crypto = Crypto::load_or_create(data_dir)
            .map_err(|e| CoreError::Io(std::io::Error::other(e.to_string())))?;
        let settings_path = data_dir.join("settings.json");
        let settings = UserSettings::load(&settings_path, platform);
        Ok(Self {
            data_dir: data_dir.to_path_buf(),
            settings_path,
            platform,
            settings: Mutex::new(settings),
            crypto,
            records,
            events: EventBus::default(),
        })
    }

    /// 读取设置快照（clone；写少读多场景可接受）。
    pub fn settings(&self) -> UserSettings {
        self.settings.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 在锁内修改设置并落盘（原子写）。
    pub fn update_settings<T>(
        &self,
        mutate: impl FnOnce(&mut UserSettings) -> T,
    ) -> Result<T, CoreError> {
        let mut guard = self.settings.lock().unwrap_or_else(|e| e.into_inner());
        let outcome = mutate(&mut guard);
        guard.save(&self.settings_path)?;
        Ok(outcome)
    }

    /// PUT /api/settings：在设置锁内应用白名单更新并持久化。
    pub fn apply_settings_update(
        &self,
        body: &serde_json::Value,
    ) -> Result<settings::SettingsUpdate, String> {
        self.update_settings(|current| {
            settings::apply_settings_update(current, body, self.platform, &self.crypto)
        })
        .map_err(|e| e.to_string())?
    }

    /// GET /api/settings：脱敏公共视图。
    pub fn public_settings(&self) -> serde_json::Value {
        self.settings().public_view(self.platform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn 初始化_目录_密钥_设置默认值() {
        let dir = std::env::temp_dir().join(format!("att-core-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let core = CoreState::init(&dir, Platform::Windows).unwrap();
        assert!(dir.join(".encryption_key").is_file());
        assert!(dir.join("records").is_dir());
        assert!(dir.join("audio").join("temp").is_dir());
        assert_eq!(core.settings().asr_provider, "whisper");

        // 更新设置 → 落盘 → 重载可见
        core.apply_settings_update(&json!({"asr_provider": "qwen_asr"})).unwrap();
        let reloaded = CoreState::init(&dir, Platform::Windows).unwrap();
        assert_eq!(reloaded.settings().asr_provider, "qwen_asr");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
