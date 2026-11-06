//! 设置域：Settings(env) + UserSettings(data/settings.json)。
//!
//! 这是**全应用唯一的设置归一化实现**（旧版同一逻辑散落在 Python 后端、旧 Electron 主进程、
//! 桌面代理与前端 stores 四处，重写时收敛于此）。

pub mod hotkey;

use crate::storage::json_store;
use crate::Platform;
#[cfg(test)]
use crate::now_rfc3339;
use hotkey::binding_from_legacy;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub use hotkey::{HotkeyAction, HotkeyBinding, TriggerMode};

/// 已下线功能的僵尸配置键：读写两侧一律剥离（清单与 app/config.py:27-38 对齐）。
pub const RETIRED_SETTINGS_KEYS: &[&str] = &[
    "selection_toolbar_enabled",
    "selection_toolbar_trigger_delay_ms",
    "selection_toolbar_fallback_to_clipboard",
    "selection_toolbar_show_examples",
    "selection_toolbar_auto_add_to_wordbook",
    "clipboard_overlay_trigger",
    "clipboard_overlay_retrigger_action",
    "clipboard_overlay_enter_behavior",
    "clipboard_overlay_auto_focus_search",
    "clipboard_overlay_close_on_click_copy",
];

/// 识别引擎相关的设置键（语义沿用 Python MODEL_RUNTIME_KEYS）：变更时数据面释放不再使用的本地模型、
/// 并在后台预加载新选中的本地引擎（见 att-server 的 settings_put）。
pub const MODEL_RUNTIME_KEYS: &[&str] = &[
    "asr_provider",
    "asr_language",
    "whisper_model",
    "whisper_device",
    "qwen_asr_model",
    "qwen_asr_device",
    "qwen_asr_dtype",
    "qwen_asr_max_new_tokens",
    "qwen_asr_return_timestamps",
    "qwen_asr_use_forced_aligner",
    "qwen_asr_forced_aligner_model",
    "qwen_asr_max_audio_seconds",
    "doubao_asr_resource_id",
    "doubao_asr_endpoint",
    "doubao_asr_end_window_size",
    "chatgpt_asr_endpoint",
    "chatgpt_asr_timeout_seconds",
];

/// 剪贴板悬浮框高度的允许范围（逻辑像素）：再矮放不下卡片内容，再高就挡住大半个屏幕。
pub const CLIPBOARD_PANEL_HEIGHT_RANGE: (i64, i64) = (200, 480);
/// 剪贴板悬浮框最大宽度的允许范围（逻辑像素）；0 单独表示铺满屏幕。
pub const CLIPBOARD_PANEL_WIDTH_RANGE: (i64, i64) = (720, 3840);

/// 设置修订号：默认值变更需要升级旧数据时加一，并在 [`upgrade_legacy_defaults`] 里补一步。
/// 2：豆包接入点改为 bigmodel_async；强制断句静音时长 0 → 800ms（0 时服务端按语义分段，
///    停顿后基本不确认，流式的未确认文字要到松开才变色）。
pub const SETTINGS_REVISION: i64 = 2;

/// 进程级配置（环境变量覆盖）：数据目录与模型目录。
#[derive(Debug, Clone)]
pub struct EnvSettings {
    pub data_dir: PathBuf,
    pub models_dir: PathBuf,
}

impl EnvSettings {
    pub fn from_env() -> Self {
        let data_dir = std::env::var("ATT_DATA_DIR").unwrap_or_else(|_| "data".into());
        let models_dir = std::env::var("ATT_MODELS_DIR").unwrap_or_else(|_| "models".into());
        Self {
            data_dir: PathBuf::from(data_dir),
            models_dir: PathBuf::from(models_dir),
        }
    }
}

/// 用户设置（data/settings.json）。
///
/// 字段名与默认值与 Python 版 `UserSettings.DEFAULT_SETTINGS` 逐一对齐
/// （旧版配置全表）；全部 `#[serde(default)]` 保证旧数据可读。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UserSettings {
    // ---- ASR 基础 ----
    pub asr_provider: String,          // whisper | qwen_asr | doubao_asr | chatgpt_asr
    pub asr_language: String,          // zh | en | ja | ko
    /// 个人词表（热词）：与引擎无关地保存，目前只有豆包会发送（见 `stt::hotwords`）。
    /// 宽容读：结构异常只丢这一项，不能拖垮整份设置
    #[serde(default, deserialize_with = "deserialize_hotwords_lenient")]
    pub asr_hotwords: Vec<String>,
    // ---- Whisper ----
    pub whisper_model: String,
    pub whisper_device: String,        // auto | cuda | cpu
    // ---- Qwen3-ASR ----
    pub qwen_asr_model: String,
    pub qwen_asr_device: String,
    pub qwen_asr_dtype: String,
    #[serde(default, deserialize_with = "deserialize_i64_lenient")]
    pub qwen_asr_max_new_tokens: i64,
    pub qwen_asr_return_timestamps: bool,
    pub qwen_asr_use_forced_aligner: bool,
    pub qwen_asr_forced_aligner_model: String,
    #[serde(default, deserialize_with = "deserialize_i64_lenient")]
    pub qwen_asr_max_audio_seconds: i64,
    // ---- 豆包（火山 SAUC）----
    pub doubao_asr_resource_id: String,
    pub doubao_asr_endpoint: String,
    #[serde(default, deserialize_with = "deserialize_i64_lenient")]
    pub doubao_asr_end_window_size: i64,
    // ---- ChatGPT 听写 ----
    pub chatgpt_asr_endpoint: String,
    #[serde(default, deserialize_with = "deserialize_i64_lenient")]
    pub chatgpt_asr_timeout_seconds: i64,
    // ---- 密文（GET 视图替换为 *_set 布尔，明文永不外发）----
    pub doubao_asr_api_key_encrypted: Option<String>,
    pub chatgpt_asr_api_key_encrypted: Option<String>,
    // ---- 行为 ----
    pub pause_audio_on_recording: bool,
    pub auto_optimize: bool,
    pub auto_copy_to_clipboard: bool,
    /// 检查到新版本时自动下载安装包（开启后启动时也会检查一次）；旧数据缺字段 = 关闭
    pub auto_download_updates: bool,
    /// 剪贴板悬浮框里按回车：`paste` 粘贴到唤起前的应用；`copy` 只复制。
    /// 开关与快捷键存在 `hotkeys` 的 ClipboardPanel 绑定里（enabled 即开关）
    pub clipboard_panel_enter_action: String,
    /// 剪贴板悬浮框高度（逻辑像素，卡片随之缩放），见 [`CLIPBOARD_PANEL_HEIGHT_RANGE`]。
    /// 尺寸两项只由悬浮框拖动写入（[`UserSettings::set_clipboard_panel_size`]），不在 PUT 白名单里：
    /// 设置页保存时会把整份旧设置带回来，若接受这两项会把刚拖好的尺寸覆盖掉
    #[serde(default, deserialize_with = "deserialize_i64_lenient")]
    pub clipboard_panel_height: i64,
    /// 剪贴板悬浮框最大宽度（逻辑像素，水平居中）；0 = 铺满屏幕，见 [`CLIPBOARD_PANEL_WIDTH_RANGE`]
    #[serde(default, deserialize_with = "deserialize_i64_lenient")]
    pub clipboard_panel_max_width: i64,
    /// 剪贴板悬浮框离屏幕底边的距离（逻辑像素，0 = 贴底）；同样只由悬浮框拖动写入，显示时再按屏幕高度夹取
    #[serde(default, deserialize_with = "deserialize_i64_lenient")]
    pub clipboard_panel_bottom_offset: i64,
    /// 透明透传键：仅存在于 PUT 白名单（与 Python 行为一致），UI 选择后原样保存。
    pub audio_input_device: Option<String>,
    // ---- 录音 ----
    pub recording_mode: String,        // single | multi | streaming
    pub silence_threshold: f64,        // 秒
    #[serde(default, deserialize_with = "deserialize_i64_lenient")]
    pub silence_level: i64,            // dB
    // ---- 提示词 ----
    pub default_preset_id: Option<String>,
    pub last_used_preset_id: Option<String>,
    /// 「优化选中文本」用的预设；None = 跟随 `default_preset_id`
    pub rewrite_preset_id: Option<String>,
    // ---- 热键（新版多绑定模型，取代旧 hotkey 单对象）----
    /// 宽容读：认不出的绑定（如更新版本新增的动作）只丢那一条，不能让整份设置回落默认值
    #[serde(default, deserialize_with = "deserialize_hotkeys_lenient")]
    pub hotkeys: Vec<HotkeyBinding>,
    /// 设置修订号（见 [`SETTINGS_REVISION`]）：不在 PUT 白名单里，只由加载升级维护
    #[serde(default, deserialize_with = "deserialize_i64_lenient")]
    pub settings_revision: i64,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self::with_platform(Platform::current())
    }
}

impl UserSettings {
    /// 按平台构造默认值（热键默认绑定随平台变化）。
    pub fn with_platform(platform: Platform) -> Self {
        Self {
            // 新装默认在线豆包：安装包不带本地模型，默认本地引擎会让首屏直接报「模型缺失」。
            // 老用户 settings.json 里已有值，不受影响；缺这个键的旧文件按新默认值补齐
            asr_provider: "doubao_asr".into(),
            asr_language: "zh".into(),
            asr_hotwords: Vec::new(),
            whisper_model: "large-v3".into(),
            whisper_device: "auto".into(),
            qwen_asr_model: "Qwen/Qwen3-ASR-1.7B".into(),
            qwen_asr_device: "auto".into(),
            qwen_asr_dtype: "auto".into(),
            qwen_asr_max_new_tokens: 256,
            qwen_asr_return_timestamps: false,
            qwen_asr_use_forced_aligner: false,
            qwen_asr_forced_aligner_model: "Qwen/Qwen3-ForcedAligner-0.6B".into(),
            qwen_asr_max_audio_seconds: 30,
            doubao_asr_resource_id: "volc.seedasr.sauc.duration".into(),
            doubao_asr_endpoint: crate::stt::sauc::DEFAULT_ENDPOINT.into(),
            doubao_asr_end_window_size: 800,
            chatgpt_asr_endpoint: "https://chatgpt.com/backend-api/transcribe".into(),
            chatgpt_asr_timeout_seconds: 120,
            doubao_asr_api_key_encrypted: None,
            chatgpt_asr_api_key_encrypted: None,
            pause_audio_on_recording: true,
            auto_optimize: false,
            auto_copy_to_clipboard: true,
            auto_download_updates: false,
            clipboard_panel_enter_action: "paste".into(),
            clipboard_panel_height: 244,
            clipboard_panel_max_width: 1280,
            clipboard_panel_bottom_offset: 0,
            audio_input_device: None,
            recording_mode: "single".into(),
            silence_threshold: 1.5,
            silence_level: -40,
            default_preset_id: None,
            last_used_preset_id: None,
            rewrite_preset_id: None,
            hotkeys: vec![
                HotkeyBinding::default_binding(platform),
                HotkeyBinding::default_for(HotkeyAction::RecordToggle, platform),
                HotkeyBinding::default_for(HotkeyAction::ClipboardPanel, platform),
                HotkeyBinding::default_for(HotkeyAction::RewriteSelection, platform),
            ],
            settings_revision: SETTINGS_REVISION,
        }
    }

    /// 从 data/settings.json 加载：宽容读 + 剥离退役键 + 旧 hotkey 迁移 + 默认值兜底。
    pub fn load(path: &Path, platform: Platform) -> Self {
        json_store::with_resource_lock(path, || {
            let raw: Value = json_store::read_json_lenient(path);
            let mut object = json_store::ensure_object(raw);
            let Some(map) = object.as_object_mut() else {
                return Self::with_platform(platform);
            };
            strip_retired_keys(map);
            migrate_legacy_hotkey(map, platform);
            upgrade_legacy_defaults(map);
            merge_defaults_into(map, platform);
            let mut settings: Self = serde_json::from_value(object).unwrap_or_else(|error| {
                tracing::warn!("settings.json 结构异常，回落默认设置: {error}");
                Self::with_platform(platform)
            });
            settings.ensure_default_bindings(platform);
            settings
        })
    }

    /// 旧数据只有录音绑定：补上后来新增动作的默认绑定（只改内存，下次保存才落盘）。
    pub fn ensure_default_bindings(&mut self, platform: Platform) {
        // 点按录音拆成独立动作之前，录音只有一条绑定、靠 trigger 二选一：
        // 手动设成点按的旧数据挪成「点按录音」，按住录音留空（未启用），行为不变
        if self.binding_of(HotkeyAction::RecordToggle).is_none() {
            if let Some(record) = self
                .hotkeys
                .iter_mut()
                .find(|b| b.action == HotkeyAction::Record && b.trigger == TriggerMode::Toggle)
            {
                record.action = HotkeyAction::RecordToggle;
                let hold = HotkeyBinding { enabled: false, ..HotkeyBinding::default_binding(platform) };
                self.hotkeys.insert(0, hold);
            }
        }
        if self.binding_of(HotkeyAction::Record).is_none() {
            self.hotkeys.insert(0, HotkeyBinding::default_binding(platform));
        }
        for action in [HotkeyAction::RecordToggle, HotkeyAction::ClipboardPanel, HotkeyAction::RewriteSelection] {
            if self.binding_of(action).is_none() {
                self.hotkeys.push(HotkeyBinding::default_for(action, platform));
            }
        }
        // 触发方式由动作决定，存盘字段跟着归一（防止手改 / 旧数据里不一致）
        for binding in &mut self.hotkeys {
            binding.trigger = binding.action.trigger();
        }
    }

    /// 原子写回。
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        json_store::write_json_atomic(path, self)
    }

    /// 首个指定动作的绑定（前端兼容视图取 Record 绑定）。
    pub fn binding_of(&self, action: HotkeyAction) -> Option<&HotkeyBinding> {
        self.hotkeys.iter().find(|b| b.action == action)
    }

    /// 旧版 hotkey 视图（GET /api/settings 与桌面设置命令都要返回它）。
    pub fn legacy_hotkey_view(&self, platform: Platform) -> Value {
        self.binding_of(HotkeyAction::Record)
            .map(|b| b.to_legacy(platform))
            .unwrap_or_else(|| HotkeyBinding::default_binding(platform).to_legacy(platform))
    }

    /// 用旧版 hotkey 对象更新 Record 绑定（保持触发模式不变）。
    pub fn update_record_binding_from_legacy(&mut self, platform: Platform, value: &Value) {
        self.update_binding_from_legacy(HotkeyAction::Record, platform, value);
    }

    /// 用旧版 `{modifiers, key}` 更新某个动作的组合键：触发模式与开关保持不变，不合法时回落该动作的默认组合。
    pub fn update_binding_from_legacy(&mut self, action: HotkeyAction, platform: Platform, value: &Value) {
        let mut binding = self
            .binding_of(action)
            .cloned()
            .unwrap_or_else(|| HotkeyBinding::default_for(action, platform));
        let (modifiers, hid_key) = hotkey::combo_from_legacy(value).unwrap_or_else(|| {
            let fallback = HotkeyBinding::default_for(action, platform);
            (fallback.modifiers, fallback.hid_key)
        });
        binding.modifiers = modifiers;
        binding.hid_key = hid_key;
        self.set_binding(binding);
    }

    fn set_binding(&mut self, binding: HotkeyBinding) {
        match self.hotkeys.iter_mut().find(|b| b.action == binding.action) {
            Some(slot) => *slot = binding,
            None => self.hotkeys.push(binding),
        }
    }

    /// 写入剪贴板悬浮框尺寸（夹到允许范围；`max_width` ≤ 0 表示铺满屏幕）。
    pub fn set_clipboard_panel_size(&mut self, max_width: i64, height: i64) {
        let (min_h, max_h) = CLIPBOARD_PANEL_HEIGHT_RANGE;
        let (min_w, max_w) = CLIPBOARD_PANEL_WIDTH_RANGE;
        self.clipboard_panel_height = height.clamp(min_h, max_h);
        self.clipboard_panel_max_width = if max_width <= 0 { 0 } else { max_width.clamp(min_w, max_w) };
    }

    /// 写入剪贴板悬浮框离底距离（只保证不为负；上限取决于当时的屏幕，由窗口层夹取）。
    pub fn set_clipboard_panel_offset(&mut self, bottom_offset: i64) {
        self.clipboard_panel_bottom_offset = bottom_offset.max(0);
    }

    pub fn reset_clipboard_panel_position(&mut self) {
        self.clipboard_panel_bottom_offset = Self::default().clipboard_panel_bottom_offset;
    }

    pub fn reset_clipboard_panel_size(&mut self) {
        let defaults = Self::default();
        self.clipboard_panel_height = defaults.clipboard_panel_height;
        self.clipboard_panel_max_width = defaults.clipboard_panel_max_width;
    }

    /// 剪贴板悬浮框是否启用（ClipboardPanel 绑定的 enabled）。
    pub fn clipboard_panel_enabled(&self) -> bool {
        self.binding_of(HotkeyAction::ClipboardPanel).is_some_and(|b| b.enabled)
    }

    /// 指定动作的绑定；还没有时给平台默认值。
    fn binding_or_default(&self, action: HotkeyAction, platform: Platform) -> HotkeyBinding {
        self.binding_of(action)
            .cloned()
            .unwrap_or_else(|| HotkeyBinding::default_for(action, platform))
    }

    /// GET /api/settings 响应视图：脱敏（密文 → *_set 布尔）+ 旧 hotkey 兼容字段。
    pub fn public_view(&self, platform: Platform) -> Value {
        let mut view = json!({});
        let object = serde_json::to_value(self).unwrap_or_else(|_| json!({}));
        if let Some(map) = object.as_object() {
            for (key, value) in map {
                // 密文字段绝不外发
                if key == "doubao_asr_api_key_encrypted" || key == "chatgpt_asr_api_key_encrypted" {
                    continue;
                }
                view[key] = value.clone();
            }
        }
        view["doubao_asr_api_key_set"] = json!(self.doubao_asr_api_key_encrypted.is_some());
        view["chatgpt_asr_api_key_set"] = json!(self.chatgpt_asr_api_key_encrypted.is_some());
        for &(action, hotkey_key, enabled_key) in EXTRA_HOTKEY_FIELDS {
            let binding = self.binding_or_default(action, platform);
            view[hotkey_key] = binding.to_legacy(platform);
            view[enabled_key] = json!(binding.enabled);
        }
        view
    }
}

/// 设置页以旧版 `{modifiers, key, display}` 形状读写的快捷键：(动作, 快捷键字段, 开关字段)。
/// 开关就是该绑定的 `enabled`；两种录音的「开关为否」在设置页显示为快捷键为空。
const EXTRA_HOTKEY_FIELDS: &[(HotkeyAction, &str, &str)] = &[
    (HotkeyAction::Record, "hotkey", "hotkey_enabled"),
    (HotkeyAction::RecordToggle, "record_toggle_hotkey", "record_toggle_enabled"),
    (HotkeyAction::ClipboardPanel, "clipboard_panel_hotkey", "clipboard_panel_enabled"),
    (HotkeyAction::RewriteSelection, "rewrite_hotkey", "rewrite_enabled"),
];

/// 设置更新结果：变更的运行时键（供上层触发停服务/重建 provider）。
#[derive(Debug, Default)]
pub struct SettingsUpdate {
    pub runtime_keys_changed: Vec<&'static str>,
    pub restart_hotkey_backend: bool,
}

/// 应用 PUT /api/settings 请求体。
///
/// 契约（与旧版接口一致，必须保真）：
/// - 白名单之外的未知键静默丢弃
/// - `null` = 未提供（跳过）
/// - 类型非法 → 中文错误（HTTP 400）
/// - `doubao_asr_api_key`（明文）→ 加密落盘；`*_clear` → 清除
pub fn apply_settings_update(
    settings: &mut UserSettings,
    body: &Value,
    platform: Platform,
    crypto: &crate::storage::crypto::Crypto,
) -> Result<SettingsUpdate, String> {
    let body = json_store::ensure_object(body.clone());
    let map = body.as_object().ok_or("请求体必须是对象")?;
    let mut outcome = SettingsUpdate::default();

    // 整组绑定先落：前端常把 GET 回来的 hotkeys 原样带回，
    // 若按键名顺序在 hotkey / clipboard_panel_* 之后处理，会把同一请求里的改键覆盖回旧值
    if let Some(value) = map.get("hotkeys").filter(|v| !v.is_null()) {
        let parsed: Vec<HotkeyBinding> = serde_json::from_value(value.clone())
            .map_err(|e| format!("字段 hotkeys 结构非法: {e}"))?;
        settings.hotkeys = parsed;
        settings.ensure_default_bindings(platform);
        outcome.restart_hotkey_backend = true;
    }

    // 通用字段：key → （读取器）逐个显式处理，保证类型契约
    for (key, value) in map {
        if value.is_null() {
            continue; // null = 未提供
        }
        let type_error = |expect: &str| format!("字段 {key} 类型非法，期望 {expect}");

        if let Some(&(action, hotkey_key, _)) =
            EXTRA_HOTKEY_FIELDS.iter().find(|(_, hotkey_key, enabled_key)| key == hotkey_key || key == enabled_key)
        {
            if key == hotkey_key {
                if !value.is_object() {
                    return Err(type_error("对象"));
                }
                settings.update_binding_from_legacy(action, platform, value);
            } else {
                let mut binding = settings.binding_or_default(action, platform);
                binding.enabled = value.as_bool().ok_or_else(|| type_error("布尔值"))?;
                settings.set_binding(binding);
            }
            outcome.restart_hotkey_backend = true;
            continue;
        }

        match key.as_str() {
            // ---- 密钥类（明文入口，只写不读）----
            "doubao_asr_api_key" => {
                let plain = value.as_str().filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| type_error("字符串"))?;
                settings.doubao_asr_api_key_encrypted = Some(crypto.encrypt(plain.trim()));
            }
            "doubao_asr_api_key_clear" => {
                if value.as_bool().unwrap_or(false) {
                    settings.doubao_asr_api_key_encrypted = None;
                }
            }
            "chatgpt_asr_api_key" => {
                let plain = value.as_str().filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| type_error("字符串"))?;
                settings.chatgpt_asr_api_key_encrypted = Some(crypto.encrypt(plain.trim()));
            }
            "chatgpt_asr_api_key_clear" => {
                if value.as_bool().unwrap_or(false) {
                    settings.chatgpt_asr_api_key_encrypted = None;
                }
            }
            // 已在循环前处理
            "hotkeys" => {}
            "clipboard_panel_enter_action" => {
                let action = value.as_str().ok_or_else(|| type_error("字符串"))?;
                if !matches!(action, "paste" | "copy") {
                    return Err(format!("字段 clipboard_panel_enter_action 取值非法: {action}"));
                }
                settings.clipboard_panel_enter_action = action.into();
            }

            // ---- 字符串键 ----
            "asr_provider" => set_string(&mut settings.asr_provider, key, value, &type_error)?,
            "asr_language" => set_string(&mut settings.asr_language, key, value, &type_error)?,
            "asr_hotwords" => {
                let words = value.as_array().ok_or_else(|| type_error("字符串数组"))?;
                settings.asr_hotwords = crate::stt::hotwords::normalize(words.iter().filter_map(Value::as_str));
            }
            "whisper_model" => set_string(&mut settings.whisper_model, key, value, &type_error)?,
            "whisper_device" => set_string(&mut settings.whisper_device, key, value, &type_error)?,
            "qwen_asr_model" => set_string(&mut settings.qwen_asr_model, key, value, &type_error)?,
            "qwen_asr_device" => set_string(&mut settings.qwen_asr_device, key, value, &type_error)?,
            "qwen_asr_dtype" => set_string(&mut settings.qwen_asr_dtype, key, value, &type_error)?,
            "qwen_asr_forced_aligner_model" => {
                set_string(&mut settings.qwen_asr_forced_aligner_model, key, value, &type_error)?
            }
            "doubao_asr_resource_id" => {
                set_string(&mut settings.doubao_asr_resource_id, key, value, &type_error)?
            }
            "doubao_asr_endpoint" => {
                set_string(&mut settings.doubao_asr_endpoint, key, value, &type_error)?
            }
            "chatgpt_asr_endpoint" => {
                set_string(&mut settings.chatgpt_asr_endpoint, key, value, &type_error)?
            }
            "recording_mode" => {
                let mode = value.as_str().ok_or_else(|| type_error("字符串"))?;
                if !matches!(mode, "single" | "multi" | "streaming") {
                    return Err(format!("字段 recording_mode 取值非法: {mode}"));
                }
                settings.recording_mode = mode.into();
            }
            "audio_input_device" => {
                settings.audio_input_device = value.as_str().map(str::to_string);
            }
            "default_preset_id" | "last_used_preset_id" => {
                let v = match value.as_str() {
                    Some(s) => Some(s.to_string()),
                    None if value.is_null() => None,
                    _ => return Err(type_error("字符串或 null")),
                };
                if key == "default_preset_id" {
                    settings.default_preset_id = v;
                } else {
                    settings.last_used_preset_id = v;
                }
            }
            // 空串 = 跟随默认预设（设置页下拉框的「跟随默认」选项）
            "rewrite_preset_id" => {
                let id = value.as_str().ok_or_else(|| type_error("字符串或 null"))?.trim();
                settings.rewrite_preset_id = (!id.is_empty()).then(|| id.to_string());
            }

            // ---- 整数键 ----
            "qwen_asr_max_new_tokens" => set_i64(&mut settings.qwen_asr_max_new_tokens, key, value, &type_error)?,
            "qwen_asr_max_audio_seconds" => set_i64(&mut settings.qwen_asr_max_audio_seconds, key, value, &type_error)?,
            "doubao_asr_end_window_size" => set_i64(&mut settings.doubao_asr_end_window_size, key, value, &type_error)?,
            "chatgpt_asr_timeout_seconds" => set_i64(&mut settings.chatgpt_asr_timeout_seconds, key, value, &type_error)?,
            "silence_level" => set_i64(&mut settings.silence_level, key, value, &type_error)?,

            // ---- 浮点键 ----
            "silence_threshold" => {
                let v = value.as_f64().ok_or_else(|| type_error("数字"))?;
                settings.silence_threshold = v.clamp(0.0, 60.0);
            }

            // ---- 布尔键 ----
            "qwen_asr_return_timestamps" => set_bool(&mut settings.qwen_asr_return_timestamps, key, value, &type_error)?,
            "qwen_asr_use_forced_aligner" => set_bool(&mut settings.qwen_asr_use_forced_aligner, key, value, &type_error)?,
            "pause_audio_on_recording" => set_bool(&mut settings.pause_audio_on_recording, key, value, &type_error)?,
            "auto_optimize" => set_bool(&mut settings.auto_optimize, key, value, &type_error)?,
            "auto_copy_to_clipboard" => set_bool(&mut settings.auto_copy_to_clipboard, key, value, &type_error)?,
            "auto_download_updates" => set_bool(&mut settings.auto_download_updates, key, value, &type_error)?,

            // 未知键：静默丢弃（契约如此）
            _ => {}
        }

        if MODEL_RUNTIME_KEYS.contains(&key.as_str()) {
            outcome.runtime_keys_changed.push(
                MODEL_RUNTIME_KEYS.iter().find(|k| **k == key.as_str()).copied().unwrap_or(""),
            );
        }
    }

    // 两个全局快捷键撞键时只会触发其中一个：直接拒绝，让用户换一个
    let enabled: Vec<&HotkeyBinding> = settings.hotkeys.iter().filter(|b| b.enabled).collect();
    for (index, later) in enabled.iter().enumerate() {
        if let Some(earlier) = enabled[..index].iter().find(|b| b.action != later.action && b.same_combo(later)) {
            if later.action.is_record() && earlier.action.is_record() {
                return Err("按住录音与点按录音的快捷键相同：同一个键只能用一种录音方式，请先清空其中一个".into());
            }
            return Err(format!("{}的快捷键与{}快捷键相同，请换一个", later.action.label(), earlier.action.label()));
        }
    }

    Ok(outcome)
}

fn set_string(
    slot: &mut String, key: &str, value: &Value,
    type_error: &dyn Fn(&str) -> String,
) -> Result<(), String> {
    let _ = key;
    *slot = value.as_str().ok_or_else(|| type_error("字符串"))?.to_string();
    Ok(())
}

fn set_i64(
    slot: &mut i64, key: &str, value: &Value,
    type_error: &dyn Fn(&str) -> String,
) -> Result<(), String> {
    let _ = key;
    // 与反序列化同策略：接受浮点数（取整），兼容前端滑杆/Python 侧写出的 -40.0
    *slot = value
        .as_i64()
        .or_else(|| value.as_f64().map(|f| f.round() as i64))
        .ok_or_else(|| type_error("整数"))?;
    Ok(())
}

fn set_bool(
    slot: &mut bool, key: &str, value: &Value,
    type_error: &dyn Fn(&str) -> String,
) -> Result<(), String> {
    let _ = key;
    *slot = value.as_bool().ok_or_else(|| type_error("布尔值"))?;
    Ok(())
}

/// 宽容整数反序列化：Python / 前端可能把整数语义字段写成浮点数
/// （如 `silence_level: -40.0`），严格 i64 反序列化会让 settings.json
/// **整体**解析失败、全部设置回落默认值（真实事故：曾导致用户配置与密钥密文被
/// 默认值覆盖）。这里接受整数与可取整的浮点，仅拒绝非数字。
fn deserialize_i64_lenient<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|f| f.round() as i64))
        .ok_or_else(|| {
            serde::de::Error::custom("期望整数（或可取整的数字）")
        })
}

/// 热键宽容反序列化：逐条解析，认不出的条目（未知动作、结构异常）跳过。
fn deserialize_hotkeys_lenient<'de, D>(deserializer: D) -> Result<Vec<HotkeyBinding>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    let items = value.as_array().cloned().unwrap_or_default();
    Ok(items
        .into_iter()
        .filter_map(|item| match serde_json::from_value::<HotkeyBinding>(item) {
            Ok(binding) => Some(binding),
            Err(error) => {
                tracing::warn!("跳过无法识别的快捷键绑定: {error}");
                None
            }
        })
        .collect())
}

/// 热词宽容反序列化：只认字符串数组里的字符串项，其余一律忽略（不报错）。
fn deserialize_hotwords_lenient<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    let words = value.as_array().map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>());
    Ok(crate::stt::hotwords::normalize(words.unwrap_or_default()))
}

/// 剥离退役键（原地）。
pub fn strip_retired_keys(object: &mut serde_json::Map<String, Value>) {
    for key in RETIRED_SETTINGS_KEYS {
        object.remove(*key);
    }
}

/// 旧 `hotkey` 对象 → `hotkeys[]`（仅在 hotkeys 缺失时转换；完整迁移见 migration 模块）。
fn migrate_legacy_hotkey(object: &mut serde_json::Map<String, Value>, platform: Platform) {
    let has_new = object.get("hotkeys").and_then(|h| h.as_array()).is_some_and(|a| !a.is_empty());
    if has_new {
        object.remove("hotkey");
        return;
    }
    if let Some(legacy) = object.remove("hotkey") {
        let binding = binding_from_legacy(platform, &legacy);
        object.insert("hotkeys".into(), serde_json::to_value(vec![binding]).unwrap_or_default());
    }
}

/// 按修订号把旧默认值升级到新默认值，每步只跑一次（用户之后再改回旧值也不会被覆盖）。
///
/// 必须在合并默认值之前调用：缺修订号的旧数据按 1 算，合并后就分不出来了。
/// 加载时只改内存，下次保存设置才落盘；在那之前每次加载都会重跑，结果相同。
pub(crate) fn upgrade_legacy_defaults(object: &mut serde_json::Map<String, Value>) {
    use crate::stt::sauc::{DEFAULT_ENDPOINT, LEGACY_DEFAULT_ENDPOINT};
    let revision = object.get("settings_revision").and_then(Value::as_i64).unwrap_or(1);
    if revision < 2 {
        if object.get("doubao_asr_endpoint").and_then(Value::as_str).map(str::trim) == Some(LEGACY_DEFAULT_ENDPOINT) {
            object.insert("doubao_asr_endpoint".into(), json!(DEFAULT_ENDPOINT));
        }
        if object.get("doubao_asr_end_window_size").and_then(Value::as_f64) == Some(0.0) {
            object.insert("doubao_asr_end_window_size".into(), json!(800));
        }
    }
    if revision < SETTINGS_REVISION {
        object.insert("settings_revision".into(), json!(SETTINGS_REVISION));
    }
}

/// 默认值合并进 JSON 对象（等价 Python `{**DEFAULT_SETTINGS, **payload}`）。
fn merge_defaults_into(object: &mut serde_json::Map<String, Value>, platform: Platform) {
    let defaults = serde_json::to_value(UserSettings::with_platform(platform))
        .unwrap_or_else(|_| json!({}));
    if let Some(default_map) = defaults.as_object() {
        for (key, value) in default_map {
            object.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::crypto::Crypto;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("att-settings-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn 加载_退役键剥离_旧热键迁移_默认值兜底() {
        let dir = temp_dir("load");
        let path = dir.join("settings.json");
        std::fs::write(&path, serde_json::json!({
            "asr_provider": "doubao_asr",
            "selection_toolbar_enabled": true,           // 退役键
            "clipboard_overlay_trigger": "x",            // 退役键
            "hotkey": {"modifiers": ["ctrl", "shift"], "key": "r", "display": "Ctrl+Shift+R"},
            "auto_optimize": true
        }).to_string()).unwrap();

        let settings = UserSettings::load(&path, Platform::Windows);
        assert_eq!(settings.asr_provider, "doubao_asr");
        assert!(settings.auto_optimize);
        // 默认值兜底：未出现的键全部有值
        assert_eq!(settings.whisper_model, "large-v3");
        // 旧热键迁移：Ctrl+Shift+R 保持
        let binding = settings.binding_of(HotkeyAction::Record).unwrap();
        assert_eq!(binding.display(Platform::Linux), "Ctrl+Shift+R");
        assert_eq!(binding.trigger, TriggerMode::Hold);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 加载_缺识别引擎时取新默认豆包_已有值保持不变() {
        let dir = temp_dir("provider-default");
        let path = dir.join("settings.json");
        // 新装（没有 settings.json）与缺 asr_provider 的旧文件：都取新默认值
        assert_eq!(UserSettings::load(&path, Platform::Macos).asr_provider, "doubao_asr");
        std::fs::write(&path, json!({ "auto_optimize": true }).to_string()).unwrap();
        assert_eq!(UserSettings::load(&path, Platform::Macos).asr_provider, "doubao_asr");
        // 老用户选过的引擎（含旧默认 whisper）原样保留
        for provider in ["whisper", "qwen_asr", "chatgpt_asr", "doubao_asr"] {
            std::fs::write(&path, json!({ "asr_provider": provider }).to_string()).unwrap();
            assert_eq!(UserSettings::load(&path, Platform::Windows).asr_provider, provider);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 加载_豆包旧默认接入点升级_自定义的不动() {
        let dir = temp_dir("endpoint");
        let path = dir.join("settings.json");
        std::fs::write(&path, json!({ "doubao_asr_endpoint": "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel" }).to_string())
            .unwrap();
        assert_eq!(UserSettings::load(&path, Platform::Macos).doubao_asr_endpoint, crate::stt::sauc::DEFAULT_ENDPOINT);

        std::fs::write(&path, json!({ "doubao_asr_endpoint": "wss://proxy.example/sauc/bigmodel" }).to_string()).unwrap();
        assert_eq!(UserSettings::load(&path, Platform::Macos).doubao_asr_endpoint, "wss://proxy.example/sauc/bigmodel");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 加载_强制断句静音时长旧默认0升级为800_只升一次() {
        let dir = temp_dir("end-window");
        let path = dir.join("settings.json");
        // 旧数据（没有修订号）：0 是旧默认值，升级；用户改过的不动
        std::fs::write(&path, json!({ "doubao_asr_end_window_size": 0 }).to_string()).unwrap();
        let upgraded = UserSettings::load(&path, Platform::Macos);
        assert_eq!(upgraded.doubao_asr_end_window_size, 800);
        assert_eq!(upgraded.settings_revision, SETTINGS_REVISION);
        std::fs::write(&path, json!({ "doubao_asr_end_window_size": 500 }).to_string()).unwrap();
        assert_eq!(UserSettings::load(&path, Platform::Macos).doubao_asr_end_window_size, 500);

        // 升级并保存后，用户主动改回 0：不再被覆盖
        let mut settings = upgraded;
        settings.doubao_asr_end_window_size = 0;
        settings.save(&path).unwrap();
        assert_eq!(UserSettings::load(&path, Platform::Macos).doubao_asr_end_window_size, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 公共视图_密文不外发_set布尔与旧hotkey齐备() {
        let dir = temp_dir("view");
        let crypto = Crypto::load_or_create(&dir).unwrap();
        let mut settings = UserSettings::with_platform(Platform::Windows);
        settings.doubao_asr_api_key_encrypted = Some(crypto.encrypt("tok"));

        let view = settings.public_view(Platform::Windows);
        assert!(view.get("doubao_asr_api_key_encrypted").is_none(), "密文字段绝不能出现在 GET 视图");
        assert_eq!(view["doubao_asr_api_key_set"], json!(true));
        assert_eq!(view["chatgpt_asr_api_key_set"], json!(false));
        assert!(view["hotkey"].is_object(), "旧 hotkey 兼容字段必须存在");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 更新_白名单null与类型契约与密钥加解密() {
        let dir = temp_dir("update");
        let crypto = Crypto::load_or_create(&dir).unwrap();
        let mut settings = UserSettings::with_platform(Platform::Windows);

        let body = serde_json::json!({
            "asr_provider": "qwen_asr",
            "recording_mode": "streaming",
            "silence_threshold": 2.5,
            "auto_optimize": null,                 // null = 未提供
            "unknown_future_key": 123,             // 未知键静默丢弃
            "doubao_asr_api_key": "sk-test-token"
        });
        let outcome = apply_settings_update(&mut settings, &body, Platform::Windows, &crypto).unwrap();
        assert_eq!(settings.asr_provider, "qwen_asr");
        assert_eq!(settings.recording_mode, "streaming");
        assert!(!settings.auto_optimize, "null 未提供 → 保持原值");
        assert!(outcome.runtime_keys_changed.contains(&"asr_provider"));
        let cipher = settings.doubao_asr_api_key_encrypted.clone().unwrap();
        assert_eq!(crypto.decrypt(&cipher).as_deref(), Some("sk-test-token"));

        // 类型非法 → 中文错误
        let bad = serde_json::json!({"auto_optimize": "yes"});
        let err = apply_settings_update(&mut settings, &bad, Platform::Windows, &crypto).unwrap_err();
        assert!(err.contains("auto_optimize") && err.contains("布尔"), "错误信息应包含字段名与期望类型: {err}");

        // 清除密钥
        let clear = serde_json::json!({"doubao_asr_api_key_clear": true});
        apply_settings_update(&mut settings, &clear, Platform::Windows, &crypto).unwrap();
        assert!(settings.doubao_asr_api_key_encrypted.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 时间戳工具_rfc3339() {
        assert!(now_rfc3339().ends_with("Z") || now_rfc3339().contains('+'));
    }

    /// 回归锁：Python/前端写出的浮点数（-40.0）不得让 settings.json 整体解析失败
    /// （曾导致全部设置回落默认值，保存时覆盖用户真实配置）。
    #[test]
    fn 加载_浮点数整数字段_宽容取整不回落默认() {
        let dir = temp_dir("float");
        let path = dir.join("settings.json");
        std::fs::write(
            &path,
            serde_json::json!({
                "asr_provider": "chatgpt_asr",
                "silence_level": -40.0,                 // Python 历史数据实况
                "qwen_asr_max_new_tokens": 448.0,       // 同类风险字段
                "chatgpt_asr_timeout_seconds": 90.0,
                "auto_optimize": true
            })
            .to_string(),
        )
        .unwrap();

        let settings = UserSettings::load(&path, Platform::Windows);
        assert_eq!(settings.asr_provider, "chatgpt_asr", "用户配置必须保留，不得回落默认");
        assert_eq!(settings.silence_level, -40, "浮点整数应取整读取");
        assert_eq!(settings.qwen_asr_max_new_tokens, 448);
        assert_eq!(settings.chatgpt_asr_timeout_seconds, 90);
        assert!(settings.auto_optimize);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 回归锁：PUT 收到浮点数（前端滑杆常见）应取整保存而非报错。
    #[test]
    fn 更新_浮点数整数字段_取整保存() {
        let dir = temp_dir("float-put");
        let crypto = Crypto::load_or_create(&dir).unwrap();
        let mut settings = UserSettings::with_platform(Platform::Windows);

        let body = serde_json::json!({ "silence_level": -35.5, "chatgpt_asr_timeout_seconds": 60.0 });
        apply_settings_update(&mut settings, &body, Platform::Windows, &crypto).unwrap();
        assert_eq!(settings.silence_level, -36, "浮点按四舍五入取整");
        assert_eq!(settings.chatgpt_asr_timeout_seconds, 60);

        // 非数字仍是 400 中文错误
        let bad = serde_json::json!({ "silence_level": "loud" });
        let err = apply_settings_update(&mut settings, &bad, Platform::Windows, &crypto).unwrap_err();
        assert!(err.contains("silence_level") && err.contains("整数"), "错误信息应包含字段名与期望类型: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 剪贴板悬浮框_旧数据补默认绑定_公共视图带开关与快捷键() {
        let dir = temp_dir("panel-load");
        let path = dir.join("settings.json");
        std::fs::write(&path, serde_json::json!({
            "hotkeys": [{"action": "record", "trigger": "hold", "modifiers": {"alt": true, "meta": true}, "enabled": true}]
        }).to_string()).unwrap();

        let settings = UserSettings::load(&path, Platform::Macos);
        assert!(settings.clipboard_panel_enabled());
        assert_eq!(settings.clipboard_panel_enter_action, "paste");
        assert_eq!((settings.clipboard_panel_height, settings.clipboard_panel_max_width), (244, 1280), "旧数据补默认尺寸");
        let view = settings.public_view(Platform::Macos);
        assert_eq!(view["clipboard_panel_enabled"], true);
        assert_eq!(view["clipboard_panel_hotkey"]["display"], "⌃⌥V");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 剪贴板悬浮框_改键开关回车行为_带回旧hotkeys也不覆盖() {
        let dir = temp_dir("panel-update");
        let crypto = Crypto::load_or_create(&dir).unwrap();
        let mut settings = UserSettings::with_platform(Platform::Macos);
        let stale_hotkeys = serde_json::to_value(&settings.hotkeys).unwrap();

        let body = serde_json::json!({
            "hotkeys": stale_hotkeys,
            "clipboard_panel_hotkey": {"modifiers": ["cmd", "shift"], "key": "b"},
            "clipboard_panel_enabled": false,
            "clipboard_panel_enter_action": "copy"
        });
        apply_settings_update(&mut settings, &body, Platform::Macos, &crypto).unwrap();
        let binding = settings.binding_of(HotkeyAction::ClipboardPanel).unwrap();
        assert_eq!(binding.display(Platform::Macos), "⌘⇧B");
        assert_eq!(binding.trigger, TriggerMode::Toggle, "改键不改触发模式");
        assert!(!settings.clipboard_panel_enabled());
        assert_eq!(settings.clipboard_panel_enter_action, "copy");

        // 尺寸只由悬浮框拖动写入：设置页 PUT 带回的旧值被忽略
        settings.set_clipboard_panel_size(1600, 300);
        let stale = serde_json::json!({"clipboard_panel_height": 244, "clipboard_panel_max_width": 1280});
        apply_settings_update(&mut settings, &stale, Platform::Macos, &crypto).unwrap();
        assert_eq!((settings.clipboard_panel_max_width, settings.clipboard_panel_height), (1600, 300));
        settings.set_clipboard_panel_size(100, 999);
        assert_eq!(settings.clipboard_panel_height, CLIPBOARD_PANEL_HEIGHT_RANGE.1, "高度夹到范围内");
        assert_eq!(settings.clipboard_panel_max_width, CLIPBOARD_PANEL_WIDTH_RANGE.0, "宽度夹到范围内");
        settings.set_clipboard_panel_size(0, 244);
        assert_eq!(settings.clipboard_panel_max_width, 0, "0 = 铺满屏幕");
        settings.reset_clipboard_panel_size();
        assert_eq!((settings.clipboard_panel_max_width, settings.clipboard_panel_height), (1280, 244));

        // 位置同理：只由拖动写入，PUT 带回的旧值忽略；负数夹成贴底，恢复默认回到贴底
        settings.set_clipboard_panel_offset(320);
        let stale = serde_json::json!({"clipboard_panel_bottom_offset": 0});
        apply_settings_update(&mut settings, &stale, Platform::Macos, &crypto).unwrap();
        assert_eq!(settings.clipboard_panel_bottom_offset, 320);
        settings.set_clipboard_panel_offset(-5);
        assert_eq!(settings.clipboard_panel_bottom_offset, 0);
        settings.set_clipboard_panel_offset(200);
        settings.reset_clipboard_panel_position();
        assert_eq!(settings.clipboard_panel_bottom_offset, 0);

        let bad = serde_json::json!({"clipboard_panel_enter_action": "type"});
        assert!(apply_settings_update(&mut settings, &bad, Platform::Macos, &crypto).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 剪贴板悬浮框_与录音撞键时拒绝_关掉后放行() {
        let dir = temp_dir("panel-conflict");
        let crypto = Crypto::load_or_create(&dir).unwrap();
        let mut settings = UserSettings::with_platform(Platform::Macos);
        let same = serde_json::json!({"clipboard_panel_hotkey": {"modifiers": ["cmd", "shift"], "key": "r"}});
        let err = apply_settings_update(&mut settings, &same, Platform::Macos, &crypto).unwrap_err();
        assert!(err.contains("录音快捷键"), "{err}");

        let mut settings = UserSettings::with_platform(Platform::Macos);
        let disabled = serde_json::json!({
            "clipboard_panel_hotkey": {"modifiers": ["cmd", "shift"], "key": "r"},
            "clipboard_panel_enabled": false
        });
        assert!(apply_settings_update(&mut settings, &disabled, Platform::Macos, &crypto).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 点按录音_旧数据补空绑定_可与按住录音分键_同键只能启用一种() {
        let dir = temp_dir("record-toggle");
        let path = dir.join("settings.json");
        std::fs::write(&path, serde_json::json!({
            "hotkeys": [{"action": "record", "trigger": "hold", "modifiers": {"alt": true, "meta": true}, "enabled": true}]
        }).to_string()).unwrap();
        let mut settings = UserSettings::load(&path, Platform::Macos);
        let toggle = settings.binding_of(HotkeyAction::RecordToggle).unwrap();
        assert!(!toggle.enabled, "旧数据补上的点按录音默认为空");
        assert_eq!(toggle.trigger, TriggerMode::Toggle);
        let view = settings.public_view(Platform::Macos);
        assert_eq!(view["hotkey"]["display"], "⌘⌥");
        assert_eq!(view["hotkey_enabled"], true);
        assert_eq!(view["record_toggle_enabled"], false);

        // 分键：两种都启用
        let crypto = Crypto::load_or_create(&dir).unwrap();
        let body = serde_json::json!({
            "record_toggle_hotkey": {"modifiers": ["cmd", "shift"], "key": "t"},
            "record_toggle_enabled": true
        });
        let outcome = apply_settings_update(&mut settings, &body, Platform::Macos, &crypto).unwrap();
        assert!(outcome.restart_hotkey_backend);
        let toggle = settings.binding_of(HotkeyAction::RecordToggle).unwrap();
        assert_eq!((toggle.display(Platform::Macos), toggle.enabled), ("⌘⇧T".to_string(), true));
        assert!(settings.binding_of(HotkeyAction::Record).unwrap().enabled);

        // 同键且都启用：拒绝，提示先清空一个
        let same = serde_json::json!({"record_toggle_hotkey": {"modifiers": ["cmd", "alt"]}});
        let err = apply_settings_update(&mut settings.clone(), &same, Platform::Macos, &crypto).unwrap_err();
        assert!(err.contains("先清空"), "{err}");

        // 同键、清空按住录音：放行，只剩点按
        let switch = serde_json::json!({"record_toggle_hotkey": {"modifiers": ["cmd", "alt"]}, "hotkey_enabled": false});
        apply_settings_update(&mut settings, &switch, Platform::Macos, &crypto).unwrap();
        assert!(!settings.binding_of(HotkeyAction::Record).unwrap().enabled);
        assert_eq!(settings.binding_of(HotkeyAction::Record).unwrap().display(Platform::Macos), "⌘⌥", "清空只关开关，组合保留");
        assert_eq!(settings.public_view(Platform::Macos)["hotkey_enabled"], false);
        settings.save(&path).unwrap();
        let reloaded = UserSettings::load(&path, Platform::Macos);
        assert!(!reloaded.binding_of(HotkeyAction::Record).unwrap().enabled, "保存后重新读取仍为空");
        assert!(reloaded.binding_of(HotkeyAction::RecordToggle).unwrap().enabled);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 点按录音_旧数据录音绑定为点按时挪成点按录音() {
        let dir = temp_dir("record-toggle-migrate");
        let path = dir.join("settings.json");
        std::fs::write(&path, serde_json::json!({
            "hotkeys": [{"action": "record", "trigger": "toggle", "modifiers": {"alt": true, "meta": true}, "enabled": true}]
        }).to_string()).unwrap();
        let settings = UserSettings::load(&path, Platform::Windows);
        let toggle = settings.binding_of(HotkeyAction::RecordToggle).unwrap();
        assert!(toggle.enabled);
        assert_eq!(toggle.display(Platform::Windows), "Win+Alt");
        let hold = settings.binding_of(HotkeyAction::Record).unwrap();
        assert!(!hold.enabled, "按住录音留空，行为与旧版一致");
        assert_eq!(hold.trigger, TriggerMode::Hold);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 优化选中文本_旧数据补默认绑定_改键开关预设_撞键拒绝() {
        let dir = temp_dir("rewrite");
        let path = dir.join("settings.json");
        std::fs::write(&path, serde_json::json!({
            "asr_provider": "doubao_asr",
            "hotkeys": [
                {"action": "record", "trigger": "hold", "modifiers": {"alt": true, "meta": true}, "enabled": true},
                {"action": "future_action", "trigger": "toggle", "modifiers": {"ctrl": true}, "enabled": true}
            ]
        }).to_string()).unwrap();
        let mut settings = UserSettings::load(&path, Platform::Macos);
        assert_eq!(settings.asr_provider, "doubao_asr", "认不出的绑定不拖垮整份设置");
        assert_eq!(settings.binding_of(HotkeyAction::Record).unwrap().display(Platform::Macos), "⌘⌥");
        assert!(settings.rewrite_preset_id.is_none(), "默认跟随默认预设");
        assert!(!settings.auto_download_updates, "旧数据没有该字段 = 不自动下载更新");
        let view = settings.public_view(Platform::Macos);
        assert_eq!(view["auto_download_updates"], false);
        assert_eq!(view["rewrite_hotkey"]["display"], "⌥A");
        assert_eq!(view["rewrite_enabled"], true);

        let crypto = Crypto::load_or_create(&dir).unwrap();
        let body = serde_json::json!({
            "rewrite_hotkey": {"modifiers": ["ctrl", "alt"], "key": "p"},
            "rewrite_enabled": false,
            "rewrite_preset_id": "preset-程序员模式"
        });
        let outcome = apply_settings_update(&mut settings, &body, Platform::Macos, &crypto).unwrap();
        assert!(outcome.restart_hotkey_backend);
        let binding = settings.binding_of(HotkeyAction::RewriteSelection).unwrap();
        assert_eq!(binding.display(Platform::Macos), "⌃⌥P");
        assert_eq!(binding.trigger, TriggerMode::Toggle);
        assert!(!binding.enabled);
        assert_eq!(settings.rewrite_preset_id.as_deref(), Some("preset-程序员模式"));
        apply_settings_update(&mut settings, &serde_json::json!({"rewrite_preset_id": ""}), Platform::Macos, &crypto).unwrap();
        assert!(settings.rewrite_preset_id.is_none(), "空串 = 跟随默认");

        // 自动下载更新：布尔键，保存后重新读取仍保持
        apply_settings_update(&mut settings, &serde_json::json!({"auto_download_updates": true}), Platform::Macos, &crypto).unwrap();
        settings.save(&path).unwrap();
        assert!(UserSettings::load(&path, Platform::Macos).auto_download_updates);
        assert!(apply_settings_update(&mut settings, &serde_json::json!({"auto_download_updates": "yes"}), Platform::Macos, &crypto).is_err());

        // 关着的时候可以和剪贴板悬浮框同键；打开就拒绝
        let same = serde_json::json!({"rewrite_hotkey": {"modifiers": ["ctrl", "alt"], "key": "v"}});
        apply_settings_update(&mut settings, &same, Platform::Macos, &crypto).unwrap();
        let err = apply_settings_update(&mut settings, &serde_json::json!({"rewrite_enabled": true}), Platform::Macos, &crypto)
            .unwrap_err();
        assert_eq!(err, "优化选中文本的快捷键与剪贴板悬浮框快捷键相同，请换一个");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 热词_更新时归一化_旧数据结构异常不拖垮整份设置() {
        let dir = temp_dir("hotwords");
        let crypto = Crypto::load_or_create(&dir).unwrap();
        let mut settings = UserSettings::with_platform(Platform::Macos);
        let update = serde_json::json!({"asr_hotwords": [" Tauri ", "tauri", "", 3, "落地页"]});
        apply_settings_update(&mut settings, &update, Platform::Macos, &crypto).unwrap();
        assert_eq!(settings.asr_hotwords, vec!["Tauri".to_string(), "落地页".into()]);
        assert!(apply_settings_update(&mut settings, &serde_json::json!({"asr_hotwords": "Tauri"}), Platform::Macos, &crypto).is_err());

        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{"asr_provider": "doubao_asr", "asr_hotwords": "不是数组"}"#).unwrap();
        let loaded = UserSettings::load(&path, Platform::Macos);
        assert_eq!(loaded.asr_provider, "doubao_asr");
        assert!(loaded.asr_hotwords.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
