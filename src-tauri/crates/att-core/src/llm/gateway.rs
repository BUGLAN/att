//! LLM 调用网关（Python 版基于 LiteLLM，这里直接按协议发 HTTP）。
//!
//! - 除 Anthropic（`/v1/messages`）外，注册表内所有服务商都走 OpenAI 兼容 `/chat/completions`；
//! - 每次请求显式注入超时（绝不依赖 SDK 默认值，旧版「无超时卡死」的教训）；
//! - 错误统一归类为 [`LlmError`]，文案先脱敏（上游会在错误体里回显密钥）；
//! - 走系统代理（与 Python httpx/LiteLLM 读代理的行为一致；豆包 ASR 是直连，不经这里）。

use super::errors::{
    classify_http, classify_transport, redact_secrets, LlmError, CODE_MODEL_NOT_FOUND, CODE_NETWORK, CODE_PROVIDER, CODE_TIMEOUT,
};
use super::registry::{self, Protocol, ProviderSpec};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// 验证类操作的固定超时（设置页按钮不应长时间转圈）。
pub const VERIFY_TIMEOUT: Duration = Duration::from_secs(15);
const ANTHROPIC_VERSION: &str = "2023-06-01";

static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .user_agent(concat!("att-desktop/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("HTTP 客户端构建失败")
});

/// 一次调用的目标：端点 + 凭据（密钥只活在内存里）。
#[derive(Clone)]
pub struct Endpoint {
    pub spec: &'static ProviderSpec,
    /// 已去掉末尾斜杠的 API Base
    pub base: String,
    pub api_key: String,
    pub extra_headers: BTreeMap<String, String>,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("provider", &self.spec.id)
            .field("base", &self.base)
            .field("api_key_set", &!self.api_key.is_empty())
            .finish()
    }
}

impl Endpoint {
    /// 用户填写的 API Base 优先，否则用注册表内置端点；custom 必须由用户提供。
    pub fn resolve(
        spec: &'static ProviderSpec,
        api_base: Option<&str>,
        api_key: &str,
        extra_headers: &BTreeMap<String, String>,
    ) -> Result<Self, LlmError> {
        let base = api_base.map(str::trim).filter(|b| !b.is_empty()).unwrap_or(spec.wire_base).trim_end_matches('/');
        if base.is_empty() {
            return Err(LlmError::plain(CODE_NETWORK, "未配置 API Base，请在档案里填写端点地址"));
        }
        Ok(Self { spec, base: base.to_string(), api_key: api_key.trim().to_string(), extra_headers: extra_headers.clone() })
    }

    /// OpenAI 兼容根路径（Ollama 的兼容接口挂在 /v1 下）。
    fn openai_root(&self) -> String {
        if self.spec.id == "ollama" && !self.base.ends_with("/v1") {
            format!("{}/v1", self.base)
        } else {
            self.base.clone()
        }
    }

    fn secrets(&self) -> Vec<&str> {
        if self.api_key.is_empty() { Vec::new() } else { vec![self.api_key.as_str()] }
    }

    fn request(&self, method: reqwest::Method, url: String, timeout: Duration) -> reqwest::RequestBuilder {
        let mut builder = CLIENT.request(method, url).timeout(timeout).header("Accept", "application/json");
        if !self.api_key.is_empty() {
            builder = match self.spec.protocol {
                Protocol::Anthropic => builder.header("x-api-key", &self.api_key),
                Protocol::OpenAiCompat => builder.bearer_auth(&self.api_key),
            };
        }
        if self.spec.protocol == Protocol::Anthropic {
            builder = builder.header("anthropic-version", ANTHROPIC_VERSION);
        }
        for (name, value) in &self.extra_headers {
            builder = builder.header(name, value);
        }
        builder
    }
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn new(role: &str, content: impl Into<String>) -> Self {
        Self { role: role.to_string(), content: content.into() }
    }
}

#[derive(Debug, Clone)]
pub struct ChatOptions {
    pub max_tokens: i64,
    /// None = 不传（部分推理模型只接受默认温度）
    pub temperature: Option<f64>,
    /// 整次请求的总预算（流式同样是总上限）
    pub timeout: Duration,
    /// 网络/上游 5xx 类可重试错误的重试次数
    pub retries: u32,
    /// 思考强度："" 不干预 / "none" 关闭 / "low"|"medium"|"high"
    pub reasoning_effort: String,
}

#[derive(Debug, Clone, Default)]
pub struct ChatResult {
    pub text: String,
    pub latency_ms: i64,
    /// stop / length（length = 输出达到 max_tokens 被截断，思考 token 也计入）
    pub finish_reason: String,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
}

impl ChatResult {
    pub fn truncated(&self) -> bool {
        self.finish_reason == "length"
    }
}

/// 流式阶段预算：开流到首条正文增量、相邻增量之间（总上限仍是 [`ChatOptions::timeout`]）。
#[derive(Debug, Clone, Copy)]
pub struct StreamBudget {
    pub first_token: Duration,
    pub chunk_idle: Duration,
}

/// 非流式补全。`model_string` 为存储里的完整模型串，发出前按服务商剥掉前缀。
pub async fn chat(endpoint: &Endpoint, model_string: &str, messages: &[ChatMessage], options: &ChatOptions) -> Result<ChatResult, LlmError> {
    let mut attempt = 0;
    loop {
        let started = Instant::now();
        let result = chat_once(endpoint, model_string, messages, options).await;
        match result {
            Ok(mut result) => {
                result.latency_ms = started.elapsed().as_millis() as i64;
                log_success(model_string, &result, options);
                return Ok(result);
            }
            Err(error) if attempt < options.retries && is_transient(&error) => {
                attempt += 1;
                tracing::warn!(model = model_string, error_code = error.code, attempt, "LLM 调用失败，重试");
                tokio::time::sleep(Duration::from_millis(500 * attempt as u64)).await;
            }
            Err(error) => {
                tracing::warn!(model = model_string, error_code = error.code, error = %error.message, "LLM 调用失败");
                return Err(error);
            }
        }
    }
}

fn is_transient(error: &LlmError) -> bool {
    matches!(error.code, CODE_NETWORK | CODE_TIMEOUT | CODE_PROVIDER)
}

fn log_success(model_string: &str, result: &ChatResult, options: &ChatOptions) {
    tracing::info!(
        model = model_string,
        latency_ms = result.latency_ms,
        prompt_tokens = result.prompt_tokens,
        completion_tokens = result.completion_tokens,
        total_tokens = result.total_tokens,
        finish_reason = %result.finish_reason,
        "LLM 调用成功"
    );
    if result.truncated() {
        tracing::warn!(model = model_string, max_tokens = options.max_tokens, "LLM 输出达到 max_tokens 上限被截断（思考 token 也计入）");
    }
}

async fn chat_once(endpoint: &Endpoint, model_string: &str, messages: &[ChatMessage], options: &ChatOptions) -> Result<ChatResult, LlmError> {
    let (url, body) = request_body(endpoint, model_string, messages, options, false);
    let response = open(endpoint, &url, body, options.timeout).await?;
    let text = response.text().await.map_err(|e| classify_transport(&e, &endpoint.secrets()))?;
    let value: Value = serde_json::from_str(&redact_secrets(&text, &endpoint.secrets()))
        .map_err(|_| LlmError::new(CODE_PROVIDER, "上游返回了无法解析的响应结构"))?;
    match endpoint.spec.protocol {
        Protocol::OpenAiCompat => {
            let choice = value.pointer("/choices/0").ok_or_else(|| LlmError::new(CODE_PROVIDER, "上游返回了无法解析的响应结构"))?;
            let usage = |key: &str| value.pointer(&format!("/usage/{key}")).and_then(Value::as_i64).unwrap_or(0);
            Ok(ChatResult {
                text: choice.pointer("/message/content").and_then(Value::as_str).unwrap_or("").to_string(),
                latency_ms: 0,
                finish_reason: choice.get("finish_reason").and_then(Value::as_str).unwrap_or("stop").to_string(),
                prompt_tokens: usage("prompt_tokens"),
                completion_tokens: usage("completion_tokens"),
                total_tokens: usage("total_tokens"),
            })
        }
        Protocol::Anthropic => {
            let text = value
                .get("content")
                .and_then(Value::as_array)
                .map(|blocks| blocks.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<String>())
                .unwrap_or_default();
            let usage = |key: &str| value.pointer(&format!("/usage/{key}")).and_then(Value::as_i64).unwrap_or(0);
            Ok(ChatResult {
                text,
                latency_ms: 0,
                finish_reason: anthropic_finish(value.get("stop_reason").and_then(Value::as_str)).to_string(),
                prompt_tokens: usage("input_tokens"),
                completion_tokens: usage("output_tokens"),
                total_tokens: usage("input_tokens") + usage("output_tokens"),
            })
        }
    }
}

fn anthropic_finish(stop_reason: Option<&str>) -> &'static str {
    if stop_reason == Some("max_tokens") { "length" } else { "stop" }
}

/// 组装请求 URL 与请求体（OpenAI 兼容 / Anthropic）。
fn request_body(endpoint: &Endpoint, model_string: &str, messages: &[ChatMessage], options: &ChatOptions, stream: bool) -> (String, Value) {
    let model = registry::wire_model_name(endpoint.spec, model_string);
    match endpoint.spec.protocol {
        Protocol::OpenAiCompat => {
            let mut body = json!({
                "model": model,
                "messages": messages.iter().map(|m| json!({ "role": m.role, "content": m.content })).collect::<Vec<_>>(),
                "max_tokens": options.max_tokens,
                "stream": stream,
            });
            if let Some(temperature) = options.temperature {
                body["temperature"] = json!(temperature);
            }
            apply_reasoning_effort(&mut body, endpoint.spec, &options.reasoning_effort);
            (format!("{}/chat/completions", endpoint.openai_root()), body)
        }
        Protocol::Anthropic => {
            let system: Vec<&str> = messages.iter().filter(|m| m.role == "system").map(|m| m.content.as_str()).collect();
            let mut body = json!({
                "model": model,
                "max_tokens": options.max_tokens,
                "stream": stream,
                "messages": messages
                    .iter()
                    .filter(|m| m.role != "system")
                    .map(|m| json!({ "role": m.role, "content": m.content }))
                    .collect::<Vec<_>>(),
            });
            if !system.is_empty() {
                body["system"] = json!(system.join("\n\n"));
            }
            if let Some(temperature) = options.temperature {
                body["temperature"] = json!(temperature);
            }
            let url = if endpoint.base.ends_with("/v1") { format!("{}/messages", endpoint.base) } else { format!("{}/v1/messages", endpoint.base) };
            (url, body)
        }
    }
}

/// 思考强度 → 上游参数（Python 版由 LiteLLM 的 reasoning_effort 映射，这里对齐其行为）：
/// DeepSeek 的「关闭」是 `thinking={type:disabled}`，其余 OpenAI 兼容上游直接传 `reasoning_effort`。
/// Anthropic 默认不开思考，「不干预/关闭」都无需传参，显式档位暂不映射。
fn apply_reasoning_effort(body: &mut Value, spec: &ProviderSpec, effort: &str) {
    match (spec.id, effort) {
        (_, "") => {}
        ("deepseek", "none") => body["thinking"] = json!({ "type": "disabled" }),
        (_, effort) => body["reasoning_effort"] = json!(effort),
    }
}

/// 上游明确表示不认某个可选参数（UnsupportedParams 一类文案）。
fn rejects_param(message: &str, param: &str) -> bool {
    let lowered = message.to_lowercase();
    lowered.contains(param)
        && ["unsupported", "unknown", "unrecognized", "not support", "does not support", "invalid", "not allowed", "extra"]
            .iter()
            .any(|marker| lowered.contains(marker))
}

/// 发起请求并确认成功状态；上游拒绝可选参数时调整后重试：
/// - `max_tokens` → `max_completion_tokens`（OpenAI 新一代推理模型）；
/// - 去掉 `reasoning_effort` / `thinking`（性能开关绝不能把原本可用的模型变成不可用）。
async fn open(endpoint: &Endpoint, url: &str, mut body: Value, timeout: Duration) -> Result<reqwest::Response, LlmError> {
    for _ in 0..3 {
        let response = endpoint
            .request(reqwest::Method::POST, url.to_string(), timeout)
            .json(&body)
            .send()
            .await
            .map_err(|e| classify_transport(&e, &endpoint.secrets()))?;
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        let error = classify_http(status, &redact_secrets(&text, &endpoint.secrets()));
        let object = body.as_object_mut().expect("请求体为对象");
        if error.message.contains("max_completion_tokens") && object.contains_key("max_tokens") {
            let max_tokens = object.remove("max_tokens").unwrap_or(Value::Null);
            object.insert("max_completion_tokens".into(), max_tokens);
            continue;
        }
        let reasoning = ["reasoning_effort", "thinking"].into_iter().find(|p| object.contains_key(*p) && rejects_param(&error.message, p));
        if let Some(param) = reasoning {
            tracing::info!("上游不支持 {param}，已去掉该参数重试");
            object.remove(param);
            continue;
        }
        return Err(error);
    }
    Err(LlmError::new(CODE_PROVIDER, "上游持续拒绝请求参数"))
}

/// 流式补全：每条正文增量回调 `on_delta`；返回汇总结果。
///
/// 看门狗：开流后 `first_token` 内必须收到首条正文增量，之后相邻增量间隔不超过 `chunk_idle`；
/// 总时长受 `options.timeout` 约束。思考增量（reasoning_content / thinking）不计入正文。
pub async fn chat_stream(
    endpoint: &Endpoint,
    model_string: &str,
    messages: &[ChatMessage],
    options: &ChatOptions,
    budget: StreamBudget,
    mut on_delta: impl FnMut(&str),
) -> Result<ChatResult, LlmError> {
    let started = Instant::now();
    let (url, body) = request_body(endpoint, model_string, messages, options, true);
    let mut response = open(endpoint, &url, body, options.timeout).await?;
    let mut result = ChatResult::default();
    let mut buffer: Vec<u8> = Vec::new();
    let mut deadline = Instant::now() + budget.first_token;
    let mut got_text = false;
    let mut done = false;

    while !done {
        let now = Instant::now();
        let chunk = match tokio::time::timeout(deadline.saturating_duration_since(now), response.chunk()).await {
            Err(_) if got_text => {
                return Err(LlmError::new(CODE_TIMEOUT, format!("流式输出中断（超过 {} 秒没有新内容）", budget.chunk_idle.as_secs())))
            }
            Err(_) => return Err(LlmError::new(CODE_TIMEOUT, format!("等待首个输出超时（{} 秒）", budget.first_token.as_secs()))),
            Ok(Err(error)) => return Err(classify_transport(&error, &endpoint.secrets())),
            Ok(Ok(None)) => break,
            Ok(Ok(Some(bytes))) => bytes,
        };
        buffer.extend_from_slice(&chunk);
        while let Some(newline) = buffer.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buffer.drain(..=newline).collect();
            let line = String::from_utf8_lossy(&line);
            let Some(payload) = line.trim().strip_prefix("data:").map(str::trim) else { continue };
            if payload == "[DONE]" {
                done = true;
                break;
            }
            let Ok(event) = serde_json::from_str::<Value>(payload) else { continue };
            if let Some(message) = event.get("error").filter(|e| !e.is_null()) {
                let text = message.get("message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| message.to_string());
                return Err(LlmError::new(CODE_PROVIDER, redact_secrets(&text, &endpoint.secrets())));
            }
            let delta = parse_stream_event(endpoint.spec.protocol, &event, &mut result);
            if let Some(text) = delta.filter(|t| !t.is_empty()) {
                result.text.push_str(&text);
                on_delta(&text);
                got_text = true;
            }
            if endpoint.spec.protocol == Protocol::Anthropic && event.get("type").and_then(Value::as_str) == Some("message_stop") {
                done = true;
                break;
            }
        }
        if got_text {
            deadline = Instant::now() + budget.chunk_idle;
        }
    }
    if result.finish_reason.is_empty() {
        result.finish_reason = "stop".into();
    }
    if result.total_tokens == 0 {
        result.total_tokens = result.prompt_tokens + result.completion_tokens;
    }
    result.latency_ms = started.elapsed().as_millis() as i64;
    log_success(model_string, &result, options);
    Ok(result)
}

/// 解析一条 SSE 事件：返回正文增量，并把 finish_reason / usage 记进结果。
fn parse_stream_event(protocol: Protocol, event: &Value, result: &mut ChatResult) -> Option<String> {
    let number = |pointer: &str| event.pointer(pointer).and_then(Value::as_i64);
    match protocol {
        Protocol::OpenAiCompat => {
            if let Some(reason) = event.pointer("/choices/0/finish_reason").and_then(Value::as_str) {
                result.finish_reason = reason.to_string();
            }
            if let Some(tokens) = number("/usage/prompt_tokens") {
                result.prompt_tokens = tokens;
            }
            if let Some(tokens) = number("/usage/completion_tokens") {
                result.completion_tokens = tokens;
            }
            if let Some(tokens) = number("/usage/total_tokens") {
                result.total_tokens = tokens;
            }
            event.pointer("/choices/0/delta/content").and_then(Value::as_str).map(str::to_string)
        }
        Protocol::Anthropic => match event.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                result.prompt_tokens = number("/message/usage/input_tokens").unwrap_or(0);
                None
            }
            Some("content_block_delta") if event.pointer("/delta/type").and_then(Value::as_str) == Some("text_delta") => {
                event.pointer("/delta/text").and_then(Value::as_str).map(str::to_string)
            }
            Some("message_delta") => {
                result.finish_reason = anthropic_finish(event.pointer("/delta/stop_reason").and_then(Value::as_str)).to_string();
                result.completion_tokens = number("/usage/output_tokens").unwrap_or(result.completion_tokens);
                None
            }
            _ => None,
        },
    }
}

// ==================== 验证类 ====================

/// 验证端点的统一结果（`{ok, error_code, message, latency_ms, remote_models}`）。
#[derive(Debug, Clone, Default)]
pub struct VerifyOutcome {
    pub ok: bool,
    pub error_code: Option<String>,
    pub message: Option<String>,
    pub latency_ms: Option<i64>,
    pub remote_models: Vec<String>,
}

impl VerifyOutcome {
    pub fn failed(error: &LlmError) -> Self {
        Self { ok: false, error_code: Some(error.code.to_string()), message: Some(error.user_message.clone()), ..Default::default() }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "ok": self.ok,
            "error_code": if self.ok { None } else { Some(self.error_code.clone().unwrap_or_else(|| "unknown".into())) },
            "message": self.message,
            "latency_ms": self.latency_ms,
            "remote_models": self.remote_models,
        })
    }
}

/// 模型级轻量验证：约 5 token 的补全，验证「凭据 + 模型名」都可用。
///
/// 不传温度、不重试，并强制关闭思考：5 token 的预算必然被思考吃光，既拖慢「测试」按钮
/// 又会刷假的截断告警；上游不认关闭参数时 [`open`] 会自动去掉重试。这里只关心连通性与鉴权。
pub async fn verify_model(endpoint: &Endpoint, model_string: &str) -> VerifyOutcome {
    let options = ChatOptions { max_tokens: 5, temperature: None, timeout: VERIFY_TIMEOUT, retries: 0, reasoning_effort: "none".into() };
    match chat(endpoint, model_string, &[ChatMessage::new("user", "Hi")], &options).await {
        Ok(result) => VerifyOutcome { ok: true, latency_ms: Some(result.latency_ms), ..Default::default() },
        Err(error) => VerifyOutcome::failed(&error),
    }
}

/// 档案级验证：鉴权 + 尽力列取上游模型列表（不支持列取时视为通过）。
pub async fn verify_profile(endpoint: &Endpoint) -> VerifyOutcome {
    let spec = endpoint.spec;
    let probe = format!("{}/{}", spec.prefix, spec.suggested_models.first().copied().unwrap_or("gpt-3.5-turbo"));
    let mut outcome = verify_model(endpoint, &probe).await;
    if !outcome.ok {
        // 探测用的是建议模型名：模型不存在不代表凭据有问题
        if outcome.error_code.as_deref() == Some(CODE_MODEL_NOT_FOUND) {
            outcome = VerifyOutcome { ok: true, ..Default::default() };
        } else {
            return outcome;
        }
    }
    if spec.supports_model_listing {
        match list_remote_models(endpoint).await {
            Ok(models) => outcome.remote_models = models,
            // 列取失败不推翻凭据验证结论，只提示手输
            Err(reason) => outcome.message = Some(format!("模型列表拉取失败（{reason}），可手动输入模型名")),
        }
    }
    outcome
}

async fn read_json(endpoint: &Endpoint, response: reqwest::Response) -> Result<Value, LlmError> {
    let status = response.status();
    let text = response.text().await.map_err(|e| classify_transport(&e, &endpoint.secrets()))?;
    let text = redact_secrets(&text, &endpoint.secrets());
    if !status.is_success() {
        return Err(classify_http(status.as_u16(), &text));
    }
    serde_json::from_str(&text).map_err(|_| LlmError::new(CODE_PROVIDER, "上游返回了无法解析的响应结构"))
}

/// 拉取 OpenAI 兼容 GET /models；失败返回已脱敏原因。
pub async fn list_remote_models(endpoint: &Endpoint) -> Result<Vec<String>, String> {
    let url = format!("{}/models", endpoint.openai_root());
    let response = endpoint
        .request(reqwest::Method::GET, url, VERIFY_TIMEOUT)
        .send()
        .await
        .map_err(|e| classify_transport(&e, &endpoint.secrets()).message)?;
    let value = read_json(endpoint, response).await.map_err(|e| e.message)?;
    let items = value.get("data").unwrap_or(&value);
    let items = items.as_array().ok_or_else(|| "模型列表响应格式无法解析".to_string())?;
    let mut ids: Vec<String> = items
        .iter()
        .filter_map(|item| item.get("id").and_then(Value::as_str))
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::testing::stub_server;

    fn endpoint(provider: &str, base: &str, key: &str) -> Endpoint {
        Endpoint::resolve(registry::get(provider).unwrap(), Some(base), key, &BTreeMap::new()).unwrap()
    }

    #[tokio::test]
    async fn 档案验证_鉴权通过并列出模型() {
        let (base, mut requests) = stub_server(vec![
            ("POST /v1/chat/completions", 200, r#"{"choices":[{"message":{"content":"Hi"},"finish_reason":"length"}]}"#.into()),
            ("GET /v1/models", 200, r#"{"data":[{"id":"b"},{"id":"a"},{"id":"a"}]}"#.into()),
        ])
        .await;
        let outcome = verify_profile(&endpoint("custom", &format!("{base}/v1/"), "sk-test-123456")).await;
        assert!(outcome.ok, "{outcome:?}");
        assert_eq!(outcome.remote_models, vec!["a", "b"]);
        let chat_request = requests.recv().await.unwrap();
        assert!(chat_request.to_lowercase().contains("authorization: bearer sk-test-123456"));
        assert!(chat_request.contains(r#""model":"gpt-3.5-turbo""#), "custom 探测模型剥掉 openai/ 前缀: {chat_request}");
    }

    #[tokio::test]
    async fn 鉴权失败_归类且不回显密钥() {
        let (base, _requests) = stub_server(vec![(
            "POST /chat/completions",
            401,
            r#"{"error":{"message":"Authentication Fails, Your api key: sk-leak-999999 is invalid"}}"#.into(),
        )])
        .await;
        let outcome = verify_model(&endpoint("deepseek", &base, "sk-leak-999999"), "deepseek/deepseek-chat").await;
        assert!(!outcome.ok);
        assert_eq!(outcome.error_code.as_deref(), Some("auth_failed"));
        assert!(!outcome.message.unwrap().contains("sk-leak-999999"));
    }

    #[tokio::test]
    async fn 探测模型不存在_不推翻凭据结论() {
        let (base, _requests) = stub_server(vec![
            ("POST /chat/completions", 400, r#"{"error":{"message":"Model Not Exist"}}"#.into()),
            ("GET /models", 200, r#"{"data":[{"id":"deepseek-chat"}]}"#.into()),
        ])
        .await;
        let outcome = verify_profile(&endpoint("deepseek", &base, "k")).await;
        assert!(outcome.ok);
        assert_eq!(outcome.remote_models, vec!["deepseek-chat"]);
    }

    #[tokio::test]
    async fn anthropic_协议与_ollama_兼容路径() {
        let (base, mut requests) = stub_server(vec![
            ("POST /v1/messages", 200, r#"{"content":[{"type":"text","text":"Hi"}],"stop_reason":"end_turn"}"#.into()),
            ("POST /v1/chat/completions", 200, r#"{"choices":[{"message":{"content":"ok"}}]}"#.into()),
        ])
        .await;
        let outcome = verify_model(&endpoint("anthropic", &base, "ak-123456"), "anthropic/claude-x").await;
        assert!(outcome.ok, "{outcome:?}");
        let request = requests.recv().await.unwrap().to_lowercase();
        assert!(request.contains("x-api-key: ak-123456") && request.contains("anthropic-version"));

        let outcome = verify_model(&endpoint("ollama", &base, ""), "ollama_chat/qwen3").await;
        assert!(outcome.ok, "Ollama 走 /v1/chat/completions: {outcome:?}");
    }

    #[tokio::test]
    async fn 连接失败_归类为网络错误() {
        let outcome = verify_model(&endpoint("custom", "http://127.0.0.1:1", ""), "openai/x").await;
        assert_eq!(outcome.error_code.as_deref(), Some("network_error"));
    }
}
