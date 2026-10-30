//! 记录优化编排（移植自 Python `app/llm/optimizer.py` + `app/services/optimization_stream_service.py`）。
//!
//! 一次优化：记录置 optimizing → 解析预设模板与默认模型 → 流式补全（增量回调）→ 清洗 → 落盘。
//! 约束（每条都是 Python 版踩过的坑）：
//! - 任何失败/取消都必须把记录从 optimizing 复位为 transcribed，否则会永久卡状态；
//! - 流式在首条增量前失败时回退一次非流式；已产出增量后不回退（避免重复计费/重复追加）；
//! - 确定性失败（思考 token 吃满预算 → 正文为空）不回退；
//! - 同一记录同时只允许一条优化，后来的请求取代旧请求；
//! - 失败文案全部已脱敏（网关层保证）。

use super::cleaning::clean_output;
use super::errors::{LlmError, CODE_OUTPUT_TRUNCATED, CODE_PROVIDER, CODE_UNSUPPORTED_PROVIDER};
use super::gateway::{self, ChatOptions, Endpoint, StreamBudget};
use super::{prompt, registry, timeouts};
use crate::records::RecordStatus;
use crate::state::CoreState;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;
use tokio::sync::watch;

/// 优化过程中的增量事件。
#[derive(Debug, Clone)]
pub enum OptimizeEvent {
    /// 已开始（记录已置 optimizing）
    Started,
    /// 正文增量（回退非流式时为整段结果）
    Delta(String),
}

#[derive(Debug, Clone)]
pub struct OptimizeSuccess {
    pub optimized_text: String,
    pub processing_time: f64,
    pub tokens_used: i64,
    pub truncated: bool,
    pub model_string: String,
    /// true = 流式在首条增量前失败，本次结果来自回退的非流式调用
    pub degraded: bool,
}

#[derive(Debug, Clone)]
pub enum OptimizeOutcome {
    Completed(OptimizeSuccess),
    /// 已脱敏的失败原因（记录已复位为 transcribed 并写入 error）
    Failed(String),
    /// 用户取消（记录已复位为 transcribed）
    Cancelled,
}

/// 活动优化登记：record_id → (任务序号, 取消信号)。
#[derive(Default)]
pub struct OptimizationHub {
    active: Mutex<HashMap<String, (u64, watch::Sender<bool>)>>,
    next_id: AtomicU64,
}

impl OptimizationHub {
    fn register(&self, record_id: &str) -> (u64, watch::Receiver<bool>) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = watch::channel(false);
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, previous)) = active.insert(record_id.to_string(), (id, tx)) {
            // 同一记录不允许两条流同时写：后来的请求取代旧请求
            let _ = previous.send(true);
        }
        (id, rx)
    }

    fn unregister(&self, record_id: &str, id: u64) {
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if active.get(record_id).map(|(current, _)| *current == id).unwrap_or(false) {
            active.remove(record_id);
        }
    }

    /// 取消指定记录正在执行的优化；没有活动任务返回 false。
    pub fn cancel(&self, record_id: &str) -> bool {
        let active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        match active.get(record_id) {
            Some((_, tx)) => tx.send(true).is_ok(),
            None => false,
        }
    }

    pub fn is_active(&self, record_id: &str) -> bool {
        self.active.lock().map(|a| a.contains_key(record_id)).unwrap_or(false)
    }
}

/// 是否配置了可用的默认优化模型（自动优化的前置条件；未配置时不该把录音卡在「优化中」）。
pub fn has_default_model(core: &CoreState) -> bool {
    core.llm.default_model_id().is_some()
}

/// 优化一条记录的原文，并按结果落盘。`on_event` 在调用方任务内同步回调。
pub async fn optimize_record(core: &CoreState, record_id: &str, text: &str, mut on_event: impl FnMut(OptimizeEvent)) -> OptimizeOutcome {
    let (task_id, mut cancel_rx) = core.optimizer.register(record_id);
    let outcome = async {
        set_status(core, record_id, RecordStatus::Optimizing, None);
        on_event(OptimizeEvent::Started);
        tokio::select! {
            biased;
            _ = cancel_rx.wait_for(|cancelled| *cancelled) => None,
            result = run(core, text, &mut on_event) => Some(result),
        }
    }
    .await;
    core.optimizer.unregister(record_id, task_id);

    match outcome {
        None => {
            set_status(core, record_id, RecordStatus::Transcribed, Some(String::new()));
            tracing::info!("优化已取消（记录 {record_id}）");
            OptimizeOutcome::Cancelled
        }
        Some(Ok(success)) => {
            let saved = core.records.patch(record_id, |record| {
                record.optimized_text = Some(success.optimized_text.clone());
                record.status = RecordStatus::Optimized;
                record.error = None;
            });
            if let Err(error) = saved {
                tracing::error!("保存优化结果失败（记录 {record_id}）: {error}");
            }
            OptimizeOutcome::Completed(success)
        }
        Some(Err(error)) => {
            let message = error.user_message.clone();
            tracing::warn!("优化失败（记录 {record_id}）: {message}");
            set_status(core, record_id, RecordStatus::Transcribed, Some(message.clone()));
            OptimizeOutcome::Failed(message)
        }
    }
}

fn set_status(core: &CoreState, record_id: &str, status: RecordStatus, error: Option<String>) {
    let result = core.records.patch(record_id, |record| {
        record.status = status;
        if let Some(error) = error {
            record.error = (!error.is_empty()).then_some(error);
        }
    });
    if let Err(error) = result {
        tracing::warn!("更新记录状态失败（记录 {record_id}）: {error}");
    }
}

/// 解析模板与模型 → 流式补全（首字前失败回退非流式）→ 清洗。
async fn run(core: &CoreState, text: &str, on_event: &mut impl FnMut(OptimizeEvent)) -> Result<OptimizeSuccess, LlmError> {
    let started = Instant::now();
    if text.trim().is_empty() {
        return Ok(OptimizeSuccess {
            optimized_text: text.to_string(),
            processing_time: 0.0,
            tokens_used: 0,
            truncated: false,
            model_string: String::new(),
            degraded: false,
        });
    }
    let settings = core.settings();
    let template = crate::prompts::resolve_template(&core.prompts_dir, settings.default_preset_id.as_deref())
        .unwrap_or_else(|| prompt::DEFAULT_PROMPT.to_string());
    let messages = prompt::build_messages(text, &template);

    let target = core.llm.resolve_runtime(None)?;
    let spec = registry::get(&target.profile.provider)
        .ok_or_else(|| LlmError::plain(CODE_UNSUPPORTED_PROVIDER, format!("未知服务商：{}", target.profile.provider)))?;
    let endpoint = Endpoint::resolve(spec, target.profile.api_base.as_deref(), &target.api_key, &target.profile.extra_headers)?;
    let params = &target.model.params;
    let budget = timeouts::derive(params.timeout_s, text.chars().count());
    let options = ChatOptions {
        max_tokens: params.max_tokens,
        temperature: Some(params.temperature),
        timeout: budget.request,
        retries: params.retries.max(0) as u32,
        reasoning_effort: params.reasoning_effort.clone(),
    };
    let model = target.model.model.as_str();
    tracing::info!(
        model,
        preset = ?settings.default_preset_id,
        text_length = text.chars().count(),
        "开始流式文本优化"
    );

    let mut streamed = String::new();
    let stream_budget = StreamBudget { first_token: budget.first_token, chunk_idle: budget.chunk_idle };
    let streamed_result = gateway::chat_stream(&endpoint, model, &messages, &options, stream_budget, |delta| {
        streamed.push_str(delta);
        on_event(OptimizeEvent::Delta(delta.to_string()));
    })
    .await
    .and_then(|result| finalize(result, params.max_tokens));

    let (optimized_text, result, degraded) = match streamed_result {
        Ok((text, result)) => (text, result, false),
        // 首条增量前失败：回退一次非流式（兼容不支持 stream 的服务商）；确定性失败不回退
        Err(error) if streamed.is_empty() && error.retryable() => {
            tracing::info!(error_code = error.code, "流式优化在首条增量前失败，回退非流式调用");
            let result = gateway::chat(&endpoint, model, &messages, &options).await?;
            let (text, result) = finalize(result, params.max_tokens)?;
            on_event(OptimizeEvent::Delta(text.clone()));
            (text, result, true)
        }
        Err(error) => return Err(error),
    };
    let success = OptimizeSuccess {
        optimized_text,
        processing_time: started.elapsed().as_secs_f64(),
        tokens_used: result.total_tokens,
        truncated: result.truncated(),
        model_string: model.to_string(),
        degraded,
    };
    tracing::info!(
        model,
        original_length = text.chars().count(),
        optimized_length = success.optimized_text.chars().count(),
        processing_time = format!("{:.2}s", success.processing_time),
        degraded,
        "文本优化完成"
    );
    Ok(success)
}

/// 截断判定 + 输出清洗：正文为空且被截断 → 不可重试；清洗后为空 → 失败可见（不把空文本写进记录）。
fn finalize(result: gateway::ChatResult, max_tokens: i64) -> Result<(String, gateway::ChatResult), LlmError> {
    if result.text.trim().is_empty() && result.truncated() {
        return Err(LlmError::new(CODE_OUTPUT_TRUNCATED, format!("模型的思考 token 占满了 max_tokens={max_tokens}，正文为空")));
    }
    let cleaned = clean_output(&result.text);
    if cleaned.is_empty() {
        return Err(LlmError::new(CODE_PROVIDER, "模型输出在清洗后为空（正文只有思考块或空白）"));
    }
    Ok((cleaned, result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::store::{ModelItemCreate, ProfileCreate};
    use crate::llm::testing::{stub_routes, StubRoute};
    use crate::records::Record;
    use std::collections::BTreeMap;

    /// 临时数据目录 + 一条记录 + 指向桩的 custom 档案/默认模型 + 一个预设。
    fn setup(base: &str, tag: &str) -> (CoreState, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("att-opt-{tag}-{}", uuid::Uuid::new_v4()));
        let data_dir = root.join("data");
        std::fs::create_dir_all(root.join("prompts")).unwrap();
        std::fs::write(root.join("prompts").join("程序员模式.txt"), "你是程序员助手").unwrap();
        let core = CoreState::init(&data_dir, crate::Platform::Macos).unwrap();
        core.update_settings(|s| s.default_preset_id = Some("preset-程序员模式".into())).unwrap();
        core.records
            .insert(Record { id: "rec-1".into(), raw_text: Some("嗯那个明天开会".into()), status: RecordStatus::Transcribed, ..Default::default() })
            .unwrap();
        let profile = core
            .llm
            .create_profile(ProfileCreate {
                provider: "custom".into(),
                name: "桩".into(),
                api_key: "sk-stub-key-0001".into(),
                api_base: Some(format!("{base}/v1")),
                extra_headers: BTreeMap::new(),
                extra_query: BTreeMap::new(),
            })
            .unwrap();
        core.llm
            .create_models(vec![ModelItemCreate { profile_id: profile.id, model: "m".into(), display_name: String::new(), params: None, set_default: true }])
            .unwrap();
        (core, root)
    }

    fn collect() -> (std::sync::Arc<Mutex<Vec<String>>>, impl FnMut(OptimizeEvent)) {
        let deltas = std::sync::Arc::new(Mutex::new(Vec::new()));
        let sink = deltas.clone();
        (deltas, move |event| {
            if let OptimizeEvent::Delta(text) = event {
                sink.lock().unwrap().push(text);
            }
        })
    }

    #[tokio::test]
    async fn 流式优化_用预设模板_清洗后落盘() {
        let (base, mut requests) = stub_routes(vec![StubRoute::sse(
            "POST /v1/chat/completions",
            &[
                r#"{"choices":[{"delta":{"content":"以下是整理后的内容："}}]}"#,
                r#"{"choices":[{"delta":{"content":"明天开会。"},"finish_reason":"stop"}]}"#,
            ],
        )])
        .await;
        let (core, root) = setup(&base, "stream");
        let (deltas, on_event) = collect();
        let outcome = optimize_record(&core, "rec-1", "嗯那个明天开会", on_event).await;
        let OptimizeOutcome::Completed(success) = outcome else { panic!("应成功: {outcome:?}") };
        assert_eq!(success.optimized_text, "明天开会。", "开头套话被清洗");
        assert!(!success.degraded);
        assert_eq!(deltas.lock().unwrap().join(""), "以下是整理后的内容：明天开会。");
        let record = core.records.get("rec-1").unwrap();
        assert_eq!(record.status, RecordStatus::Optimized);
        assert_eq!(record.optimized_text.as_deref(), Some("明天开会。"));
        let request = requests.recv().await.unwrap();
        assert!(request.contains("你是程序员助手"), "使用设置里的默认预设");
        assert!(request.contains("<transcript>"), "正文进信封");
        assert!(request.contains(r#""model":"m""#));
        assert!(!core.optimizer.is_active("rec-1"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn 流式首字前失败_回退非流式() {
        let (base, _requests) = stub_routes(vec![
            StubRoute::json("POST /v1/chat/completions", 400, r#"{"error":{"message":"stream not supported"}}"#).when_body(r#""stream":true"#),
            StubRoute::json("POST /v1/chat/completions", 200, r#"{"choices":[{"message":{"content":"明天开会。"},"finish_reason":"stop"}]}"#),
        ])
        .await;
        let (core, root) = setup(&base, "fallback");
        let (deltas, on_event) = collect();
        let OptimizeOutcome::Completed(success) = optimize_record(&core, "rec-1", "嗯那个明天开会", on_event).await else {
            panic!("应回退成功")
        };
        assert!(success.degraded);
        assert_eq!(deltas.lock().unwrap().as_slice(), ["明天开会。"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn 思考吃满预算_不回退_记录复位并写入错误() {
        let (base, mut requests) = stub_routes(vec![StubRoute::sse(
            "POST /v1/chat/completions",
            &[r#"{"choices":[{"delta":{"reasoning_content":"想很久"},"finish_reason":"length"}]}"#],
        )])
        .await;
        let (core, root) = setup(&base, "truncated");
        let outcome = optimize_record(&core, "rec-1", "嗯那个明天开会", |_| {}).await;
        let OptimizeOutcome::Failed(message) = outcome else { panic!("应失败") };
        assert!(message.contains("思考 token"));
        let record = core.records.get("rec-1").unwrap();
        assert_eq!(record.status, RecordStatus::Transcribed);
        assert!(record.error.unwrap().contains("思考 token"));
        requests.recv().await.unwrap();
        assert!(requests.try_recv().is_err(), "确定性失败不得回退重跑");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn 取消_记录复位为已转写() {
        // 桩不响应（连接挂起），模拟上游慢
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });
        let (core, root) = setup(&base, "cancel");
        let core = std::sync::Arc::new(core);
        let task_core = core.clone();
        let task = tokio::spawn(async move { optimize_record(&task_core, "rec-1", "嗯那个明天开会", |_| {}).await });
        for _ in 0..50 {
            if core.optimizer.is_active("rec-1") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(core.optimizer.cancel("rec-1"));
        assert!(matches!(task.await.unwrap(), OptimizeOutcome::Cancelled));
        assert_eq!(core.records.get("rec-1").unwrap().status, RecordStatus::Transcribed);
        assert!(!core.optimizer.cancel("rec-1"), "已结束的优化不可再取消");
        let _ = std::fs::remove_dir_all(&root);
    }
}
