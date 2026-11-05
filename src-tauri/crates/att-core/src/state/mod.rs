//! CoreState：服务层共享的核心句柄（axum 与 Tauri 命令共用）。

use crate::clipboard::ClipboardRepo;
use crate::events::EventBus;
use crate::llm::optimizer::OptimizationHub;
use crate::llm::store::LlmStore;
use crate::records::RecordRepo;
use crate::settings::{self, UserSettings};
use crate::storage::crypto::Crypto;
use crate::Platform;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("数据目录初始化失败: {0}")]
    Io(#[from] std::io::Error),
}

/// 核心状态：settings 读写锁持有当前用户设置；其余组件无内部可变态。
pub struct CoreState {
    pub data_dir: PathBuf,
    /// 本地模型根目录（`models/`）：`ATT_MODELS_DIR` > 数据目录同级的 `models/` > `<数据目录>/models`
    pub models_dir: PathBuf,
    pub settings_path: PathBuf,
    pub platform: Platform,
    settings: Mutex<UserSettings>,
    pub crypto: Arc<Crypto>,
    pub records: RecordRepo,
    /// LLM v2 档案 + 模型（data/llm_v2.json）
    pub llm: LlmStore,
    /// 剪贴板历史（data/clipboard/）
    pub clipboard: ClipboardRepo,
    /// 活动中的记录优化（取消 / 同记录互斥）
    pub optimizer: OptimizationHub,
    /// 本地文件转写（任务表 / 单并发闸门 / 结果缓存）
    pub transcriber: crate::transcribe::FileTranscriber,
    /// 本地引擎常驻池（faster-whisper / Qwen3-ASR）
    pub local_models: crate::stt::local::LocalModels,
    /// 提示词预设目录（prompts/*.txt；打包版由壳层改指向安装资源目录）
    pub prompts_dir: PathBuf,
    /// 当前应用版本（检查更新用）：默认取本 crate 版本（与工作区版本同步），壳层改为 Tauri 包版本
    pub app_version: String,
    /// 检查更新与安装包下载
    pub updater: Arc<crate::update::Updater>,
    pub events: EventBus,
}

/// 解析本地模型根目录：环境变量优先，其次数据目录同级的 `models/`（仓库里 `data/` 与 `models/` 平级）。
pub fn resolve_models_dir(data_dir: &Path) -> PathBuf {
    if let Ok(dir) = std::env::var("ATT_MODELS_DIR") {
        if !dir.trim().is_empty() {
            return tidy(PathBuf::from(dir));
        }
    }
    if let Some(parent) = data_dir.parent() {
        let sibling = parent.join("models");
        if sibling.is_dir() {
            return tidy(sibling);
        }
    }
    data_dir.join("models")
}

/// 规范化路径：去掉 `..` 段（Windows 的 `\\?\` 前缀也一并去掉，便于日志与错误提示可读）。
fn tidy(path: PathBuf) -> PathBuf {
    let resolved = std::fs::canonicalize(&path).unwrap_or(path);
    #[cfg(windows)]
    {
        if let Some(text) = resolved.to_string_lossy().strip_prefix(r"\\?\") {
            return PathBuf::from(text.to_string());
        }
    }
    resolved
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
        let crypto = Arc::new(crypto);
        let llm = LlmStore::load(data_dir, crypto.clone());
        let settings_path = data_dir.join("settings.json");
        let settings = UserSettings::load(&settings_path, platform);
        Ok(Self {
            data_dir: data_dir.to_path_buf(),
            models_dir: resolve_models_dir(data_dir),
            settings_path,
            platform,
            settings: Mutex::new(settings),
            crypto,
            records,
            llm,
            clipboard: ClipboardRepo::new(data_dir),
            optimizer: OptimizationHub::default(),
            transcriber: Default::default(),
            local_models: Default::default(),
            prompts_dir: crate::prompts::resolve_dir(data_dir, None),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            updater: Arc::new(crate::update::Updater::new(data_dir)),
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

    /// 提示词预设库：内置 `prompts_dir` + 自定义 `data/custom_prompts.json`。
    pub fn prompt_library(&self) -> crate::prompts::PromptLibrary {
        crate::prompts::PromptLibrary::new(&self.prompts_dir, &self.data_dir)
    }

    /// 删除自定义预设；它正被选用时回退：
    /// - 录音默认预设（`default_preset_id` / `last_used_preset_id`）→ 内置默认（「通用优化」，没有则第一个）
    /// - 「优化选中文本」预设（`rewrite_preset_id`）→ None，即跟随默认预设
    pub fn delete_custom_preset(&self, id: &str) -> Result<PresetFallback, crate::prompts::PresetError> {
        let library = self.prompt_library();
        library.delete(id)?;
        let fallback_id = crate::prompts::default_id(&library.load_all());
        let fallback = self
            .update_settings(|s| {
                if s.default_preset_id.as_deref() == Some(id) {
                    s.default_preset_id = fallback_id.clone();
                }
                if s.last_used_preset_id.as_deref() == Some(id) {
                    s.last_used_preset_id = fallback_id.clone();
                }
                if s.rewrite_preset_id.as_deref() == Some(id) {
                    s.rewrite_preset_id = None;
                }
                PresetFallback {
                    default_preset_id: s.default_preset_id.clone(),
                    rewrite_preset_id: s.rewrite_preset_id.clone(),
                }
            })
            .map_err(|e| crate::prompts::PresetError::Io(std::io::Error::other(e.to_string())))?;
        Ok(fallback)
    }
}

/// 删除预设后各选用处的当前值（供接口回报给前端刷新）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct PresetFallback {
    pub default_preset_id: Option<String>,
    pub rewrite_preset_id: Option<String>,
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
