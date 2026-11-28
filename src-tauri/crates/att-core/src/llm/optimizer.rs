//! 记录优化编排（移植自 Python `app/llm/optimizer.py` + `app/services/optimization_stream_service.py`）。
//!
//! 一次优化：记录置 optimizing → 解析预设模板与默认模型 → 流式补全（增量回调）→ 清洗 → 落盘。
//! 约束（每条都是 Python 版踩过的坑）：
//! - 任何失败/取消都必须把记录从 optimizing 复位为 transcribed，否则会永久卡状态：登记由 [`OptimizingLease`]
//!   持有，请求断开、调用方 future 被丢弃时由它的 Drop 兜底复位；被新任务取代的旧任务不再改记录状态；
//!   进程退出时还在优化的记录由启动时的 [`recover_interrupted`] 复位；
//! - 流式在首条增量前失败时回退一次非流式；已产出增量后不回退（避免重复计费/重复追加）；
//! - 确定性失败（思考 token 吃满预算 → 正文为空）不回退；
//! - 输入超过 [`prompt::MAX_ENVELOPE_CHARS`] 直接失败、不调用上游；输出被 max_tokens 截断
//!   （finish_reason=length）一律按失败处理，截断稿不写进记录、不交付；
//! - 同一记录同时只允许一条优化，后来的请求取代旧请求；
//! - 失败文案全部已脱敏（网关层保证）。

use super::cleaning::clean_output;
use super::errors::{LlmError, CODE_INPUT_TOO_LONG, CODE_OUTPUT_TRUNCATED, CODE_PROVIDER, CODE_UNSUPPORTED_PROVIDER};
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
    /// 本次优化耗时；记录里另存为 `optimize_time`，不要写进 `processing_time`
    pub processing_time: f64,
    pub tokens_used: i64,
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

    /// 注销任务；返回它注销时是否仍是该记录的当前任务（false = 已被新任务取代，不能再改记录状态）。
    fn unregister(&self, record_id: &str, id: u64) -> bool {
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        let current = active.get(record_id).map(|(current, _)| *current == id).unwrap_or(false);
        if current {
            active.remove(record_id);
        }
        current
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

/// 一条优化任务的登记凭据：注册时把记录置为 optimizing，结束时由 [`OptimizingLease::finish`] 注销。
///
/// 没走到 `finish` 就被丢弃（HTTP 请求断开、调用方超时丢弃 future、任务被中止）时，Drop 兜底：
/// 注销登记；如果自己仍是该记录的当前任务、记录还停在 optimizing，就复位为 transcribed。
struct OptimizingLease<'a> {
    core: &'a CoreState,
    record_id: String,
    id: u64,
    finished: bool,
}

impl<'a> OptimizingLease<'a> {
    fn register(core: &'a CoreState, record_id: &str) -> (Self, watch::Receiver<bool>) {
        let (id, cancel_rx) = core.optimizer.register(record_id);
        set_status(core, record_id, RecordStatus::Optimizing, None);
        (Self { core, record_id: record_id.to_string(), id, finished: false }, cancel_rx)
    }

    /// 正常结束：注销登记；返回是否仍是当前任务。
    fn finish(mut self) -> bool {
        self.finished = true;
        self.core.optimizer.unregister(&self.record_id, self.id)
    }
}

impl Drop for OptimizingLease<'_> {
    fn drop(&mut self) {
        if self.finished || !self.core.optimizer.unregister(&self.record_id, self.id) {
            return;
        }
        tracing::info!("优化被中断（记录 {}），复位状态", self.record_id);
        let result = self.core.records.patch(&self.record_id, |record| {
            if record.status == RecordStatus::Optimizing {
                record.status = RecordStatus::Transcribed;
            }
        });
        if let Err(error) = result {
            tracing::warn!("复位优化状态失败（记录 {}）: {error}", self.record_id);
        }
    }
}

/// 启动时调用：上次退出时还停在 optimizing 的记录（优化任务随进程没了）复位。
/// 已有优化稿的复位为 optimized，否则为 transcribed；正在优化的不动。返回改了几条。
pub fn recover_interrupted(core: &CoreState) -> usize {
    core.records
        .optimizing_ids()
        .into_iter()
        .filter(|id| {
            let mut changed = false;
            let result = core.records.patch(id, |record| {
                // 锁内再确认一次：恢复跑的同时用户可能已经点了新的优化
                if record.status == RecordStatus::Optimizing && !core.optimizer.is_active(&record.id) {
                    let has_optimized = record.optimized_text.as_deref().is_some_and(|text| !text.trim().is_empty());
                    record.status = if has_optimized { RecordStatus::Optimized } else { RecordStatus::Transcribed };
                    changed = true;
                }
            });
            result.is_ok() && changed
        })
        .count()
}

/// 是否配置了可用的默认优化模型（自动优化的前置条件；未配置时不该把录音卡在「优化中」）。
pub fn has_default_model(core: &CoreState) -> bool {
    core.llm.default_model_id().is_some()
}

/// 优化一条记录的原文（用默认预设），并按结果落盘。`on_event` 在调用方任务内同步回调。
pub async fn optimize_record(
    core: &CoreState,
    record_id: &str,
    text: &str,
    on_event: impl FnMut(OptimizeEvent),
) -> OptimizeOutcome {
    optimize_record_with_preset(core, record_id, text, None, on_event).await
}

/// 同 [`optimize_record`]，`preset_id` 指定本次用的预设（None = 默认预设；找不到时同样回退默认）。
/// 「优化选中文本」用它套自己的预设，其余流程（流式、清洗、落盘、取消）与录音优化完全一致。
pub async fn optimize_record_with_preset(
    core: &CoreState,
    record_id: &str,
    text: &str,
    preset_id: Option<&str>,
    mut on_event: impl FnMut(OptimizeEvent),
) -> OptimizeOutcome {
    let (lease, mut cancel_rx) = OptimizingLease::register(core, record_id);
    on_event(OptimizeEvent::Started);
    let outcome = tokio::select! {
        biased;
        _ = cancel_rx.wait_for(|cancelled| *cancelled) => None,
        result = run(core, text, preset_id, &mut on_event) => Some(result),
    };
    let current = lease.finish();
    finish_optimization(core, record_id, outcome, current)
}

/// 用内置的「语音指令改写选中文本」模板执行一次优化。
///
/// 语音识别出的 `instruction` 是可执行要求；`selection` 只是被改写对象。记录里的原文由调用方按
/// 「要求 + 原文」组装，这里只负责生成提示词、调用模型与落盘。
pub async fn optimize_record_with_voice_rewrite(
    core: &CoreState,
    record_id: &str,
    instruction: &str,
    selection: &str,
    mut on_event: impl FnMut(OptimizeEvent),
) -> OptimizeOutcome {
    let (lease, mut cancel_rx) = OptimizingLease::register(core, record_id);
    on_event(OptimizeEvent::Started);
    let outcome = tokio::select! {
        biased;
        _ = cancel_rx.wait_for(|cancelled| *cancelled) => None,
        result = run_voice_rewrite(core, instruction, selection, &mut on_event) => Some(result),
    };
    let current = lease.finish();
    finish_optimization(core, record_id, outcome, current)
}

/// 按结果落盘。`current` 为 false 表示本任务已被同一记录的新任务取代：记录状态归新任务管，这里一律不写。
fn finish_optimization(
    core: &CoreState,
    record_id: &str,
    outcome: Option<Result<OptimizeSuccess, LlmError>>,
    current: bool,
) -> OptimizeOutcome {
    if !current {
        tracing::info!("优化已被新任务取代（记录 {record_id}），旧任务不改记录");
        return match outcome {
            Some(Ok(success)) => OptimizeOutcome::Completed(success),
            Some(Err(error)) => OptimizeOutcome::Failed(error.user_message),
            None => OptimizeOutcome::Cancelled,
        };
    }
    match outcome {
        None => {
            set_status(core, record_id, RecordStatus::Transcribed, Some(String::new()));
            tracing::info!("优化已取消（记录 {record_id}）");
            OptimizeOutcome::Cancelled
        }
        Some(Ok(success)) => {
            let saved = core.records.patch(record_id, |record| {
                record.optimized_text = Some(success.optimized_text.clone());
                record.optimize_time = Some((success.processing_time * 100.0).round() / 100.0);
                // token 用量累加（同一条记录多次优化都算进用量统计）；服务商没报用量时不动
                if success.tokens_used > 0 {
                    record.optimize_tokens =
                        Some(record.optimize_tokens.unwrap_or(0).saturating_add(success.tokens_used as u64));
                }
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

/// 解析预设模板与模型 → 组装消息。
async fn run(
    core: &CoreState,
    text: &str,
    preset_id: Option<&str>,
    on_event: &mut impl FnMut(OptimizeEvent),
) -> Result<OptimizeSuccess, LlmError> {
    if text.trim().is_empty() {
        return Ok(unchanged(text));
    }
    check_input_len(text, "原文", "请分段优化")?;
    let settings = core.settings();
    // 查找链：本次指定 → 设置里的默认预设 → 内置默认；指定的预设被删了也落到用户自己选的默认，而不是直接跳内置
    let template = core
        .prompt_library()
        .resolve_template(&[preset_id, settings.default_preset_id.as_deref()])
        .unwrap_or_else(|| prompt::DEFAULT_PROMPT.to_string());
    let preset_id = preset_id.or(settings.default_preset_id.as_deref());
    let messages = prompt::build_messages(text, &template, &settings.asr_hotwords);
    let prompt = format!("预设：{preset_id:?}");
    let glossary = settings.asr_hotwords.len();
    run_chat(core, messages, text.chars().count(), &prompt, glossary, on_event).await
}

/// 用固定的语音改写模板组装消息；不读取用户预设，避免临时要求被某个「整理口述」预设干扰。
async fn run_voice_rewrite(
    core: &CoreState,
    instruction: &str,
    selection: &str,
    on_event: &mut impl FnMut(OptimizeEvent),
) -> Result<OptimizeSuccess, LlmError> {
    if instruction.trim().is_empty() || selection.trim().is_empty() {
        return Ok(unchanged(selection));
    }
    check_input_len(selection, "选中的文字", "请分段改写")?;
    check_input_len(instruction, "改写要求", "请说得简短一些")?;
    let messages = prompt::build_voice_rewrite_messages(instruction, selection);
    let input_chars = instruction.chars().count() + selection.chars().count();
    run_chat(core, messages, input_chars, "语音改写选中文本", 0, on_event).await
}

/// 输入超过单次上限时直接报错（不截断、不调用上游）：截断后的结果会被当成完整稿落盘、替换选区。
fn check_input_len(text: &str, what: &str, advice: &str) -> Result<(), LlmError> {
    let chars = text.chars().count();
    if chars <= prompt::MAX_ENVELOPE_CHARS {
        return Ok(());
    }
    Err(LlmError::plain(
        CODE_INPUT_TOO_LONG,
        format!("{what}超过 {} 字（当前 {chars} 字），{advice}", prompt::MAX_ENVELOPE_CHARS),
    ))
}

fn unchanged(text: &str) -> OptimizeSuccess {
    OptimizeSuccess {
        optimized_text: text.to_string(),
        processing_time: 0.0,
        tokens_used: 0,
        model_string: String::new(),
        degraded: false,
    }
}

/// 模型调用 → 流式补全（首字前失败回退非流式）→ 清洗。
async fn run_chat(
    core: &CoreState,
    messages: Vec<gateway::ChatMessage>,
    input_chars: usize,
    prompt_label: &str,
    glossary: usize,
    on_event: &mut impl FnMut(OptimizeEvent),
) -> Result<OptimizeSuccess, LlmError> {
    let started = Instant::now();
    let target = core.llm.resolve_runtime(None)?;
    let spec = registry::get(&target.profile.provider).ok_or_else(|| {
        LlmError::plain(CODE_UNSUPPORTED_PROVIDER, format!("未知服务商：{}", target.profile.provider))
    })?;
    let endpoint =
        Endpoint::resolve(spec, target.profile.api_base.as_deref(), &target.api_key, &target.profile.extra_headers)?;
    let params = &target.model.params;
    let budget = timeouts::derive(params.timeout_s, input_chars);
    let options = ChatOptions {
        max_tokens: params.max_tokens,
        temperature: Some(params.temperature),
        timeout: budget.request,
        retries: params.retries.max(0) as u32,
        reasoning_effort: params.reasoning_effort.clone(),
    };
    let model = target.model.model.as_str();
    tracing::info!(model, prompt = prompt_label, text_length = input_chars, glossary, "开始流式文本优化");

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
        // 首条增量前遇到暂时性失败：回退一次非流式；4xx 与确定性失败不回退
        Err(error) if streamed.is_empty() && error.retryable() => {
            tracing::info!(error_code = error.code, "流式优化在首条增量前失败，回退非流式调用");
            let result = gateway::chat(&endpoint, model, &messages, &options).await?;
            let (text, result) = finalize(result, params.max_tokens)?;
            on_event(OptimizeEvent::Delta(text.clone()));
            (text, result, true)
        }
        // 已有增量时不重跑：保留已流出的内容，但必须报错并让上层把记录复位为已转写。
        Err(error) => return Err(error),
    };
    let success = OptimizeSuccess {
        optimized_text,
        processing_time: started.elapsed().as_secs_f64(),
        tokens_used: result.total_tokens,
        model_string: model.to_string(),
        degraded,
    };
    tracing::info!(
        model,
        original_length = input_chars,
        optimized_length = success.optimized_text.chars().count(),
        processing_time = format!("{:.2}s", success.processing_time),
        degraded,
        "文本优化完成"
    );
    Ok(success)
}

/// 截断判定 + 输出清洗。被 max_tokens 截断的结果一律失败（确定性失败，不回退重跑）：
/// 正文为空 → 思考 token 吃满预算；正文不为空 → 截断稿不完整，不写进记录、不替换选区。
/// 清洗后为空 → 失败可见（不把空文本写进记录）。
fn finalize(result: gateway::ChatResult, max_tokens: i64) -> Result<(String, gateway::ChatResult), LlmError> {
    if result.truncated() {
        if result.text.trim().is_empty() {
            return Err(LlmError::new(
                CODE_OUTPUT_TRUNCATED,
                format!("模型的思考 token 占满了 max_tokens={max_tokens}，正文为空"),
            ));
        }
        return Err(LlmError::plain(
            CODE_OUTPUT_TRUNCATED,
            format!(
                "输出被截断，已保留原文：输出达到 Max Tokens 上限（max_tokens={max_tokens}）。\
                 可在 设置 → LLM 服务 里调大 Max Tokens，或分段优化"
            ),
        ));
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
            .insert(Record {
                id: "rec-1".into(),
                raw_text: Some("嗯那个明天开会".into()),
                status: RecordStatus::Transcribed,
                processing_time: Some(0.6),
                ..Default::default()
            })
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
            .create_models(vec![ModelItemCreate {
                profile_id: profile.id,
                model: "m".into(),
                display_name: String::new(),
                params: None,
                set_default: true,
            }])
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
    async fn 优化完成_只写优化耗时_不改识别处理耗时() {
        let (base, _requests) = stub_routes(vec![StubRoute::sse(
            "POST /v1/chat/completions",
            &[r#"{"choices":[{"delta":{"content":"明天开会。"},"finish_reason":"stop"}]}"#],
        )])
        .await;
        let (core, root) = setup(&base, "processing-time");
        let before = core.records.get("rec-1").unwrap();
        assert_eq!(before.processing_time, Some(0.6), "识别阶段耗时由录音链路写入");

        let outcome = optimize_record(&core, "rec-1", "嗯那个明天开会", |_| {}).await;
        assert!(matches!(outcome, OptimizeOutcome::Completed(_)), "{outcome:?}");

        let after = core.records.get("rec-1").unwrap();
        assert_eq!(after.processing_time, Some(0.6), "自动优化共用这条链路，不能把优化时长写进处理耗时");
        assert!(after.optimize_time.is_some(), "优化耗时另存 optimize_time");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn 优化完成_token用量累加写进记录() {
        let (base, _requests) = stub_routes(vec![StubRoute::sse(
            "POST /v1/chat/completions",
            &[
                r#"{"choices":[{"delta":{"content":"明天开会。"},"finish_reason":"stop"}]}"#,
                r#"{"choices":[],"usage":{"prompt_tokens":30,"completion_tokens":12,"total_tokens":42}}"#,
            ],
        )])
        .await;
        let (core, root) = setup(&base, "tokens");
        assert_eq!(core.records.get("rec-1").unwrap().optimize_tokens, None, "没优化过时没有 token 字段");

        let outcome = optimize_record(&core, "rec-1", "嗯那个明天开会", |_| {}).await;
        assert!(matches!(outcome, OptimizeOutcome::Completed(_)), "{outcome:?}");
        assert_eq!(core.records.get("rec-1").unwrap().optimize_tokens, Some(42));

        let outcome = optimize_record(&core, "rec-1", "嗯那个明天开会", |_| {}).await;
        assert!(matches!(outcome, OptimizeOutcome::Completed(_)), "{outcome:?}");
        assert_eq!(core.records.get("rec-1").unwrap().optimize_tokens, Some(84), "再次优化累加，不覆盖");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn 指定预设_覆盖默认预设_找不到时回退默认() {
        let (base, mut requests) = stub_routes(vec![StubRoute::sse(
            "POST /v1/chat/completions",
            &[r#"{"choices":[{"delta":{"content":"明天开会。"},"finish_reason":"stop"}]}"#],
        )])
        .await;
        let (core, root) = setup(&base, "preset");
        std::fs::write(root.join("prompts").join("Agent 提示词.txt"), "你是提示词改写器").unwrap();
        let outcome =
            optimize_record_with_preset(&core, "rec-1", "嗯那个明天开会", Some("preset-Agent 提示词"), |_| {}).await;
        assert!(matches!(outcome, OptimizeOutcome::Completed(_)), "{outcome:?}");
        let request = requests.recv().await.unwrap();
        assert!(request.contains("你是提示词改写器") && !request.contains("你是程序员助手"), "用指定预设");

        let outcome = optimize_record_with_preset(&core, "rec-1", "嗯那个明天开会", None, |_| {}).await;
        assert!(matches!(outcome, OptimizeOutcome::Completed(_)));
        assert!(requests.recv().await.unwrap().contains("你是程序员助手"), "None = 默认预设");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn 语音改写_要求与选中文字分装_不读取用户预设() {
        let (base, mut requests) = stub_routes(vec![StubRoute::sse(
            "POST /v1/chat/completions",
            &[r#"{"choices":[{"delta":{"content":"该方案整体可行。"},"finish_reason":"stop"}]}"#],
        )])
        .await;
        let (core, root) = setup(&base, "voice-rewrite");
        let outcome =
            optimize_record_with_voice_rewrite(&core, "rec-1", "改成更正式的语气", "这个方案大概可以", |_| {}).await;
        assert!(matches!(outcome, OptimizeOutcome::Completed(_)), "{outcome:?}");
        let request = requests.recv().await.unwrap();
        assert!(request.contains("语音改写选中文本"), "使用固定内置模板");
        assert!(request.contains("<rewrite_instruction>\\n改成更正式的语气\\n</rewrite_instruction>"));
        assert!(request.contains("<selected_text>\\n这个方案大概可以\\n</selected_text>"));
        assert!(!request.contains("你是程序员助手"), "不受设置里的默认预设影响");
        assert_eq!(core.records.get("rec-1").unwrap().optimized_text.as_deref(), Some("该方案整体可行。"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn 流式首字前暂时失败_回退非流式() {
        let (base, _requests) = stub_routes(vec![
            StubRoute::json("POST /v1/chat/completions", 503, r#"{"error":{"message":"stream unavailable"}}"#)
                .when_body(r#""stream":true"#),
            StubRoute::json(
                "POST /v1/chat/completions",
                200,
                r#"{"choices":[{"message":{"content":"明天开会。"},"finish_reason":"stop"}]}"#,
            ),
        ])
        .await;
        let (core, root) = setup(&base, "fallback");
        let (deltas, on_event) = collect();
        let OptimizeOutcome::Completed(success) = optimize_record(&core, "rec-1", "嗯那个明天开会", on_event).await
        else {
            panic!("应回退成功")
        };
        assert!(success.degraded);
        assert_eq!(deltas.lock().unwrap().as_slice(), ["明天开会。"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn 流式_http_400_不回退非流式() {
        let (base, mut requests) = stub_routes(vec![StubRoute::json(
            "POST /v1/chat/completions",
            400,
            r#"{"error":{"message":"context length exceeded"}}"#,
        )])
        .await;
        let (core, root) = setup(&base, "stream-400");
        let outcome = optimize_record(&core, "rec-1", "嗯那个明天开会", |_| {}).await;
        let OptimizeOutcome::Failed(message) = outcome else { panic!("应失败: {outcome:?}") };
        assert!(message.contains("HTTP 400"), "{message}");
        requests.recv().await.expect("应收到流式请求");
        assert!(requests.try_recv().is_err(), "4xx 不得回退非流式重跑");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn 流式有增量后提前_eof_不回退且记录复位() {
        let (base, mut requests) = stub_routes(vec![
            StubRoute::sse_raw(
                "POST /v1/chat/completions",
                "data: {\"choices\":[{\"delta\":{\"content\":\"半截优化\"}}]}\n\n",
            ),
            StubRoute::json(
                "POST /v1/chat/completions",
                200,
                r#"{"choices":[{"message":{"content":"完整结果"},"finish_reason":"stop"}]}"#,
            ),
        ])
        .await;
        let (core, root) = setup(&base, "early-eof");
        let (deltas, on_event) = collect();
        let outcome = optimize_record(&core, "rec-1", "嗯那个明天开会", on_event).await;
        let OptimizeOutcome::Failed(message) = outcome else { panic!("应报错: {outcome:?}") };
        assert!(message.contains("不完整"), "{message}");
        assert_eq!(deltas.lock().unwrap().as_slice(), ["半截优化"]);

        let record = core.records.get("rec-1").unwrap();
        assert_eq!(record.status, RecordStatus::Transcribed);
        assert!(record.optimized_text.is_none(), "不得把半截结果落盘为 optimized");
        assert!(record.error.unwrap().contains("不完整"));
        requests.recv().await.unwrap();
        assert!(requests.try_recv().is_err(), "已有增量时不得回退非流式重跑");
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
    async fn 输出被截断_按失败处理_不改写优化稿() {
        let (base, mut requests) = stub_routes(vec![StubRoute::sse(
            "POST /v1/chat/completions",
            &[
                r#"{"choices":[{"delta":{"content":"明天开会，"}}]}"#,
                r#"{"choices":[{"delta":{"content":"议题是"},"finish_reason":"length"}]}"#,
            ],
        )])
        .await;
        let (core, root) = setup(&base, "output-truncated");
        core.records.patch("rec-1", |record| record.optimized_text = Some("旧优化稿".into())).unwrap();
        let (deltas, on_event) = collect();
        let outcome = optimize_record(&core, "rec-1", "嗯那个明天开会", on_event).await;
        let OptimizeOutcome::Failed(message) = outcome else { panic!("截断稿不得当成完成: {outcome:?}") };
        assert!(message.contains("输出被截断，已保留原文") && message.contains("max_tokens="), "{message}");
        assert_eq!(deltas.lock().unwrap().join(""), "明天开会，议题是", "增量照常推送，由调用方决定怎么收尾");
        let record = core.records.get("rec-1").unwrap();
        assert_eq!(record.status, RecordStatus::Transcribed);
        assert_eq!(record.optimized_text.as_deref(), Some("旧优化稿"), "截断稿不得写进记录");
        assert!(record.error.unwrap().contains("输出被截断"));
        requests.recv().await.unwrap();
        assert!(requests.try_recv().is_err(), "截断是确定性失败，不得回退重跑");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn 输入超过上限_直接失败_不调用上游() {
        let (base, mut requests) = stub_routes(vec![StubRoute::sse(
            "POST /v1/chat/completions",
            &[r#"{"choices":[{"delta":{"content":"明天开会。"},"finish_reason":"stop"}]}"#],
        )])
        .await;
        let (core, root) = setup(&base, "input-too-long");
        let at_limit = "字".repeat(prompt::MAX_ENVELOPE_CHARS);
        let too_long = "字".repeat(prompt::MAX_ENVELOPE_CHARS + 1);

        let outcome = optimize_record(&core, "rec-1", &too_long, |_| {}).await;
        let OptimizeOutcome::Failed(message) = outcome else { panic!("应失败: {outcome:?}") };
        assert!(message.contains("原文超过 16000 字（当前 16001 字），请分段优化"), "{message}");
        let record = core.records.get("rec-1").unwrap();
        assert_eq!(record.status, RecordStatus::Transcribed);
        assert!(record.optimized_text.is_none());

        let outcome = optimize_record_with_voice_rewrite(&core, "rec-1", "改正式一点", &too_long, |_| {}).await;
        let OptimizeOutcome::Failed(message) = outcome else { panic!("应失败: {outcome:?}") };
        assert!(message.contains("选中的文字超过 16000 字"), "{message}");
        assert!(requests.try_recv().is_err(), "超长输入不得调用上游");

        let outcome = optimize_record(&core, "rec-1", &at_limit, |_| {}).await;
        assert!(matches!(outcome, OptimizeOutcome::Completed(_)), "恰好到上限仍可优化: {outcome:?}");
        let request = requests.recv().await.unwrap();
        assert!(request.contains(&at_limit) && !request.contains("[truncated]"), "正文完整送出");
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

    fn slow_sse(delay: std::time::Duration) -> StubRoute {
        StubRoute::sse(
            "POST /v1/chat/completions",
            &[r#"{"choices":[{"delta":{"content":"明天开会。"},"finish_reason":"stop"}]}"#],
        )
        .after(delay)
    }

    #[tokio::test]
    async fn 调用方丢弃_future_后记录复位且不再登记() {
        let (base, _requests) = stub_routes(vec![slow_sse(std::time::Duration::from_secs(30))]).await;
        let (core, root) = setup(&base, "dropped");
        let (_deltas, on_event) = collect();
        // 模拟请求断开 / 前端超时：handler future 被丢弃，走不到正常收尾
        let timed_out = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            optimize_record(&core, "rec-1", "嗯那个明天开会", on_event),
        )
        .await;
        assert!(timed_out.is_err(), "桩挂起，应超时");
        assert_eq!(core.records.get("rec-1").unwrap().status, RecordStatus::Transcribed, "不能卡在优化中");
        assert!(!core.optimizer.is_active("rec-1"), "登记必须随 future 一起清掉");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn 被取代的旧任务不回退新任务的状态() {
        let (base, _requests) = stub_routes(vec![slow_sse(std::time::Duration::from_millis(400))]).await;
        let (core, root) = setup(&base, "superseded");
        let (_first_deltas, first_events) = collect();
        let (_second_deltas, second_events) = collect();
        let ms = std::time::Duration::from_millis;
        let first = optimize_record(&core, "rec-1", "嗯那个明天开会", first_events);
        let second = async {
            tokio::time::sleep(ms(50)).await;
            optimize_record(&core, "rec-1", "嗯那个明天开会", second_events).await
        };
        let probe = async {
            // 旧任务此时已被取消并收尾；新任务仍在等上游
            tokio::time::sleep(ms(200)).await;
            core.records.get("rec-1").unwrap().status
        };
        let (first, second, during) = tokio::join!(first, second, probe);
        assert!(matches!(first, OptimizeOutcome::Cancelled), "{first:?}");
        assert_eq!(during, RecordStatus::Optimizing, "新任务进行期间状态保持优化中");
        assert!(matches!(second, OptimizeOutcome::Completed(_)), "{second:?}");
        assert_eq!(core.records.get("rec-1").unwrap().status, RecordStatus::Optimized);
        assert!(!core.optimizer.is_active("rec-1"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 启动恢复_卡在优化中的记录复位_进行中的不动() {
        let (core, root) = setup("http://127.0.0.1:9", "recover");
        for (id, optimized) in [("rec-stuck", None), ("rec-stuck-optimized", Some("旧优化稿")), ("rec-running", None)]
        {
            core.records
                .insert(Record {
                    id: id.into(),
                    raw_text: Some("原文".into()),
                    optimized_text: optimized.map(str::to_string),
                    status: RecordStatus::Optimizing,
                    ..Default::default()
                })
                .unwrap();
        }
        let (_running_id, _cancel) = core.optimizer.register("rec-running");

        assert_eq!(recover_interrupted(&core), 2);
        assert_eq!(core.records.get("rec-stuck").unwrap().status, RecordStatus::Transcribed);
        let optimized = core.records.get("rec-stuck-optimized").unwrap();
        assert_eq!(optimized.status, RecordStatus::Optimized, "有优化稿的复位为已优化");
        assert_eq!(optimized.optimized_text.as_deref(), Some("旧优化稿"), "恢复不能动文字");
        assert_eq!(core.records.get("rec-running").unwrap().status, RecordStatus::Optimizing);
        assert_eq!(core.records.get("rec-1").unwrap().status, RecordStatus::Transcribed);
        let _ = std::fs::remove_dir_all(&root);
    }
}
