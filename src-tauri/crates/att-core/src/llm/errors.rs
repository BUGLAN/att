//! LLM 错误分级与脱敏（移植自 Python `app/llm/errors.py` / `security.py`）。
//!
//! 上游鉴权失败时会把完整密钥回显在错误体里（DeepSeek/OpenAI 实测如此），
//! 因此所有对外文案都必须先过 [`redact_secrets`]，绝不能让密钥流入日志/前端。

use regex::Regex;
use std::sync::LazyLock;

pub const CODE_AUTH: &str = "auth_failed";
pub const CODE_MODEL_NOT_FOUND: &str = "model_not_found";
pub const CODE_QUOTA: &str = "quota_exceeded";
pub const CODE_TIMEOUT: &str = "timeout";
pub const CODE_NETWORK: &str = "network_error";
pub const CODE_PROVIDER: &str = "provider_error";
pub const CODE_UNSUPPORTED_PROVIDER: &str = "unsupported_provider";
/// 输出预算被思考 token 吃光（正文为空 + finish_reason=length）：重试必然同样失败
pub const CODE_OUTPUT_TRUNCATED: &str = "output_truncated";
pub const CODE_UNKNOWN: &str = "unknown";

/// 面向用户的建议文案（失败原因要可执行，而不是一坨异常字符串）。
fn user_hint(code: &str) -> &'static str {
    match code {
        CODE_AUTH => "检查 API Key 是否复制完整、是否被撤销或过期、账户是否欠费",
        CODE_MODEL_NOT_FOUND => "检查模型名是否正确（含服务商前缀），或从模型列表重新拉取",
        CODE_QUOTA => "账户欠费或触发限流，请充值或稍后重试",
        CODE_TIMEOUT => "端点不可达或响应过慢，请检查网络/代理，或在模型参数里调大超时",
        CODE_NETWORK => "网络连接失败，请检查网络与代理设置",
        CODE_PROVIDER => "上游服务故障，请稍后重试",
        CODE_UNSUPPORTED_PROVIDER => "可在档案编辑里改用「自定义 OpenAI 兼容」端点",
        CODE_OUTPUT_TRUNCATED => "模型的思考 token 占满了 Max Tokens，没有余量输出正文。请在模型参数里把「思考强度」设为「关闭」，或调大 Max Tokens",
        _ => "调用失败，请查看日志获取详情",
    }
}

/// 带分级的 LLM 调用错误；`message` 保证已脱敏。
#[derive(Debug, Clone)]
pub struct LlmError {
    pub code: &'static str,
    pub message: String,
    /// 附带建议后的完整文案（对外展示用）
    pub user_message: String,
}

impl LlmError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        let message = message.into();
        let user_message = if message.is_empty() {
            user_hint(code).to_string()
        } else {
            format!("{message}。建议：{}", user_hint(code))
        };
        Self { code, message, user_message }
    }

    /// false = 换一种传输方式重试也必然失败（不得回退重跑，否则耗时翻倍）。
    pub fn retryable(&self) -> bool {
        self.code != CODE_OUTPUT_TRUNCATED
    }

    /// 不附加建议（文案本身已可执行）。
    pub fn plain(code: &'static str, message: impl Into<String>) -> Self {
        let message = message.into();
        Self { code, user_message: message.clone(), message }
    }
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.user_message)
    }
}

impl std::error::Error for LlmError {}

const MASK: &str = "***";

static KEY_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    // api key: xxx / api_key=xxx / access_token: xxx（强制 : 或 = 分隔，避免误伤普通短语）
    Regex::new(r#"(?i)\b(api[\s_-]?key|access[\s_-]?token|auth[\s_-]?token)\b(\s*[:=]\s*)(['"]?)([^\s'"]{3,})"#)
        .expect("正则合法")
});
static BEARER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(bearer)\s+([A-Za-z0-9_\-\.\+/]{6,})").expect("正则合法"));
static BARE_KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bsk-[A-Za-z0-9_\-]{4,}").expect("正则合法"));

/// 抹掉文本里的密钥：已知字面量精确替换优先，再按常见模式兜底。
pub fn redact_secrets(text: &str, secrets: &[&str]) -> String {
    let mut result = text.to_string();
    for secret in secrets {
        if secret.len() >= 3 {
            result = result.replace(secret, MASK);
        }
    }
    let result = KEY_VALUE.replace_all(&result, |caps: &regex::Captures| format!("{}{}{}{MASK}", &caps[1], &caps[2], &caps[3]));
    let result = BEARER.replace_all(&result, |caps: &regex::Captures| format!("{} {MASK}", &caps[1]));
    BARE_KEY.replace_all(&result, MASK).into_owned()
}

/// 按 HTTP 状态码 + 错误体文本归类（错误体已脱敏）。
pub fn classify_http(status: u16, body: &str) -> LlmError {
    let lowered = body.to_lowercase();
    let model_missing = lowered.contains("model")
        && ["not exist", "not found", "does not exist", "no such model", "invalid model", "unknown model"]
            .iter()
            .any(|marker| lowered.contains(marker));
    let code = match status {
        401 | 403 => CODE_AUTH,
        402 => CODE_QUOTA,
        404 => CODE_MODEL_NOT_FOUND,
        429 => CODE_QUOTA,
        400 | 422 if model_missing => CODE_MODEL_NOT_FOUND,
        400..=499 if lowered.contains("insufficient") || lowered.contains("balance") => CODE_QUOTA,
        _ if model_missing => CODE_MODEL_NOT_FOUND,
        _ => CODE_PROVIDER,
    };
    let summary = summarize_body(body);
    let message = if summary.is_empty() { format!("上游返回 HTTP {status}") } else { format!("上游返回 HTTP {status}：{summary}") };
    LlmError::new(code, message)
}

/// 传输层错误（连接失败 / 超时）。
pub fn classify_transport(error: &reqwest::Error, secrets: &[&str]) -> LlmError {
    let text = redact_secrets(&error_chain(error), secrets);
    if error.is_timeout() {
        return LlmError::new(CODE_TIMEOUT, format!("请求超时（{text}）"));
    }
    LlmError::new(CODE_NETWORK, format!("无法连接上游服务（{text}）"))
}

/// reqwest 的顶层 Display 很笼统（error sending request），带上 source 链才看得出 DNS/TLS/拒绝连接。
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut parts = vec![error.to_string()];
    let mut source = error.source();
    while let Some(inner) = source {
        let text = inner.to_string();
        if !parts.contains(&text) {
            parts.push(text);
        }
        source = inner.source();
    }
    parts.join("：")
}

/// 从 JSON 错误体里取出可读信息（error.message / message / detail），否则截断原文。
fn summarize_body(body: &str) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        let candidates = [
            value.pointer("/error/message"),
            value.pointer("/message"),
            value.pointer("/detail"),
            value.pointer("/error"),
        ];
        for candidate in candidates.into_iter().flatten() {
            if let Some(text) = candidate.as_str().filter(|t| !t.trim().is_empty()) {
                return truncate(text.trim());
            }
        }
    }
    truncate(body.trim())
}

fn truncate(text: &str) -> String {
    const LIMIT: usize = 300;
    if text.chars().count() <= LIMIT {
        return text.to_string();
    }
    format!("{}…", text.chars().take(LIMIT).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 脱敏_字面量与常见模式() {
        let text = "Authentication Fails, Your api key: sk-abcdef123456 is invalid; Bearer abcdefghijk; raw sk-zzzz9999";
        let safe = redact_secrets(text, &[]);
        assert!(!safe.contains("abcdef123456") && !safe.contains("abcdefghijk") && !safe.contains("zzzz9999"), "{safe}");
        assert_eq!(redact_secrets("token XYZ-SECRET here", &["XYZ-SECRET"]), "token *** here");
        // 普通短语不误伤
        assert_eq!(redact_secrets("authorization failed", &[]), "authorization failed");
    }

    #[test]
    fn 状态码与错误体归类() {
        assert_eq!(classify_http(401, "{}").code, CODE_AUTH);
        assert_eq!(classify_http(402, r#"{"error":{"message":"Insufficient Balance"}}"#).code, CODE_QUOTA);
        let missing = classify_http(400, r#"{"error":{"message":"Model Not Exist"}}"#);
        assert_eq!(missing.code, CODE_MODEL_NOT_FOUND);
        assert!(missing.message.contains("Model Not Exist"));
        assert_eq!(classify_http(503, "upstream down").code, CODE_PROVIDER);
    }
}
