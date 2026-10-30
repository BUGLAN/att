//! LLM 档案 + 模型存储（`data/llm_v2.json`，移植自 Python `app/llm/store.py` / `migration.py`）。
//!
//! - 文件格式与 Python 版完全一致（两版可互读），写入走原子写；
//! - 加载**逐条容错**：单条数据损坏只跳过该条并记入 `skipped_entries`，其余照常加载；
//! - v2 文件不存在而 v1 `llm_configs.json` 存在时自动迁移，v1 改名 `.v1.bak` 保留回滚锚点；
//! - 密钥只存档案级密文（Fernet，与 Python 互认），对外视图一律剔除 `api_key_encrypted`。

use super::errors::{LlmError, CODE_AUTH, CODE_MODEL_NOT_FOUND};
use super::registry;
use crate::storage::crypto::Crypto;
use crate::storage::json_store;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const STORE_FILE_NAME: &str = "llm_v2.json";
pub const LEGACY_FILE_NAME: &str = "llm_configs.json";
const LEGACY_BACKUP_SUFFIX: &str = ".v1.bak";
/// v1 时代豆包档案的默认端点（迁移 custom 且无 base 时兜底）
const DOUBAO_LEGACY_BASE: &str = "https://ark.cn-beijing.volces.com/api/v3";
/// 思考强度取值（空串 = 不干预）
const REASONING_EFFORTS: [&str; 5] = ["", "none", "low", "medium", "high"];

fn now_iso() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string()
}

/// 时间戳 + 短随机：避免批量创建时 id 碰撞。
fn new_id(prefix: &str) -> String {
    let random = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}-{}{}", chrono::Local::now().timestamp_millis(), &random[..4])
}

/// 可安全展示的密钥尾号提示（非敏感）。
fn key_hint(plain_key: &str) -> String {
    let key = plain_key.trim();
    if key.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = key.chars().collect();
    let tail: String = if chars.len() >= 8 { chars[chars.len() - 4..].iter().collect() } else { "****".into() };
    if key.starts_with("sk-") { format!("sk-****{tail}") } else { format!("****{tail}") }
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|v| !v.is_empty()).map(str::to_string)
}

// ==================== 数据模型 ====================

/// 最近一次验证结果（档案与模型共用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VerifyStatus {
    pub state: String,
    pub checked_at: Option<String>,
    pub latency_ms: Option<i64>,
    pub error_code: Option<String>,
    pub error: Option<String>,
}

impl Default for VerifyStatus {
    fn default() -> Self {
        Self { state: "unknown".into(), checked_at: None, latency_ms: None, error_code: None, error: None }
    }
}

impl VerifyStatus {
    pub fn unknown_now() -> Self {
        Self { checked_at: Some(now_iso()), ..Default::default() }
    }

    /// 网关验证结果 → 可持久化的状态。
    pub fn from_outcome(outcome: &super::gateway::VerifyOutcome) -> Self {
        Self {
            state: if outcome.ok { "ok" } else { "failed" }.into(),
            checked_at: Some(now_iso()),
            latency_ms: outcome.latency_ms,
            error_code: if outcome.ok { None } else { outcome.error_code.clone() },
            error: outcome.message.clone(),
        }
    }

    pub fn failed(error: &LlmError) -> Self {
        Self {
            state: "failed".into(),
            checked_at: Some(now_iso()),
            latency_ms: None,
            error_code: Some(error.code.to_string()),
            error: Some(error.user_message.clone()),
        }
    }
}

/// 模型级调用参数（超时/重试由网关每次请求强制注入）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelParams {
    pub temperature: f64,
    pub max_tokens: i64,
    pub timeout_s: i64,
    pub retries: i64,
    pub reasoning_effort: String,
}

impl Default for ModelParams {
    fn default() -> Self {
        Self { temperature: 0.3, max_tokens: 4096, timeout_s: 60, retries: 2, reasoning_effort: String::new() }
    }
}

impl ModelParams {
    /// 请求体校验（对应 Pydantic 的 ge/le 约束）；思考强度未知取值降级为「不干预」。
    pub fn validated(mut self) -> Result<Self, String> {
        if !(5..=600).contains(&self.timeout_s) {
            return Err("timeout_s 必须在 5~600 秒之间".into());
        }
        if !(0..=5).contains(&self.retries) {
            return Err("retries 必须在 0~5 之间".into());
        }
        self.normalize_effort();
        Ok(self)
    }

    /// 加载期宽容处理：越界值钳回合法区间，不让一条脏参数让整个模型条目消失。
    fn sanitized(mut self) -> Self {
        self.timeout_s = self.timeout_s.clamp(5, 600);
        self.retries = self.retries.clamp(0, 5);
        self.normalize_effort();
        self
    }

    fn normalize_effort(&mut self) {
        let effort = self.reasoning_effort.trim().to_lowercase();
        self.reasoning_effort = if REASONING_EFFORTS.contains(&effort.as_str()) { effort } else { String::new() };
    }
}

/// 供应商档案：一家服务商的凭据与端点，密钥只存这一份。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderProfile {
    pub id: String,
    pub provider: String,
    pub name: String,
    #[serde(default)]
    pub api_key_encrypted: String,
    #[serde(default)]
    pub api_key_hint: String,
    #[serde(default)]
    pub api_base: Option<String>,
    #[serde(default)]
    pub extra_headers: BTreeMap<String, String>,
    #[serde(default)]
    pub extra_query: BTreeMap<String, String>,
    #[serde(default)]
    pub status: VerifyStatus,
}

/// 模型条目：挂在某个档案下的可调用模型。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub profile_id: String,
    /// 完整模型串（服务商前缀/模型名）
    pub model: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub params: ModelParams,
    #[serde(default = "default_use_cases")]
    pub use_cases: BTreeMap<String, bool>,
    #[serde(default)]
    pub status: VerifyStatus,
}

fn default_use_cases() -> BTreeMap<String, bool> {
    BTreeMap::from([("optimize".to_string(), true)])
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreData {
    pub version: i64,
    pub profiles: Vec<ProviderProfile>,
    pub models: Vec<ModelEntry>,
    pub default_model_id: Option<String>,
    pub fallback_model_id: Option<String>,
}

impl Default for StoreData {
    fn default() -> Self {
        Self { version: 2, profiles: Vec::new(), models: Vec::new(), default_model_id: None, fallback_model_id: None }
    }
}

// ==================== 请求模型 ====================

#[derive(Debug, Deserialize)]
pub struct ProfileCreate {
    pub provider: String,
    pub name: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub api_base: Option<String>,
    #[serde(default)]
    pub extra_headers: BTreeMap<String, String>,
    #[serde(default)]
    pub extra_query: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ProfileUpdate {
    pub name: Option<String>,
    /// None / 空串 = 保持现有密钥
    pub api_key: Option<String>,
    pub api_base: Option<String>,
    pub extra_headers: Option<BTreeMap<String, String>>,
    pub extra_query: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize)]
pub struct ModelItemCreate {
    pub profile_id: String,
    /// 裸名（按档案前缀组装）或完整模型串
    pub model: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub params: Option<ModelParams>,
    #[serde(default)]
    pub set_default: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct ModelUpdate {
    pub display_name: Option<String>,
    pub model: Option<String>,
    pub params: Option<ModelParams>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DraftVerify {
    pub provider: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub api_base: Option<String>,
    #[serde(default)]
    pub extra_headers: BTreeMap<String, String>,
    /// 提供时验证到模型级，否则只验证档案级
    #[serde(default)]
    pub model_name: Option<String>,
}

// ==================== 存储 ====================

#[derive(Debug, PartialEq, Eq)]
pub enum StoreError {
    NotFound,
    /// 删除对象包含/就是默认优化模型（409）
    ConflictDefault,
    Invalid(String),
    Io(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::NotFound => f.write_str("对象不存在"),
            StoreError::ConflictDefault => f.write_str("包含默认优化模型"),
            StoreError::Invalid(message) | StoreError::Io(message) => f.write_str(message),
        }
    }
}

/// 运行时调用三元组（密钥明文只活在内存里）。
#[derive(Clone)]
pub struct RuntimeTarget {
    pub model: ModelEntry,
    pub profile: ProviderProfile,
    pub api_key: String,
}

struct Inner {
    data: StoreData,
    /// 因数据损坏被跳过的条目（设置页要提示用户，不能静默吞掉）
    skipped: Vec<String>,
}

pub struct LlmStore {
    file: PathBuf,
    crypto: Arc<Crypto>,
    inner: Mutex<Inner>,
}

impl LlmStore {
    /// 加载 v2；不存在时尝试迁移 v1；都没有则从空配置开始。
    pub fn load(data_dir: &Path, crypto: Arc<Crypto>) -> Self {
        let file = data_dir.join(STORE_FILE_NAME);
        let legacy = data_dir.join(LEGACY_FILE_NAME);
        let store = Self { file, crypto, inner: Mutex::new(Inner { data: StoreData::default(), skipped: Vec::new() }) };
        if store.file.exists() {
            store.load_v2();
        } else if legacy.exists() {
            store.migrate_legacy(&legacy);
        } else {
            tracing::info!("未找到 LLM 配置文件，从空配置开始");
        }
        store
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn save(&self, data: &StoreData) -> Result<(), StoreError> {
        json_store::write_json_atomic(&self.file, data).map_err(|e| {
            tracing::error!("写入 LLM 配置失败: {e}");
            StoreError::Io(format!("保存 LLM 配置失败：{e}"))
        })
    }

    fn load_v2(&self) {
        let raw = json_store::ensure_object(json_store::read_json_lenient::<Value>(&self.file));
        let mut skipped = Vec::new();
        let label = |item: &Value, idx: usize| item.get("id").and_then(Value::as_str).map(str::to_string).unwrap_or(idx.to_string());

        let mut profiles = Vec::new();
        for (idx, item) in raw.get("profiles").and_then(Value::as_array).into_iter().flatten().enumerate() {
            match serde_json::from_value::<ProviderProfile>(item.clone()) {
                Ok(profile) => profiles.push(profile),
                Err(error) => {
                    tracing::warn!("跳过损坏的 LLM 档案 #{idx}: {error}");
                    skipped.push(format!("profile:{}", label(item, idx)));
                }
            }
        }
        let mut models = Vec::new();
        for (idx, item) in raw.get("models").and_then(Value::as_array).into_iter().flatten().enumerate() {
            match serde_json::from_value::<ModelEntry>(item.clone()) {
                Ok(mut entry) => {
                    entry.params = entry.params.sanitized();
                    models.push(entry);
                }
                Err(error) => {
                    tracing::warn!("跳过损坏的 LLM 模型 #{idx}: {error}");
                    skipped.push(format!("model:{}", label(item, idx)));
                }
            }
        }
        let binding = |key: &str| raw.get(key).and_then(Value::as_str).map(str::to_string);
        let mut data = StoreData {
            version: 2,
            profiles,
            models,
            default_model_id: binding("default_model_id"),
            fallback_model_id: binding("fallback_model_id"),
        };
        if !skipped.is_empty() {
            tracing::warn!("LLM 配置存在损坏条目，已跳过: {skipped:?}");
        }
        let healed = heal_custom_model_strings(&mut data);
        let repaired = repair_bindings(&mut data);
        if healed || (repaired && !data.models.is_empty()) {
            let _ = self.save(&data);
        }
        *self.lock() = Inner { data, skipped };
    }

    fn migrate_legacy(&self, legacy: &Path) {
        tracing::info!("检测到 v1 LLM 配置，开始自动迁移: {}", legacy.display());
        let raw = json_store::ensure_object(json_store::read_json_lenient::<Value>(legacy));
        let (data, skipped) = migrate_v1(&raw, &self.crypto);
        if self.save(&data).is_err() {
            // 落盘失败：保留 v1 文件，下次启动重试迁移；本次仍以内存结果服务
            *self.lock() = Inner { data, skipped };
            return;
        }
        let backup = legacy.with_file_name(format!("{LEGACY_FILE_NAME}{LEGACY_BACKUP_SUFFIX}"));
        if let Err(error) = std::fs::rename(legacy, &backup) {
            tracing::error!("v1 配置改名备份失败（不影响 v2 使用）: {error}");
        }
        tracing::info!(
            "v1 LLM 配置迁移完成：档案 {} / 模型 {} / 跳过 {:?}",
            data.profiles.len(),
            data.models.len(),
            skipped
        );
        *self.lock() = Inner { data, skipped };
    }

    /// 在锁内修改并落盘；落盘失败时回滚内存，保证内存与文件一致。
    fn mutate<T>(&self, f: impl FnOnce(&mut StoreData) -> Result<T, StoreError>) -> Result<T, StoreError> {
        let mut inner = self.lock();
        let mut draft = inner.data.clone();
        let outcome = f(&mut draft)?;
        self.save(&draft)?;
        inner.data = draft;
        Ok(outcome)
    }

    // ==================== 视图 ====================

    /// GET /api/llm/state：档案 + 模型 + 默认绑定 + 损坏条目提示。
    pub fn overview(&self) -> Value {
        let inner = self.lock();
        json!({
            "version": inner.data.version,
            "profiles": inner.data.profiles.iter().map(profile_public).collect::<Vec<_>>(),
            "models": inner.data.models.iter().map(model_public).collect::<Vec<_>>(),
            "default_model_id": inner.data.default_model_id,
            "fallback_model_id": inner.data.fallback_model_id,
            "skipped_entries": inner.skipped,
        })
    }

    pub fn list_profiles(&self) -> Vec<ProviderProfile> {
        self.lock().data.profiles.clone()
    }

    pub fn get_profile(&self, id: &str) -> Option<ProviderProfile> {
        self.lock().data.profiles.iter().find(|p| p.id == id).cloned()
    }

    pub fn list_models(&self, profile_id: Option<&str>) -> Vec<ModelEntry> {
        let inner = self.lock();
        inner.data.models.iter().filter(|m| profile_id.map(|p| m.profile_id == p).unwrap_or(true)).cloned().collect()
    }

    pub fn get_model(&self, id: &str) -> Option<ModelEntry> {
        self.lock().data.models.iter().find(|m| m.id == id).cloned()
    }

    pub fn default_model_id(&self) -> Option<String> {
        let inner = self.lock();
        inner.data.default_model_id.clone().or_else(|| inner.data.models.first().map(|m| m.id.clone()))
    }

    pub fn fallback_model_id(&self) -> Option<String> {
        self.lock().data.fallback_model_id.clone()
    }

    // ==================== 档案 ====================

    pub fn create_profile(&self, request: ProfileCreate) -> Result<ProviderProfile, StoreError> {
        let spec = registry::get(&request.provider).ok_or_else(|| StoreError::Invalid(format!("未知服务商：{}", request.provider)))?;
        let name = request.name.trim().to_string();
        if name.is_empty() {
            return Err(StoreError::Invalid("档案名称不能为空".into()));
        }
        let api_base = non_empty(request.api_base.as_deref());
        if spec.id == "custom" && api_base.is_none() {
            return Err(StoreError::Invalid("自定义 OpenAI 兼容端点必须填写 API Base".into()));
        }
        let profile = ProviderProfile {
            id: new_id("prof"),
            provider: request.provider,
            name,
            api_key_encrypted: if request.api_key.is_empty() { String::new() } else { self.crypto.encrypt(&request.api_key) },
            api_key_hint: key_hint(&request.api_key),
            api_base,
            extra_headers: request.extra_headers,
            extra_query: request.extra_query,
            status: VerifyStatus::unknown_now(),
        };
        let created = profile.clone();
        self.mutate(|data| {
            data.profiles.push(profile);
            Ok(())
        })?;
        tracing::info!("创建 LLM 供应商档案 {}（{}）", created.id, created.provider);
        Ok(created)
    }

    pub fn update_profile(&self, id: &str, request: ProfileUpdate) -> Result<ProviderProfile, StoreError> {
        let crypto = self.crypto.clone();
        self.mutate(|data| {
            let profile = data.profiles.iter_mut().find(|p| p.id == id).ok_or(StoreError::NotFound)?;
            let mut credentials_changed = false;
            if let Some(name) = non_empty(request.name.as_deref()) {
                profile.name = name;
            }
            if let Some(key) = request.api_key.filter(|k| !k.is_empty()) {
                profile.api_key_encrypted = crypto.encrypt(&key);
                profile.api_key_hint = key_hint(&key);
                credentials_changed = true;
            }
            if let Some(base) = request.api_base {
                let base = non_empty(Some(&base));
                if base != profile.api_base {
                    if profile.provider == "custom" && base.is_none() {
                        return Err(StoreError::Invalid("自定义 OpenAI 兼容端点必须填写 API Base".into()));
                    }
                    profile.api_base = base;
                    credentials_changed = true;
                }
            }
            if let Some(headers) = request.extra_headers {
                profile.extra_headers = headers;
            }
            if let Some(query) = request.extra_query {
                profile.extra_query = query;
            }
            if credentials_changed {
                // 凭据变了，旧验证结论不再可信
                profile.status = VerifyStatus::unknown_now();
            }
            Ok(profile.clone())
        })
    }

    /// 删除档案并级联其下模型；含默认模型时拒绝。
    pub fn delete_profile(&self, id: &str) -> Result<(), StoreError> {
        self.mutate(|data| {
            if !data.profiles.iter().any(|p| p.id == id) {
                return Err(StoreError::NotFound);
            }
            let model_ids: Vec<String> = data.models.iter().filter(|m| m.profile_id == id).map(|m| m.id.clone()).collect();
            if data.default_model_id.as_ref().map(|d| model_ids.contains(d)).unwrap_or(false) {
                return Err(StoreError::ConflictDefault);
            }
            data.models.retain(|m| m.profile_id != id);
            if data.fallback_model_id.as_ref().map(|f| model_ids.contains(f)).unwrap_or(false) {
                data.fallback_model_id = None;
            }
            data.profiles.retain(|p| p.id != id);
            Ok(())
        })
    }

    pub fn set_profile_status(&self, id: &str, status: VerifyStatus) {
        let _ = self.mutate(|data| {
            let profile = data.profiles.iter_mut().find(|p| p.id == id).ok_or(StoreError::NotFound)?;
            profile.status = status;
            Ok(())
        });
    }

    /// 解密档案密钥：无密钥返回空串；密文损坏返回 auth_failed。
    pub fn decrypt_key(&self, profile: &ProviderProfile) -> Result<String, LlmError> {
        if profile.api_key_encrypted.is_empty() {
            return Ok(String::new());
        }
        self.crypto.decrypt(&profile.api_key_encrypted).ok_or_else(|| {
            tracing::error!("LLM 密钥解密失败（档案 {}）", profile.id);
            LlmError::new(CODE_AUTH, "已保存的 API Key 无法解密（可能更换了设备或加密文件），请重新填写密钥")
        })
    }

    // ==================== 模型 ====================

    /// 批量创建：返回 (created, skipped[{model, reason}])。
    pub fn create_models(&self, items: Vec<ModelItemCreate>) -> Result<(Vec<ModelEntry>, Vec<Value>), StoreError> {
        let mut inner = self.lock();
        let mut draft = inner.data.clone();
        let mut created = Vec::new();
        let mut skipped = Vec::new();
        for item in items {
            let Some(profile) = draft.profiles.iter().find(|p| p.id == item.profile_id).cloned() else {
                skipped.push(json!({ "model": item.model, "reason": format!("档案不存在：{}", item.profile_id) }));
                continue;
            };
            let model_string = match registry::compose_model_string(&profile.provider, &item.model, profile.api_base.as_deref()) {
                Ok(model) => model,
                Err(reason) => {
                    skipped.push(json!({ "model": item.model, "reason": reason }));
                    continue;
                }
            };
            if draft.models.iter().any(|m| m.profile_id == profile.id && m.model == model_string) {
                skipped.push(json!({ "model": item.model, "reason": "该档案下已存在同名模型" }));
                continue;
            }
            let params = match item.params.map(ModelParams::validated).transpose() {
                Ok(params) => params.unwrap_or_default(),
                Err(reason) => {
                    skipped.push(json!({ "model": item.model, "reason": reason }));
                    continue;
                }
            };
            let display_name = non_empty(Some(&item.display_name)).unwrap_or_else(|| item.model.trim().to_string());
            let entry = ModelEntry {
                id: new_id("mdl"),
                profile_id: profile.id.clone(),
                model: model_string,
                display_name,
                params,
                use_cases: default_use_cases(),
                status: VerifyStatus::default(),
            };
            if item.set_default || draft.default_model_id.is_none() {
                draft.default_model_id = Some(entry.id.clone());
            }
            draft.models.push(entry.clone());
            created.push(entry);
        }
        if !created.is_empty() {
            self.save(&draft)?;
            inner.data = draft;
        }
        tracing::info!("批量创建 LLM 模型：成功 {} / 跳过 {}", created.len(), skipped.len());
        Ok((created, skipped))
    }

    pub fn update_model(&self, id: &str, request: ModelUpdate) -> Result<ModelEntry, StoreError> {
        let params = request.params.map(ModelParams::validated).transpose().map_err(StoreError::Invalid)?;
        self.mutate(|data| {
            let profiles = data.profiles.clone();
            let entry = data.models.iter_mut().find(|m| m.id == id).ok_or(StoreError::NotFound)?;
            if let Some(display_name) = request.display_name {
                entry.display_name = non_empty(Some(&display_name)).unwrap_or_else(|| entry.model.clone());
            }
            if let Some(model) = request.model {
                let profile = profiles
                    .iter()
                    .find(|p| p.id == entry.profile_id)
                    .ok_or_else(|| StoreError::Invalid(format!("档案不存在：{}", entry.profile_id)))?;
                entry.model = registry::compose_model_string(&profile.provider, &model, profile.api_base.as_deref())
                    .map_err(StoreError::Invalid)?;
            }
            if let Some(params) = params {
                entry.params = params;
            }
            Ok(entry.clone())
        })
    }

    pub fn delete_model(&self, id: &str) -> Result<(), StoreError> {
        self.mutate(|data| {
            if !data.models.iter().any(|m| m.id == id) {
                return Err(StoreError::NotFound);
            }
            if data.default_model_id.as_deref() == Some(id) {
                return Err(StoreError::ConflictDefault);
            }
            if data.fallback_model_id.as_deref() == Some(id) {
                data.fallback_model_id = None;
            }
            data.models.retain(|m| m.id != id);
            Ok(())
        })
    }

    pub fn set_model_status(&self, id: &str, status: VerifyStatus) {
        let _ = self.mutate(|data| {
            let entry = data.models.iter_mut().find(|m| m.id == id).ok_or(StoreError::NotFound)?;
            entry.status = status;
            Ok(())
        });
    }

    pub fn set_default_model(&self, id: &str) -> Result<(), StoreError> {
        self.mutate(|data| {
            if !data.models.iter().any(|m| m.id == id) {
                return Err(StoreError::NotFound);
            }
            data.default_model_id = Some(id.to_string());
            Ok(())
        })
    }

    /// None = 清除备用模型（失败不自动切换）。
    pub fn set_fallback_model(&self, id: Option<&str>) -> Result<(), StoreError> {
        self.mutate(|data| {
            if let Some(id) = id {
                if !data.models.iter().any(|m| m.id == id) {
                    return Err(StoreError::NotFound);
                }
            }
            data.fallback_model_id = id.map(str::to_string);
            Ok(())
        })
    }

    /// 解析可调用三元组（模型、档案、明文密钥）；model_id 为空时取默认模型。
    pub fn resolve_runtime(&self, model_id: Option<&str>) -> Result<RuntimeTarget, LlmError> {
        let model_id = model_id.map(str::to_string).or_else(|| self.default_model_id());
        let model = model_id
            .as_deref()
            .and_then(|id| self.get_model(id))
            .ok_or_else(|| LlmError::plain(CODE_MODEL_NOT_FOUND, "尚未配置可用的 LLM 模型，请在设置页添加"))?;
        let profile = self
            .get_profile(&model.profile_id)
            .ok_or_else(|| LlmError::plain(CODE_MODEL_NOT_FOUND, format!("模型所属的供应商档案不存在：{}", model.profile_id)))?;
        let api_key = self.decrypt_key(&profile)?;
        Ok(RuntimeTarget { model, profile, api_key })
    }
}

/// 档案的安全序列化：剔除密文字段（硬约束：任何响应不得包含密钥）。
pub fn profile_public(profile: &ProviderProfile) -> Value {
    let mut value = serde_json::to_value(profile).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.remove("api_key_encrypted");
    }
    value
}

pub fn model_public(entry: &ModelEntry) -> Value {
    serde_json::to_value(entry).unwrap_or(Value::Null)
}

/// 自愈：custom 档案下未带 openai/ 前缀的历史模型串补齐前缀。
fn heal_custom_model_strings(data: &mut StoreData) -> bool {
    let custom_ids: Vec<String> = data.profiles.iter().filter(|p| p.provider == "custom").map(|p| p.id.clone()).collect();
    let mut changed = false;
    for entry in data.models.iter_mut().filter(|m| custom_ids.contains(&m.profile_id)) {
        if !entry.model.starts_with("openai/") {
            entry.model = format!("openai/{}", entry.model);
            changed = true;
        }
    }
    changed
}

/// default/fallback 指向不存在的模型时自动回落。
fn repair_bindings(data: &mut StoreData) -> bool {
    let valid = |id: &Option<String>| id.as_ref().map(|id| data.models.iter().any(|m| &m.id == id)).unwrap_or(false);
    let mut changed = false;
    if !valid(&data.default_model_id) {
        let fallback = data.models.first().map(|m| m.id.clone());
        changed |= data.default_model_id != fallback;
        data.default_model_id = fallback;
    }
    if data.fallback_model_id.is_some() && !valid(&data.fallback_model_id) {
        data.fallback_model_id = None;
        changed = true;
    }
    changed
}

/// v1 → v2：按「provider + 解密后 key + api_base」聚合档案，每条 v1 配置生成一个模型条目。
fn migrate_v1(raw: &Value, crypto: &Crypto) -> (StoreData, Vec<String>) {
    let mut data = StoreData::default();
    let mut skipped = Vec::new();
    let Some(entries) = raw.get("configs").and_then(Value::as_array) else {
        return (data, skipped);
    };
    let v1_default = raw.get("default_config_id").and_then(Value::as_str);
    // (provider, key 标记, api_base) → 档案下标
    type ProfileKey = (String, String, Option<String>);
    let mut index: Vec<(ProfileKey, usize)> = Vec::new();

    for (idx, item) in entries.iter().enumerate() {
        let Some(object) = item.as_object() else {
            skipped.push(format!("entry:{idx}:非对象"));
            continue;
        };
        let text = |key: &str| object.get(key).and_then(Value::as_str).unwrap_or("").trim().to_string();
        let config_id = non_empty(Some(&text("id"))).unwrap_or_else(|| format!("entry:{idx}"));
        let provider_v1 = text("provider");
        let provider = registry::v1_provider_id(&provider_v1);
        let model_name = text("model");
        if model_name.is_empty() {
            skipped.push(format!("{config_id}:缺少模型名"));
            continue;
        }
        let mut api_base = non_empty(Some(&text("api_base")));
        if api_base.is_none() && provider_v1.eq_ignore_ascii_case("doubao") {
            api_base = Some(DOUBAO_LEGACY_BASE.to_string());
        }
        let model_string = match registry::compose_model_string(provider, &model_name, api_base.as_deref()) {
            Ok(model) => model,
            Err(_) => {
                skipped.push(format!("{config_id}:custom 端点缺失，无法确定 API Base"));
                continue;
            }
        };
        let encrypted = text("api_key_encrypted");
        let plain = if encrypted.is_empty() { Some(String::new()) } else { crypto.decrypt(&encrypted) };
        let decrypt_failed = plain.is_none();
        let plain = plain.unwrap_or_default();
        let legacy_name = text("name");

        // 解密失败的用密文本身做标记：同密文归同一档案，便于统一重填
        let marker = if decrypt_failed { format!("enc:{encrypted}") } else { plain.clone() };
        let key = (provider.to_string(), marker, api_base.clone());
        let profile_idx = match index.iter().find(|(k, _)| *k == key) {
            Some((_, profile_idx)) => *profile_idx,
            None => {
                let spec = registry::get(provider).expect("v1 映射只产出注册表内的 id");
                let status = if decrypt_failed {
                    VerifyStatus {
                        state: "decrypt_failed".into(),
                        checked_at: Some(now_iso()),
                        latency_ms: None,
                        error_code: Some(CODE_AUTH.into()),
                        error: Some("已保存的密钥无法解密，请重新填写".into()),
                    }
                } else {
                    VerifyStatus::unknown_now()
                };
                data.profiles.push(ProviderProfile {
                    id: format!("prof-m{idx}{}", index.len()),
                    provider: provider.to_string(),
                    name: non_empty(Some(&legacy_name)).unwrap_or_else(|| spec.name.to_string()),
                    // 解密失败的保留原密文，用户重填 key 后即可修复
                    api_key_encrypted: if decrypt_failed {
                        encrypted.clone()
                    } else if plain.is_empty() {
                        String::new()
                    } else {
                        crypto.encrypt(&plain)
                    },
                    api_key_hint: key_hint(&plain),
                    api_base: api_base.clone(),
                    extra_headers: BTreeMap::new(),
                    extra_query: BTreeMap::new(),
                    status,
                });
                index.push((key, data.profiles.len() - 1));
                data.profiles.len() - 1
            }
        };
        let number = |key: &str| object.get(key).and_then(Value::as_f64);
        let params = ModelParams {
            temperature: number("temperature").unwrap_or(0.3),
            max_tokens: number("max_tokens").map(|v| v as i64).filter(|v| *v > 0).unwrap_or(4096),
            ..ModelParams::default()
        };
        let entry = ModelEntry {
            id: format!("mdl-m{idx}"),
            profile_id: data.profiles[profile_idx].id.clone(),
            model: model_string,
            display_name: non_empty(Some(&legacy_name)).unwrap_or_else(|| model_name.clone()),
            params,
            use_cases: default_use_cases(),
            status: VerifyStatus::default(),
        };
        let was_default = object.get("is_default").and_then(Value::as_bool).unwrap_or(false) || v1_default == Some(config_id.as_str());
        if was_default && data.default_model_id.is_none() {
            data.default_model_id = Some(entry.id.clone());
        }
        data.models.push(entry);
    }
    repair_bindings(&mut data);
    (data, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("att-llm-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn crypto(dir: &Path) -> Arc<Crypto> {
        Arc::new(Crypto::load_or_create(dir).unwrap())
    }

    #[test]
    fn 档案与模型_增删改与默认绑定() {
        let dir = temp_dir("crud");
        let store = LlmStore::load(&dir, crypto(&dir));
        let profile = store
            .create_profile(ProfileCreate {
                provider: "deepseek".into(),
                name: " DS ".into(),
                api_key: "sk-1234567890abcd".into(),
                api_base: None,
                extra_headers: BTreeMap::new(),
                extra_query: BTreeMap::new(),
            })
            .unwrap();
        assert_eq!(profile.name, "DS");
        assert_eq!(profile.api_key_hint, "sk-****abcd");
        assert!(profile_public(&profile).get("api_key_encrypted").is_none());
        assert_eq!(store.decrypt_key(&profile).unwrap(), "sk-1234567890abcd");

        let (created, skipped) = store
            .create_models(vec![
                ModelItemCreate { profile_id: profile.id.clone(), model: "deepseek-chat".into(), display_name: String::new(), params: None, set_default: false },
                ModelItemCreate { profile_id: profile.id.clone(), model: "deepseek-chat".into(), display_name: String::new(), params: None, set_default: false },
            ])
            .unwrap();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].model, "deepseek/deepseek-chat");
        assert_eq!(skipped.len(), 1, "同名模型应跳过");
        // 第一个模型自动成为默认
        assert_eq!(store.default_model_id().as_deref(), Some(created[0].id.as_str()));
        assert_eq!(store.delete_model(&created[0].id), Err(StoreError::ConflictDefault));
        assert_eq!(store.delete_profile(&profile.id), Err(StoreError::ConflictDefault));

        // 重新加载：文件格式可读回
        let reloaded = LlmStore::load(&dir, crypto(&dir));
        assert_eq!(reloaded.list_models(None).len(), 1);
        let target = reloaded.resolve_runtime(None).unwrap();
        assert_eq!(target.api_key, "sk-1234567890abcd");

        let updated = reloaded
            .update_profile(&profile.id, ProfileUpdate { api_key: Some(String::new()), name: Some("新名".into()), ..Default::default() })
            .unwrap();
        assert_eq!(updated.name, "新名");
        assert_eq!(reloaded.decrypt_key(&updated).unwrap(), "sk-1234567890abcd", "空串密钥 = 保持原值");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 自定义端点_必须填写_api_base() {
        let dir = temp_dir("custom");
        let store = LlmStore::load(&dir, crypto(&dir));
        let request = |base: Option<&str>| ProfileCreate {
            provider: "custom".into(),
            name: "中转".into(),
            api_key: String::new(),
            api_base: base.map(str::to_string),
            extra_headers: BTreeMap::new(),
            extra_query: BTreeMap::new(),
        };
        assert!(matches!(store.create_profile(request(Some("  "))), Err(StoreError::Invalid(_))));
        let profile = store.create_profile(request(Some("http://127.0.0.1:3000/v1"))).unwrap();
        let (created, _) = store
            .create_models(vec![ModelItemCreate { profile_id: profile.id, model: "group/x".into(), display_name: String::new(), params: None, set_default: true }])
            .unwrap();
        assert_eq!(created[0].model, "openai/group/x");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 损坏条目逐条跳过_绑定自动修复() {
        let dir = temp_dir("lenient");
        std::fs::write(
            dir.join(STORE_FILE_NAME),
            serde_json::to_string(&json!({
                "version": 2,
                "profiles": [
                    {"id": "p1", "provider": "custom", "name": "ok", "api_base": "http://h"},
                    {"id": "broken"}
                ],
                "models": [
                    {"id": "m1", "profile_id": "p1", "model": "group/x", "params": {"timeout_s": 9999, "reasoning_effort": "MAX"}},
                    {"id": "m2"}
                ],
                "default_model_id": "gone"
            }))
            .unwrap(),
        )
        .unwrap();
        let store = LlmStore::load(&dir, crypto(&dir));
        let overview = store.overview();
        assert_eq!(overview["skipped_entries"], json!(["profile:broken", "model:m2"]));
        assert_eq!(overview["default_model_id"], "m1");
        let model = store.get_model("m1").unwrap();
        assert_eq!(model.model, "openai/group/x", "custom 历史模型串自愈");
        assert_eq!(model.params.timeout_s, 600);
        assert_eq!(model.params.reasoning_effort, "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn v1_配置自动迁移并备份() {
        let dir = temp_dir("migrate");
        let crypto = crypto(&dir);
        let encrypted = crypto.encrypt("sk-legacy-key-0001");
        std::fs::write(
            dir.join(LEGACY_FILE_NAME),
            serde_json::to_string(&json!({
                "configs": [
                    {"id": "a", "provider": "deepseek", "name": "deepseek", "api_key_encrypted": encrypted,
                     "api_base": "https://api.deepseek.com/v1", "model": "deepseek-chat", "temperature": 0.7, "max_tokens": 2048},
                    {"id": "b", "provider": "deepseek", "name": "第二个", "api_key_encrypted": encrypted,
                     "api_base": "https://api.deepseek.com/v1", "model": "deepseek-reasoner"},
                    {"id": "c", "provider": "custom", "model": "x"},
                    {"id": "d", "provider": "openai", "api_key_encrypted": "broken-token", "model": "gpt-4o"}
                ],
                "default_config_id": "b"
            }))
            .unwrap(),
        )
        .unwrap();
        let store = LlmStore::load(&dir, crypto.clone());
        assert!(dir.join(STORE_FILE_NAME).exists());
        assert!(!dir.join(LEGACY_FILE_NAME).exists());
        assert!(dir.join("llm_configs.json.v1.bak").exists());

        let profiles = store.list_profiles();
        assert_eq!(profiles.len(), 2, "同 key 同端点的两条配置聚合为一个档案");
        assert_eq!(store.decrypt_key(&profiles[0]).unwrap(), "sk-legacy-key-0001");
        assert_eq!(profiles[1].status.state, "decrypt_failed");
        let models = store.list_models(None);
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].model, "deepseek/deepseek-chat");
        assert_eq!(models[0].params.max_tokens, 2048);
        // 显示名取各自条目的名字（Python 版这里会沿用上一条的名字）
        assert_eq!(models[1].display_name, "第二个");
        assert_eq!(store.default_model_id().as_deref(), Some(models[1].id.as_str()));
        assert_eq!(store.overview()["skipped_entries"].as_array().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
