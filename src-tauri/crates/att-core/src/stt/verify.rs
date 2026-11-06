//! 在线引擎凭据验证：区分「填了 Token」和「Token 真的能用」。
//!
//! - 豆包：只做 WebSocket 握手（401 / 403 / 404 在握手阶段就能判定），成功后立刻断开，
//!   不推音频、不产生计费时长；
//! - ChatGPT 听写：上传 0.5 秒静音（空文本是合法结果），按 HTTP 状态判定。
//!
//! 结果按「凭据指纹」缓存在内存（[`CredentialChecks`]，挂在 `CoreState` 上）：Token、Resource ID、
//! 接入点任一变化，旧结果自动失效。指纹只是进程内哈希，不落盘、不外发。
//!
//! 只有「确定不可用」（[`CheckStatus::Invalid`]：鉴权失败、接入点不存在）才会让首页显示未就绪；
//! 网络、限流、服务端异常记为 [`CheckStatus::Unknown`]，不据此拦录音。

use super::sauc::ErrorKind;
use super::{chatgpt, chatgpt_config, doubao, doubao_config};
use crate::state::CoreState;
use serde::Serialize;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Mutex;
use std::time::Duration;

/// 豆包 provider 标识。
pub const DOUBAO: &str = "doubao_asr";
/// ChatGPT 听写 provider 标识。
pub const CHATGPT: &str = "chatgpt_asr";

/// ChatGPT 验证请求的超时：比正常听写短，验证不该让设置页等太久。
const CHATGPT_CHECK_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    /// 握手 / 请求成功
    Ok,
    /// 确定不可用：Token 无效或过期、Resource ID 与开通的产品不符、接入点不存在（需要用户改设置）
    Invalid,
    /// 没验成（网络、限流、服务端异常、风控）：不据此判定不可用
    Unknown,
}

/// 一次验证的结果（序列化给前端；指纹不外发）。
#[derive(Debug, Clone, Serialize)]
pub struct CredentialCheck {
    pub provider: String,
    pub status: CheckStatus,
    /// 结论文案：成功时为空串，失败时是可执行的中文提示（不含凭据）
    pub message: String,
    pub checked_at: String,
    #[serde(skip)]
    fingerprint: u64,
}

/// 各在线引擎最近一次验证结果（按 provider 存一份，指纹不符即视为没有）。
#[derive(Default)]
pub struct CredentialChecks {
    inner: Mutex<HashMap<String, CredentialCheck>>,
}

impl CredentialChecks {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, CredentialCheck>> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn get(&self, provider: &str, fingerprint: u64) -> Option<CredentialCheck> {
        self.lock().get(provider).filter(|check| check.fingerprint == fingerprint).cloned()
    }

    fn store(&self, check: CredentialCheck) {
        self.lock().insert(check.provider.clone(), check);
    }
}

/// 是否是需要验证凭据的在线引擎。
pub fn is_online_provider(provider: &str) -> bool {
    matches!(provider.trim(), DOUBAO | CHATGPT)
}

fn fingerprint(parts: &[&str]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for part in parts {
        part.trim().hash(&mut hasher);
    }
    hasher.finish()
}

/// 当前设置下该 provider 的凭据指纹；没配齐（缺 Token 等）时为 `None`。
fn current_fingerprint(core: &CoreState, provider: &str) -> Option<u64> {
    match provider.trim() {
        DOUBAO => {
            let config = doubao_config(core);
            config.ensure().ok()?;
            Some(fingerprint(&[DOUBAO, &config.api_key, &config.resource_id, &config.endpoint]))
        }
        CHATGPT => {
            let config = chatgpt_config(core);
            config.ensure().ok()?;
            Some(fingerprint(&[CHATGPT, &config.api_key, &config.endpoint]))
        }
        _ => None,
    }
}

/// 该 provider 当前凭据的最近一次验证结果（没验过、凭据已变或没配齐时为 `None`）。
pub fn current_check(core: &CoreState, provider: &str) -> Option<CredentialCheck> {
    let fingerprint = current_fingerprint(core, provider)?;
    core.credential_checks.get(provider.trim(), fingerprint)
}

/// 当前凭据确定不可用时给出原因（供 `/api/health` 判定就绪）；没验过或只是没验成返回 `None`。
pub fn invalid_reason(core: &CoreState, provider: &str) -> Option<String> {
    current_check(core, provider).filter(|check| check.status == CheckStatus::Invalid).map(|check| check.message)
}

/// 立即验证该 provider 的当前凭据并记下结果。没配齐时返回 `Err`（提示去填写）。
pub async fn check(core: &CoreState, provider: &str) -> Result<CredentialCheck, String> {
    let provider = provider.trim();
    let (status, message, fingerprint) = match provider {
        DOUBAO => {
            let config = doubao_config(core);
            config.ensure().map_err(|error| error.to_string())?;
            let fingerprint = fingerprint(&[DOUBAO, &config.api_key, &config.resource_id, &config.endpoint]);
            let (status, message) = check_doubao(&config).await;
            (status, message, fingerprint)
        }
        CHATGPT => {
            let mut config = chatgpt_config(core);
            config.ensure()?;
            let fingerprint = fingerprint(&[CHATGPT, &config.api_key, &config.endpoint]);
            config.timeout = config.timeout.min(CHATGPT_CHECK_TIMEOUT);
            let (status, message) = check_chatgpt(&config).await;
            (status, message, fingerprint)
        }
        other => return Err(format!("{other} 不是在线引擎，无需验证凭据")),
    };
    tracing::info!(provider, ?status, "在线引擎凭据验证完成");
    let check = CredentialCheck {
        provider: provider.to_string(),
        status,
        message,
        checked_at: crate::now_rfc3339(),
        fingerprint,
    };
    core.credential_checks.store(check.clone());
    Ok(check)
}

/// 测试支撑：直接记下一条验证结论（按当前凭据指纹），供下游 crate 的测试模拟「验证失败」。
#[cfg(any(test, feature = "test-support"))]
pub fn record_for_test(core: &CoreState, provider: &str, status: CheckStatus, message: &str) {
    let fingerprint = current_fingerprint(core, provider).expect("测试前先配好凭据");
    core.credential_checks.store(CredentialCheck {
        provider: provider.trim().to_string(),
        status,
        message: message.to_string(),
        checked_at: crate::now_rfc3339(),
        fingerprint,
    });
}

/// 豆包：握手成功即视为可用，随即断开（不推音频，不计费）。
async fn check_doubao(config: &doubao::DoubaoConfig) -> (CheckStatus, String) {
    match doubao::DoubaoSession::connect(config, "zh", "", None).await {
        Ok(session) => {
            session.cancel().await;
            (CheckStatus::Ok, String::new())
        }
        Err(error) => (doubao_status(error.info.kind), error.to_string()),
    }
}

/// 豆包错误分类 → 验证结论：鉴权与接入点问题是用户要改的设置，其余只算「没验成」。
pub fn doubao_status(kind: ErrorKind) -> CheckStatus {
    match kind {
        ErrorKind::Auth | ErrorKind::Protocol => CheckStatus::Invalid,
        _ => CheckStatus::Unknown,
    }
}

/// ChatGPT：上传 0.5 秒静音。
async fn check_chatgpt(config: &chatgpt::ChatGptConfig) -> (CheckStatus, String) {
    let wav = match crate::audio::wav_bytes_16k(&[0i16; 8000]) {
        Ok(wav) => wav,
        Err(error) => return (CheckStatus::Unknown, format!("验证音频生成失败：{error}")),
    };
    match chatgpt::transcribe_wav_detailed(config, wav, "zh").await {
        Ok(_) => (CheckStatus::Ok, String::new()),
        Err(error) => (chatgpt_status(error.status, error.html), error.message),
    }
}

/// ChatGPT HTTP 状态 → 验证结论：401、非风控的 403、404 是令牌或端点问题；风控 HTML、限流、5xx、网络只算没验成。
pub fn chatgpt_status(status: Option<u16>, html: bool) -> CheckStatus {
    match status {
        Some(401) | Some(404) => CheckStatus::Invalid,
        Some(403) if !html => CheckStatus::Invalid,
        _ => CheckStatus::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_core(tag: &str) -> CoreState {
        let dir = std::env::temp_dir().join(format!("att-verify-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        CoreState::init(&dir, crate::Platform::current()).unwrap()
    }

    #[test]
    fn 豆包错误分类_鉴权与接入点算确定不可用() {
        assert_eq!(doubao_status(ErrorKind::Auth), CheckStatus::Invalid);
        assert_eq!(doubao_status(ErrorKind::Protocol), CheckStatus::Invalid);
        assert_eq!(doubao_status(ErrorKind::Network), CheckStatus::Unknown);
        assert_eq!(doubao_status(ErrorKind::Quota), CheckStatus::Unknown);
        assert_eq!(doubao_status(ErrorKind::Server), CheckStatus::Unknown);
    }

    #[test]
    fn chatgpt_状态分类_风控与网络只算没验成() {
        assert_eq!(chatgpt_status(Some(401), false), CheckStatus::Invalid);
        assert_eq!(chatgpt_status(Some(403), false), CheckStatus::Invalid);
        assert_eq!(chatgpt_status(Some(403), true), CheckStatus::Unknown);
        assert_eq!(chatgpt_status(Some(404), false), CheckStatus::Invalid);
        assert_eq!(chatgpt_status(Some(429), false), CheckStatus::Unknown);
        assert_eq!(chatgpt_status(Some(502), false), CheckStatus::Unknown);
        assert_eq!(chatgpt_status(None, false), CheckStatus::Unknown);
    }

    #[test]
    fn 结果按指纹失效_换_token_后旧结论不再生效() {
        let core = temp_core("fingerprint");
        // 没配 Token：没有指纹，也就没有结论
        assert!(current_check(&core, DOUBAO).is_none());

        let body = serde_json::json!({ "doubao_asr_api_key": "token-a" });
        core.apply_settings_update(&body).unwrap();
        let fp_a = current_fingerprint(&core, DOUBAO).unwrap();
        core.credential_checks.store(CredentialCheck {
            provider: DOUBAO.into(),
            status: CheckStatus::Invalid,
            message: "鉴权失败".into(),
            checked_at: crate::now_rfc3339(),
            fingerprint: fp_a,
        });
        assert_eq!(invalid_reason(&core, DOUBAO).as_deref(), Some("鉴权失败"));

        // 换 Token / 换 Resource ID：旧结论都失效
        core.apply_settings_update(&serde_json::json!({ "doubao_asr_api_key": "token-b" })).unwrap();
        assert!(current_check(&core, DOUBAO).is_none());
        assert_ne!(current_fingerprint(&core, DOUBAO).unwrap(), fp_a);
        core.apply_settings_update(&serde_json::json!({ "doubao_asr_api_key": "token-a", "doubao_asr_resource_id": "volc.bigasr.sauc.duration" }))
            .unwrap();
        assert!(invalid_reason(&core, DOUBAO).is_none());
    }

    #[test]
    fn 验证结果序列化不带指纹() {
        let check = CredentialCheck {
            provider: DOUBAO.into(),
            status: CheckStatus::Ok,
            message: String::new(),
            checked_at: "2026-11-06T00:00:00Z".into(),
            fingerprint: 42,
        };
        let value = serde_json::to_value(&check).unwrap();
        assert_eq!(value["status"], "ok");
        assert!(value.get("fingerprint").is_none());
    }

    #[tokio::test]
    async fn 本地引擎与未配置凭据不做验证() {
        let core = temp_core("unconfigured");
        assert!(check(&core, "whisper").await.is_err());
        let error = check(&core, DOUBAO).await.unwrap_err();
        assert!(error.contains("Access Token"), "{error}");
        assert!(!is_online_provider("qwen_asr"));
        assert!(is_online_provider(" doubao_asr "));
    }
}
