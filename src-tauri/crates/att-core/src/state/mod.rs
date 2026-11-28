//! CoreState：服务层共享的核心句柄（axum 与 Tauri 命令共用）。

use crate::clipboard::ClipboardRepo;
use crate::events::EventBus;
use crate::llm::optimizer::OptimizationHub;
use crate::llm::store::LlmStore;
use crate::records::RecordRepo;
use crate::settings::{self, HotkeyAction, HotkeySpec, UserSettings};
use crate::storage::crypto::Crypto;
use crate::Platform;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("数据目录初始化失败: {0}")]
    Io(#[from] std::io::Error),
    #[error("备份或恢复期间数据暂时不可修改")]
    BackupMaintenance,
}

/// 核心状态：settings 读写锁持有当前用户设置；其余组件无内部可变态。
pub struct CoreState {
    pub data_dir: PathBuf,
    /// 本地模型根目录：`ATT_MODELS_DIR` > 用户设置 > 数据目录同级的 `models/` > `<数据目录>/models`
    pub models_dir: PathBuf,
    /// 安装包资源根目录（Tauri `resource_dir()`）：Qwen 推理库等随包资源从这里找；开发态可能为 `None`
    pub resource_dir: Option<PathBuf>,
    pub settings_path: PathBuf,
    pub platform: Platform,
    settings: Mutex<UserSettings>,
    /// 键盘监听使用的无锁配置入口；只在设置成功落盘后整体替换。
    hotkey_specs: RwLock<Arc<Vec<HotkeySpec>>>,
    pub crypto: Arc<Crypto>,
    pub records: RecordRepo,
    /// LLM v2 档案 + 模型（data/llm_v2.json）
    pub llm: LlmStore,
    /// 剪贴板历史（data/clipboard/）
    pub clipboard: ClipboardRepo,
    /// 活动中的记录优化（取消 / 同记录互斥）
    pub optimizer: OptimizationHub,
    /// 进行中的「重新识别」（同记录互斥；启动恢复时据此区分被中断的记录）
    pub retranscribe: crate::records::retranscribe::RetranscribeJobs,
    /// 数据占用统计的缓存（设置 → 诊断 →「存储」）
    pub storage_usage: crate::storage::usage::UsageCache,
    /// 本地文件转写（任务表 / 单并发闸门 / 结果缓存）
    pub transcriber: crate::transcribe::FileTranscriber,
    /// 本机用户确认过的精确路径白名单（只在内存里）：界面防误操作用，
    /// 数据面只认令牌，不再用它判断是否允许转写
    pub transcribe_allowed: Mutex<HashSet<PathBuf>>,
    /// 用户通过系统对话框确认过、供下一次导出 / 恢复使用的一次性路径白名单
    pub backup_paths: crate::backup::BackupPathGrants,
    /// 备份 / 恢复期间暂停录音与剪贴板等后台写入；恢复成功后保持到进程重启。
    backup_maintenance: AtomicBool,
    /// 本地引擎常驻池（faster-whisper / Qwen3-ASR / SenseVoice）
    pub local_models: crate::stt::local::LocalModels,
    /// 本地模型的下载任务与安装状态（设置页 / `/api/models`）
    pub model_manager: Arc<crate::stt::local::download::ModelManager>,
    /// 本地模型使用闸门：录音 / 文件转写 / 重新识别 / 预加载持读锁，删除模型持写锁。
    pub model_gate: Arc<tokio::sync::RwLock<()>>,
    /// 录音进行中（含建连、收尾）：删除模型时据此拒绝，避免把正在用的文件删掉。
    /// 由壳层录音 Actor 在状态变化时维护，核心层只读取。
    recording_active: AtomicBool,
    /// 在线引擎最近一次凭据验证结果（按凭据指纹失效，只在内存里）
    pub credential_checks: crate::stt::verify::CredentialChecks,
    /// 提示词预设目录（prompts/*.txt；打包版由壳层改指向安装资源目录）
    pub prompts_dir: PathBuf,
    /// 当前应用版本（检查更新用）：默认取本 crate 版本（与工作区版本同步），壳层改为 Tauri 包版本
    pub app_version: String,
    /// 检查更新与安装包下载
    pub updater: Arc<crate::update::Updater>,
    pub events: EventBus,
}

/// 环境变量是否锁定了模型目录；设置页据此禁用目录选择。
pub fn models_dir_env_override() -> bool {
    std::env::var("ATT_MODELS_DIR").is_ok_and(|dir| !dir.trim().is_empty())
}

/// 按环境变量、用户设置、现有默认目录的顺序选择本地模型根目录。
fn choose_models_dir(data_dir: &Path, configured: Option<&str>, env_dir: Option<&str>) -> PathBuf {
    if let Some(dir) = env_dir.filter(|dir| !dir.trim().is_empty()) {
        return tidy(PathBuf::from(dir.trim()));
    }
    if let Some(dir) = configured.filter(|dir| !dir.trim().is_empty()) {
        return tidy(PathBuf::from(dir.trim()));
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
pub(crate) fn tidy(path: PathBuf) -> PathBuf {
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
        Self::init_with_models_env(data_dir, platform, std::env::var("ATT_MODELS_DIR").ok().as_deref())
    }

    fn init_with_models_env(data_dir: &Path, platform: Platform, models_env: Option<&str>) -> Result<Self, CoreError> {
        // 必须先处理导入日志，再加载记录、密钥和设置，避免用半导入的一套数据初始化组件。
        crate::backup::recover_interrupted_import(data_dir)?;
        let records = RecordRepo::new(data_dir);
        records.ensure_dirs()?;
        for dir in ["audio/temp", "audio/permanent", "clipboard/images", "sessions"] {
            std::fs::create_dir_all(data_dir.join(dir))?;
        }
        let crypto =
            Crypto::load_or_create(data_dir).map_err(|e| CoreError::Io(std::io::Error::other(e.to_string())))?;
        let crypto = Arc::new(crypto);
        let llm = LlmStore::load(data_dir, crypto.clone());
        let settings_path = data_dir.join("settings.json");
        let settings = UserSettings::load(&settings_path, platform);
        let hotkey_specs = Arc::new(enabled_hotkey_specs(&settings));
        let models_dir = choose_models_dir(data_dir, settings.models_dir.as_deref(), models_env);
        Ok(Self {
            data_dir: data_dir.to_path_buf(),
            models_dir: models_dir.clone(),
            resource_dir: None,
            settings_path,
            platform,
            settings: Mutex::new(settings),
            hotkey_specs: RwLock::new(hotkey_specs),
            crypto,
            records,
            llm,
            clipboard: ClipboardRepo::new(data_dir),
            optimizer: OptimizationHub::default(),
            retranscribe: Default::default(),
            storage_usage: Default::default(),
            transcriber: Default::default(),
            transcribe_allowed: Mutex::new(HashSet::new()),
            backup_paths: Default::default(),
            backup_maintenance: AtomicBool::new(false),
            local_models: Default::default(),
            model_manager: Arc::new(crate::stt::local::download::ModelManager::new(models_dir)),
            model_gate: Arc::new(tokio::sync::RwLock::new(())),
            recording_active: AtomicBool::new(false),
            credential_checks: Default::default(),
            prompts_dir: crate::prompts::resolve_dir(data_dir, None),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            updater: Arc::new(crate::update::Updater::new(data_dir)),
            events: EventBus::default(),
        })
    }

    /// 开始备份维护窗口。一次只允许一个导出 / 导入占用。
    pub fn begin_backup_maintenance(&self) -> bool {
        self.backup_maintenance.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }

    /// 导出失败或导入失败后恢复后台写入；导入成功则保持锁定直到重启。
    pub fn end_backup_maintenance(&self) {
        self.backup_maintenance.store(false, Ordering::Release);
    }

    pub fn backup_maintenance_active(&self) -> bool {
        self.backup_maintenance.load(Ordering::Acquire)
    }

    /// 读取设置快照（clone；写少读多场景可接受）。
    pub fn settings(&self) -> UserSettings {
        self.settings.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 当前热键监听快照。事件线程只短暂持有读锁并克隆 `Arc`。
    pub fn hotkey_specs(&self) -> Arc<Vec<HotkeySpec>> {
        self.hotkey_specs.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 录音 Actor 维护的「正在录音」标记；删除模型时用它拒绝危险操作。
    pub fn set_recording_active(&self, active: bool) {
        self.recording_active.store(active, Ordering::Relaxed);
    }

    pub fn is_recording_active(&self) -> bool {
        self.recording_active.load(Ordering::Relaxed)
    }

    /// 在锁内修改设置并落盘（原子写）。
    pub fn update_settings<T>(&self, mutate: impl FnOnce(&mut UserSettings) -> T) -> Result<T, CoreError> {
        if self.backup_maintenance_active() {
            return Err(CoreError::BackupMaintenance);
        }
        let mut current = self.settings.lock().unwrap_or_else(|e| e.into_inner());
        // 先在副本上应用：落盘成功后才替换内存，保存失败不能留下只改了一半的设置。
        let mut next = current.clone();
        let outcome = mutate(&mut next);
        next.save(&self.settings_path)?;
        *current = next;
        let hotkey_specs = Arc::new(enabled_hotkey_specs(&current));
        drop(current);
        *self.hotkey_specs.write().unwrap_or_else(|e| e.into_inner()) = hotkey_specs;
        Ok(outcome)
    }

    /// PUT /api/settings：在设置锁内应用白名单更新并持久化。
    pub fn apply_settings_update(&self, body: &serde_json::Value) -> Result<settings::SettingsUpdate, String> {
        if self.backup_maintenance_active() {
            return Err("备份或恢复期间数据暂时不可修改".into());
        }
        let mut current = self.settings.lock().unwrap_or_else(|e| e.into_inner());
        // 校验返回 Err 时不能进入保存流程：先在副本上应用，成功落盘后才替换内存。
        let mut next = current.clone();
        let outcome = settings::apply_settings_update(&mut next, body, self.platform, &self.crypto)?;
        next.save(&self.settings_path).map_err(|e| e.to_string())?;
        *current = next;
        let hotkey_specs = Arc::new(enabled_hotkey_specs(&current));
        drop(current);
        *self.hotkey_specs.write().unwrap_or_else(|e| e.into_inner()) = hotkey_specs;
        Ok(outcome)
    }

    /// 剪贴板历史的自动清理上限（设置里 0 = 不限）。
    pub fn clipboard_limits(&self) -> crate::clipboard::RetentionLimits {
        let settings = self.settings();
        crate::clipboard::RetentionLimits {
            max_items: settings.clipboard_max_items.max(0) as usize,
            retention_days: settings.clipboard_retention_days.max(0),
        }
    }

    /// 按设置的上限清理剪贴板历史（入库后、每小时、改了上限时）；删了条目就通知列表刷新。返回删掉的条数。
    pub fn prune_clipboard(&self) -> usize {
        if self.backup_maintenance_active() {
            return 0;
        }
        let removed = self.clipboard.prune(self.clipboard_limits());
        if removed > 0 {
            tracing::info!("剪贴板历史按上限清理了 {removed} 条");
            self.events.publish(crate::events::AppEvent::ClipboardRemoved { count: removed });
        }
        removed
    }

    /// 按设置的录音音频保留天数删过期音频（每天一次、改了设置时）；0 = 永久保留，什么都不做。返回删掉的文件数。
    pub fn prune_record_audio(&self) -> usize {
        if self.backup_maintenance_active() {
            return 0;
        }
        let days = self.settings().audio_retention_days;
        match self.records.prune_audio(days, chrono::Utc::now()) {
            Ok(0) => 0,
            Ok(removed) => {
                tracing::info!("录音音频按保留期（{days} 天）删除了 {removed} 个文件");
                self.storage_usage.invalidate();
                removed
            }
            Err(error) => {
                tracing::warn!("按保留期删除录音失败: {error}");
                0
            }
        }
    }

    /// 清理孤儿音频（`audio/orphans/`）：返回（删掉的文件数, 字节数）。
    pub fn clear_orphan_audio(&self) -> std::io::Result<(u64, u64)> {
        let result = crate::storage::usage::clear_dir_files(&crate::storage::usage::orphan_audio_dir(&self.data_dir));
        self.storage_usage.invalidate();
        result
    }

    /// GET /api/settings：脱敏公共视图。
    pub fn public_settings(&self) -> serde_json::Value {
        let env_dir = std::env::var("ATT_MODELS_DIR").ok();
        self.public_settings_with_env(env_dir.as_deref())
    }

    fn public_settings_with_env(&self, env_dir: Option<&str>) -> serde_json::Value {
        let settings = self.settings();
        let next_models_dir = choose_models_dir(&self.data_dir, settings.models_dir.as_deref(), env_dir);
        let mut view = settings.public_view(self.platform);
        view["models_dir_effective"] = serde_json::json!(self.models_dir.display().to_string());
        view["models_dir_next"] = serde_json::json!(next_models_dir.display().to_string());
        view["models_dir_env_override"] = serde_json::json!(env_dir.is_some_and(|dir| !dir.trim().is_empty()));
        view
    }

    /// 登记一个允许转写的文件（精确白名单）。
    ///
    /// 只接受本机用户在界面里选择 / 拖入 / 手动确认过的路径。返回 canonicalize 后的路径，
    /// 与转写校验使用同一形态（Windows 去掉 `\\?\` 前缀）。
    pub fn allow_transcribe_file(&self, path: &Path) -> Result<PathBuf, String> {
        // 与 HTTP 侧一致：容忍首尾空白与「复制为路径」带的引号
        let raw = path.to_string_lossy();
        let raw = raw.trim().trim_matches('"');
        let resolved = std::fs::canonicalize(raw).map_err(|_| format!("文件不存在或无法访问：{raw}"))?;
        if !resolved.is_file() {
            return Err(format!("不是文件：{raw}"));
        }
        let resolved = tidy(resolved);
        self.transcribe_allowed.lock().unwrap_or_else(|e| e.into_inner()).insert(resolved.clone());
        Ok(resolved)
    }

    /// 该路径是否已经由本机用户登记为可转写。
    pub fn is_transcribe_allowed(&self, path: &Path) -> bool {
        self.transcribe_allowed.lock().unwrap_or_else(|e| e.into_inner()).contains(path)
    }

    /// 当前启用的持久 API 令牌明文（未开启 / 未配置 / 密文损坏时 None）。
    ///
    /// 只供数据面鉴权与「设置 → 诊断」显式读取；不进入 `GET /api/settings` 或日志。
    pub fn api_token(&self) -> Option<String> {
        let cipher = {
            let settings = self.settings.lock().unwrap_or_else(|e| e.into_inner());
            if !settings.api_token_enabled {
                return None;
            }
            settings.api_token_encrypted.clone()
        };
        cipher.as_deref().and_then(|token| self.crypto.decrypt(token)).filter(|t| !t.is_empty())
    }

    /// 提示词预设库：内置 `prompts_dir` + 自定义 `data/custom_prompts.json`。
    pub fn prompt_library(&self) -> crate::prompts::PromptLibrary {
        crate::prompts::PromptLibrary::new(&self.prompts_dir, &self.data_dir)
    }

    /// 当前生效的默认优化预设（录音后自动优化用；设置里的已不存在时回退内置默认）。
    pub fn default_preset(&self) -> Option<crate::prompts::Preset> {
        let presets = self.prompt_library().load_all();
        let id = crate::prompts::effective_default_id(&presets, self.settings().default_preset_id.as_deref())?;
        presets.into_iter().find(|p| p.id == id)
    }

    /// 把默认优化预设设为 `id`（同时记为最近使用，与设置页「默认预设」同一字段）；预设不存在时返回 None、不改设置。
    pub fn select_default_preset(&self, id: &str) -> Result<Option<crate::prompts::Preset>, CoreError> {
        let Some(preset) = self.prompt_library().find(id) else { return Ok(None) };
        self.update_settings(|s| {
            s.default_preset_id = Some(preset.id.clone());
            s.last_used_preset_id = Some(preset.id.clone());
        })?;
        Ok(Some(preset))
    }

    /// 「切换优化预设」快捷键：按预设列表顺序把默认优化预设换成下一个；没有任何预设时返回 None。
    pub fn cycle_default_preset(&self) -> Result<Option<crate::prompts::Preset>, CoreError> {
        let presets = self.prompt_library().load_all();
        let Some(next) = crate::prompts::next_preset(&presets, self.settings().default_preset_id.as_deref()) else {
            return Ok(None);
        };
        let id = next.id.clone();
        self.select_default_preset(&id)
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

fn enabled_hotkey_specs(settings: &UserSettings) -> Vec<HotkeySpec> {
    settings
        .hotkeys
        .iter()
        .filter(|binding| binding.enabled && binding.action != HotkeyAction::CancelRecording)
        .map(HotkeySpec::from_binding)
        .collect()
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
    use crate::settings::hotkey::HotkeyAction;
    use serde_json::json;

    #[test]
    fn 切换默认优化预设_写回设置_不存在的预设不改() {
        let dir = std::env::temp_dir().join(format!("att-core-cycle-{}", uuid::Uuid::new_v4()));
        let mut core = CoreState::init(&dir, Platform::Macos).unwrap();
        let prompts = dir.join("prompts-test");
        std::fs::create_dir_all(&prompts).unwrap();
        std::fs::write(prompts.join("通用优化.txt"), "通用").unwrap();
        std::fs::write(prompts.join("程序员模式.txt"), "程序员").unwrap();
        core.prompts_dir = prompts;

        assert_eq!(core.default_preset().unwrap().name, "通用优化", "没设置时生效的是内置默认");
        assert_eq!(core.cycle_default_preset().unwrap().unwrap().name, "程序员模式", "通用优化之后回到第一个");
        assert_eq!(core.settings().default_preset_id.as_deref(), Some("preset-程序员模式"));
        assert_eq!(core.settings().last_used_preset_id.as_deref(), Some("preset-程序员模式"));
        assert_eq!(core.cycle_default_preset().unwrap().unwrap().name, "通用优化");

        assert!(core.select_default_preset("preset-不存在").unwrap().is_none());
        assert_eq!(core.settings().default_preset_id.as_deref(), Some("preset-通用优化"), "不存在的预设不改设置");
        assert_eq!(core.select_default_preset("preset-程序员模式").unwrap().unwrap().name, "程序员模式");

        // 快捷键默认为空；只开开关不录入组合会被拒绝
        assert!(!core.settings().binding_of(HotkeyAction::CyclePreset).unwrap().enabled);
        let err = core.apply_settings_update(&json!({"cycle_preset_enabled": true})).unwrap_err();
        assert!(err.contains("切换优化预设的快捷键不能为空"), "{err}");
        core.apply_settings_update(
            &json!({"cycle_preset_hotkey": {"modifiers": ["ctrl", "alt"], "key": "p"}, "cycle_preset_enabled": true}),
        )
        .unwrap();
        let settings = core.settings();
        let binding = settings.binding_of(HotkeyAction::CyclePreset).unwrap();
        assert!(binding.enabled);
        assert_eq!(binding.display(Platform::Macos), "⌃⌥P");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 初始化_目录_密钥_设置默认值() {
        let dir = std::env::temp_dir().join(format!("att-core-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let core = CoreState::init(&dir, Platform::Windows).unwrap();
        assert!(dir.join(".encryption_key").is_file());
        assert!(dir.join("records").is_dir());
        assert!(dir.join("audio").join("temp").is_dir());
        assert_eq!(core.settings().asr_provider, "doubao_asr");

        // 更新设置 → 落盘 → 重载可见
        core.apply_settings_update(&json!({"asr_provider": "qwen_asr"})).unwrap();
        let reloaded = CoreState::init(&dir, Platform::Windows).unwrap();
        assert_eq!(reloaded.settings().asr_provider, "qwen_asr");

        // 校验失败的更新不落内存也不落盘：语音改写不能以空组合启用
        let err = core.apply_settings_update(&json!({"voice_rewrite_enabled": true})).unwrap_err();
        assert!(err.contains("快捷键不能为空"), "{err}");
        assert!(!core.settings().binding_of(HotkeyAction::VoiceRewriteSelection).unwrap().enabled);
        let reloaded = CoreState::init(&dir, Platform::Windows).unwrap();
        assert!(!reloaded.settings().binding_of(HotkeyAction::VoiceRewriteSelection).unwrap().enabled);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 热键快照_设置成功落盘后立即替换_旧快照保持不变() {
        let dir = std::env::temp_dir().join(format!("att-hotkey-snapshot-{}", uuid::Uuid::new_v4()));
        let core = CoreState::init(&dir, Platform::Macos).unwrap();
        let initial = core.hotkey_specs();
        assert!(initial.iter().any(|spec| spec.action == HotkeyAction::Record));

        core.update_settings(|settings| {
            settings.hotkeys.iter_mut().find(|binding| binding.action == HotkeyAction::Record).unwrap().enabled = false;
        })
        .unwrap();

        let updated = core.hotkey_specs();
        assert!(!Arc::ptr_eq(&initial, &updated), "设置成功后要发布新的快照");
        assert!(initial.iter().any(|spec| spec.action == HotkeyAction::Record), "读取者手里的旧快照必须继续有效");
        assert!(
            !updated.iter().any(|spec| spec.action == HotkeyAction::Record),
            "下一次键盘事件应立即看到停用后的绑定"
        );

        core.apply_settings_update(&json!({"voice_rewrite_enabled": true})).unwrap_err();
        assert!(Arc::ptr_eq(&updated, &core.hotkey_specs()), "校验失败不能发布半成品设置");

        core.apply_settings_update(&json!({
            "cycle_preset_hotkey": {"modifiers": ["ctrl", "alt"], "key": "p"},
            "cycle_preset_enabled": true
        }))
        .unwrap();
        let updated_again = core.hotkey_specs();
        assert!(updated_again.iter().any(|spec| spec.action == HotkeyAction::CyclePreset));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 设置更新_后面的键校验失败_前面的键不生效也不落盘() {
        let dir = std::env::temp_dir().join(format!("att-core-settings-atomic-{}", uuid::Uuid::new_v4()));
        let core = CoreState::init(&dir, Platform::Windows).unwrap();

        // 先落一份基线，确保失败时有可对比的 settings.json。
        core.apply_settings_update(&json!({"asr_provider": "chatgpt_asr"})).unwrap();
        let before = core.settings();
        let disk_before = std::fs::read(&core.settings_path).unwrap();

        // asr_provider 排在 recording_mode 前面：旧实现会先改掉它，再因后面的非法值报错并落盘。
        let err = core
            .apply_settings_update(&json!({
                "asr_provider": "qwen_asr",
                "recording_mode": "invalid"
            }))
            .unwrap_err();
        assert!(err.contains("recording_mode"), "{err}");

        let after = core.settings();
        assert_eq!(after.asr_provider, before.asr_provider, "前面的合法键不能部分生效");
        assert_eq!(after.recording_mode, before.recording_mode);
        assert_eq!(std::fs::read(&core.settings_path).unwrap(), disk_before, "失败时不能落盘");

        let reloaded = CoreState::init(&dir, Platform::Windows).unwrap();
        assert_eq!(reloaded.settings().asr_provider, before.asr_provider);
        assert_eq!(reloaded.settings().recording_mode, before.recording_mode);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 模型目录_环境变量_设置_现有默认依次优先() {
        let root = std::env::temp_dir().join(format!("att-models-dir-{}", uuid::Uuid::new_v4()));
        let data_dir = root.join("data");
        let configured = root.join("configured");
        let sibling = root.join("models");
        let env_dir = root.join("env");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(&configured).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::create_dir_all(&env_dir).unwrap();

        assert_eq!(
            choose_models_dir(&data_dir, Some(configured.to_str().unwrap()), Some(env_dir.to_str().unwrap())),
            tidy(env_dir.clone()),
            "环境变量必须覆盖用户设置"
        );
        assert_eq!(
            choose_models_dir(&data_dir, Some(configured.to_str().unwrap()), None),
            tidy(configured.clone()),
            "用户设置必须覆盖现有默认"
        );
        assert_eq!(choose_models_dir(&data_dir, None, None), tidy(sibling.clone()), "同级 models/ 存在时沿用现有默认");
        std::fs::remove_dir(&sibling).unwrap();
        assert_eq!(
            choose_models_dir(&data_dir, Some(""), Some("  ")),
            data_dir.join("models"),
            "空设置与空环境变量都回落到数据目录内的 models/"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 模型目录_保存设置后下一次初始化使用新目录() {
        let root = std::env::temp_dir().join(format!("att-models-restart-{}", uuid::Uuid::new_v4()));
        let data_dir = root.join("data");
        let configured = root.join("models-on-another-drive");
        std::fs::create_dir_all(&configured).unwrap();

        let core = CoreState::init_with_models_env(&data_dir, Platform::Windows, None).unwrap();
        core.apply_settings_update(&serde_json::json!({
            "models_dir": configured.to_string_lossy()
        }))
        .unwrap();
        let public = core.public_settings_with_env(None);
        assert_eq!(public["models_dir"], configured.to_string_lossy().as_ref());
        assert_eq!(public["models_dir_effective"], core.models_dir.to_string_lossy().as_ref());
        // models_dir_next 是规范化后的路径（macOS 临时目录 /var 是 /private/var 的符号链接），期望也走 tidy
        assert_eq!(public["models_dir_next"], tidy(configured.clone()).display().to_string());
        assert_eq!(public["models_dir_env_override"], false);
        drop(core);

        let restarted = CoreState::init_with_models_env(&data_dir, Platform::Windows, None).unwrap();
        assert_eq!(restarted.models_dir, tidy(configured));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 备份维护态_阻止设置写入和自动清理() {
        let dir = std::env::temp_dir().join(format!("att-backup-maintenance-{}", uuid::Uuid::new_v4()));
        let mut core = CoreState::init(&dir, Platform::Macos).unwrap();
        let prompts = dir.join("prompts-test");
        std::fs::create_dir_all(&prompts).unwrap();
        std::fs::write(prompts.join("通用优化.txt"), "通用").unwrap();
        std::fs::write(prompts.join("程序员模式.txt"), "程序员").unwrap();
        core.prompts_dir = prompts;
        core.update_settings(|settings| settings.asr_provider = "chatgpt_asr".into()).unwrap();
        let before = std::fs::read(&core.settings_path).unwrap();

        assert!(core.begin_backup_maintenance());
        assert!(matches!(
            core.update_settings(|settings| settings.asr_provider = "qwen_asr".into()),
            Err(CoreError::BackupMaintenance)
        ));
        assert!(core.apply_settings_update(&json!({"asr_provider": "qwen_asr"})).is_err());
        assert!(core.select_default_preset("preset-程序员模式").is_err());
        assert_eq!(std::fs::read(&core.settings_path).unwrap(), before, "维护态不能覆盖恢复后的设置");
        assert_eq!(core.settings().asr_provider, "chatgpt_asr");
        assert_eq!(core.prune_clipboard(), 0);
        assert_eq!(core.prune_record_audio(), 0);

        core.end_backup_maintenance();
        core.update_settings(|settings| settings.asr_provider = "qwen_asr".into()).unwrap();
        assert_eq!(core.settings().asr_provider, "qwen_asr");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
