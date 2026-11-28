//! ASR 文本 → LLM 滑动窗口校对（移植自 Python `app/services/transcribe_proofread.py`）。
//!
//! - 窗口双上限：每窗 ≤ batch_segments 段且 ≤ batch_chars 字；超长段先按句末标点拆成子段；可带前文尾部作参考；
//! - 每窗「LLM → 验证 → 反馈 → 重试」最多 [`MAX_STEPS`] 次：键集合完全相等 + 逐条相似度不越阈；
//! - 验证耗尽的窗口保留原文（计入 fallback_batches）；全部窗口都因 LLM 错误失败 → 整体降级不算失败；
//! - 输出被 max_tokens 截断时该窗立即失败，不再把半截回复拼进上下文追问；
//! - 窗口并发 [`CONCURRENCY`]，temperature=0.2，各窗 token 用量累计返回。

use crate::llm::errors::{LlmError, CODE_UNSUPPORTED_PROVIDER};
use crate::llm::gateway::{self, ChatMessage, ChatOptions, Endpoint};
use crate::llm::prompt::glossary_block;
use crate::llm::registry;
use crate::state::CoreState;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// 提示词版本：措辞调整必须递增，使校对缓存变体自然失效。
pub const PROMPT_VERSION: i64 = 1;
const MAX_STEPS: usize = 3;
const SIMILARITY_DEFAULT: f64 = 0.7;
const SIMILARITY_SHORT: f64 = 0.3;
const SHORT_TEXT_CHARS: usize = 20;
const CONCURRENCY: usize = 2;

const SYSTEM_PROMPT: &str = "你是语音转写（ASR）文本的校对员。用户给你一个 JSON 对象：键是段落编号，值是识别原文。
你的任务是逐条纠错，规则：
1. 只修正识别错误：错别字、同音/近音字、标点缺失或错用、明显的专有名词错误；
2. 删除口头语气词（呃、嗯、啊）与拟声词（*笑声*、*咳嗽*）；
3. 严禁：改写句式、替换同义词、翻译、总结、合并或拆分段落；
4. 前文参考（如有）仅用于判断术语与人称，不是待校对内容；
5. 输出键集合必须与输入完全一致，值与键一一对应；
6. 只输出 JSON 对象本体：不要解释、不要 markdown 代码围栏。";

#[derive(Debug, Clone, Copy)]
pub struct ProofreadOptions {
    pub batch_segments: usize,
    pub batch_chars: usize,
    pub context_chars: usize,
}

impl Default for ProofreadOptions {
    fn default() -> Self {
        Self { batch_segments: 40, batch_chars: 4000, context_chars: 200 }
    }
}

impl ProofreadOptions {
    /// 请求体 proofread_options → 选项；非法值回落默认（clamp 到安全区间）。
    pub fn from_request(value: Option<&Value>) -> Self {
        let base = Self::default();
        let Some(object) = value.and_then(Value::as_object) else { return base };
        let clamp = |key: &str, low: usize, high: usize, default: usize| {
            object.get(key).and_then(Value::as_i64).map(|v| (v.max(0) as usize).clamp(low, high)).unwrap_or(default)
        };
        Self {
            batch_segments: clamp("batch_segments", 1, 200, base.batch_segments),
            batch_chars: clamp("batch_chars", 200, 20000, base.batch_chars),
            context_chars: clamp("context_chars", 0, 2000, base.context_chars),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProofreadResult {
    pub applied: bool,
    pub text: String,
    pub segments: Vec<Value>,
    pub model_id: Option<String>,
    pub model_label: String,
    pub windows: usize,
    pub requests: usize,
    pub total_tokens: u64,
    pub fallback_batches: Vec<usize>,
    pub error: String,
    pub reason: String,
}

impl ProofreadResult {
    /// 全部窗口都校对成功：只有这种结果才写进缓存变体。
    pub fn is_complete(&self) -> bool {
        self.applied && self.fallback_batches.is_empty()
    }

    /// 响应 proofread 字段（不含正文）。
    pub fn meta(&self) -> Value {
        let mut meta = json!({
            "applied": self.applied,
            "llm_model": if self.model_label.is_empty() { self.model_id.clone().unwrap_or_else(|| "default".into()) } else { self.model_label.clone() },
            "windows": self.windows,
            "requests": self.requests,
            "total_tokens": self.total_tokens,
            "fallback_batches": self.fallback_batches,
            "prompt_version": PROMPT_VERSION,
        });
        if self.applied && !self.fallback_batches.is_empty() {
            // 部分窗口保留了原文（验证耗尽、输出被截断或请求失败）：结果不进缓存，下次会重新校对
            meta["partial"] = json!(true);
        }
        if !self.error.is_empty() {
            meta["error"] = json!(self.error);
        }
        if !self.reason.is_empty() {
            meta["reason"] = json!(self.reason);
        }
        meta
    }
}

/// 段索引切窗：双上限，只切在已展开子段的边界。
pub fn split_windows(texts: &[String], options: &ProofreadOptions) -> Vec<Vec<usize>> {
    let mut windows = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut current_chars = 0;
    for (index, text) in texts.iter().enumerate() {
        let length = text.chars().count();
        if !current.is_empty()
            && (current.len() >= options.batch_segments || current_chars + length > options.batch_chars)
        {
            windows.push(std::mem::take(&mut current));
            current_chars = 0;
        }
        current.push(index);
        current_chars += length;
    }
    if !current.is_empty() {
        windows.push(current);
    }
    windows
}

#[derive(Debug, Clone)]
struct ExpandedSegment {
    source_index: usize,
    value: Value,
}

fn is_sentence_end(ch: char) -> bool {
    matches!(ch, '。' | '！' | '？' | '；' | '.' | '!' | '?' | ';' | '\n')
}

fn is_closing_punctuation(ch: char) -> bool {
    matches!(ch, '”' | '’' | '」' | '』' | '）' | '】' | '》' | '〉' | '"' | '\'' | ')' | ']' | '}')
}

/// 把一段文本拆成不超过 `limit` 个字符的子段，优先在句末标点后断开。
fn split_text(text: &str, limit: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= limit {
        return vec![text.to_string()];
    }
    let limit = limit.max(1);
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let max_end = (start + limit).min(chars.len());
        let mut end = max_end;
        if max_end < chars.len() {
            let boundary = (start..max_end).rev().find(|index| is_sentence_end(chars[*index]));
            if let Some(index) = boundary {
                end = index + 1;
                while end < chars.len() && end < max_end && is_closing_punctuation(chars[end]) {
                    end += 1;
                }
            }
        }
        chunks.push(chars[start..end].iter().collect());
        start = end;
    }
    chunks
}

/// 超长 ASR 段先按句拆成子段；时间戳按子段字数比例分配，保持首尾和单调性。
fn split_oversized_segments(segments: &[Value], limit: usize) -> Vec<ExpandedSegment> {
    let mut expanded = Vec::new();
    for (source_index, segment) in segments.iter().enumerate() {
        let text = segment.get("text").and_then(Value::as_str).unwrap_or("");
        let chunks = split_text(text, limit.max(1));
        let chunk_count = chunks.len();
        let total_chars = chunks.iter().map(|chunk| chunk.chars().count()).sum::<usize>().max(1);
        let start = segment.get("start").and_then(Value::as_f64);
        let end = segment.get("end").and_then(Value::as_f64);
        let mut consumed = 0usize;
        for (chunk_index, chunk) in chunks.into_iter().enumerate() {
            let chunk_chars = chunk.chars().count();
            let chunk_start = consumed;
            consumed += chunk_chars;
            let mut value = segment.clone();
            if let Some(object) = value.as_object_mut() {
                object.insert("text".into(), json!(chunk));
                if let (Some(start), Some(end)) = (start, end) {
                    let duration = (end - start).max(0.0);
                    let ratio_start = chunk_start as f64 / total_chars as f64;
                    let ratio_end =
                        if chunk_index + 1 == chunk_count { 1.0 } else { consumed as f64 / total_chars as f64 };
                    object.insert("start".into(), json!(start + duration * ratio_start));
                    object.insert("end".into(), json!(start + duration * ratio_end));
                }
            }
            expanded.push(ExpandedSegment { source_index, value });
        }
    }
    expanded
}

fn clean_for_similarity(text: &str) -> Vec<char> {
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().collect()
}

/// 归一相似度（0~1）：与 Python `difflib.SequenceMatcher.ratio()` 同算法（最长公共子串递归匹配，2M/T）。
pub fn similarity(left: &str, right: &str) -> f64 {
    let a = clean_for_similarity(left);
    let b = clean_for_similarity(right);
    let total = a.len() + b.len();
    if total == 0 {
        return 1.0;
    }
    2.0 * matching_chars(&a, &b) as f64 / total as f64
}

fn matching_chars(a: &[char], b: &[char]) -> usize {
    // 找最长公共子串（动态规划，按行滚动），左右两侧递归
    if a.is_empty() || b.is_empty() {
        return 0;
    }
    let (mut best_len, mut best_a, mut best_b) = (0, 0, 0);
    let mut previous = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        let mut current = vec![0usize; b.len() + 1];
        for j in 1..=b.len() {
            if a[i - 1] == b[j - 1] {
                current[j] = previous[j - 1] + 1;
                if current[j] > best_len {
                    best_len = current[j];
                    best_a = i - best_len;
                    best_b = j - best_len;
                }
            }
        }
        previous = current;
    }
    if best_len == 0 {
        return 0;
    }
    best_len
        + matching_chars(&a[..best_a], &b[..best_b])
        + matching_chars(&a[best_a + best_len..], &b[best_b + best_len..])
}

/// 验证一窗结果：键集合完全相等 + 逐条相似度不越阈。返回 Err(反馈文案) 喂回 LLM 重试。
pub fn validate_window(
    original: &BTreeMap<usize, String>,
    corrected: &Map<String, Value>,
) -> Result<BTreeMap<usize, String>, String> {
    let expected: BTreeSet<String> = original.keys().map(|k| k.to_string()).collect();
    let actual: BTreeSet<String> = corrected.keys().cloned().collect();
    if expected != actual {
        let missing: Vec<&String> = expected.difference(&actual).collect();
        let extra: Vec<&String> = actual.difference(&expected).collect();
        let mut parts = Vec::new();
        if !missing.is_empty() {
            parts.push(format!("缺少键：{missing:?}"));
        }
        if !extra.is_empty() {
            parts.push(format!("多出键：{extra:?}"));
        }
        return Err(format!("{}。必须返回包含全部 {} 个键的完整 JSON 对象。", parts.join("；"), expected.len()));
    }
    let mut fixed = BTreeMap::new();
    for (key, raw) in original {
        let value = &corrected[&key.to_string()];
        let text = value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string());
        let threshold =
            if clean_for_similarity(raw).len() <= SHORT_TEXT_CHARS { SIMILARITY_SHORT } else { SIMILARITY_DEFAULT };
        if similarity(raw, &text) < threshold {
            return Err(format!("键 {key} 改动过大（相似度低于 {threshold}）：请只纠错，不要改写句式或替换同义词。"));
        }
        fixed.insert(*key, text);
    }
    Ok(fixed)
}

/// 宽容解析 LLM 输出：剥 markdown 围栏 / 前后缀噪音 / 尾随逗号。
pub fn parse_llm_json(content: &str) -> Option<Map<String, Value>> {
    let mut text = content.trim();
    if text.is_empty() {
        return None;
    }
    let fenced;
    if let Some(start) = text.find("```") {
        let after = &text[start + 3..];
        let after = after.strip_prefix("json").unwrap_or(after);
        if let Some(end) = after.find("```") {
            fenced = after[..end].trim().to_string();
            text = &fenced;
        }
    }
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    if end <= start {
        return None;
    }
    let body = &text[start..=end];
    let trailing_comma = regex::Regex::new(r",\s*([}\]])").expect("正则合法");
    let body = trailing_comma.replace_all(body, "$1");
    match serde_json::from_str::<Value>(&body).ok()? {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

fn build_user_prompt(original: &BTreeMap<usize, String>, context_tail: &str) -> String {
    let object: Map<String, Value> = original.iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
    let mut parts = Vec::new();
    if !context_tail.is_empty() {
        parts.push(format!("前文参考（仅供理解语境，不是待校对内容）：\n{context_tail}"));
    }
    parts.push(format!("待校对：\n{}", Value::Object(object)));
    parts.join("\n\n")
}

/// 对 ASR segments（`[{start,end,text}]`）执行滑窗校对；超长段的子段时间戳按字数比例重算。
pub async fn run(
    core: &CoreState,
    model_id: Option<&str>,
    options: ProofreadOptions,
    segments: Vec<Value>,
    on_progress: impl Fn(usize, usize) + Send + Sync,
) -> Result<ProofreadResult, LlmError> {
    let target = core.llm.resolve_runtime(model_id)?;
    let spec = registry::get(&target.profile.provider).ok_or_else(|| {
        LlmError::plain(CODE_UNSUPPORTED_PROVIDER, format!("未知服务商：{}", target.profile.provider))
    })?;
    let endpoint =
        Endpoint::resolve(spec, target.profile.api_base.as_deref(), &target.api_key, &target.profile.extra_headers)?;
    let label = if target.model.display_name.is_empty() {
        target.model.model.clone()
    } else {
        target.model.display_name.clone()
    };
    let original_texts: Vec<String> =
        segments.iter().map(|s| s.get("text").and_then(Value::as_str).unwrap_or("").to_string()).collect();
    let original_joined = original_texts.iter().filter(|t| !t.is_empty()).cloned().collect::<Vec<_>>().join(" ");
    let base = ProofreadResult {
        applied: false,
        text: String::new(),
        segments: segments.clone(),
        model_id: model_id.map(str::to_string),
        model_label: label,
        windows: 0,
        requests: 0,
        total_tokens: 0,
        fallback_batches: Vec::new(),
        error: String::new(),
        reason: String::new(),
    };
    if original_joined.trim().is_empty() {
        return Ok(ProofreadResult { reason: "asr_empty".into(), ..base });
    }
    let expanded = split_oversized_segments(&segments, options.batch_chars);
    let texts: Vec<String> = expanded
        .iter()
        .map(|segment| segment.value.get("text").and_then(Value::as_str).unwrap_or("").to_string())
        .collect();
    let joined: String = texts.iter().filter(|t| !t.is_empty()).cloned().collect::<Vec<_>>().join(" ");
    let windows = split_windows(&texts, &options);
    // 前文参考：按窗口起点从原文拼接里截取前 context_chars 字
    let joined_chars: Vec<char> = joined.chars().collect();
    let mut cumulative = Vec::with_capacity(texts.len());
    let mut total = 0;
    for text in &texts {
        total += text.chars().count() + 1;
        cumulative.push(total);
    }
    let chat_options = ChatOptions {
        max_tokens: target.model.params.max_tokens,
        temperature: Some(0.2),
        timeout: Duration::from_secs(target.model.params.timeout_s.max(5) as u64),
        retries: target.model.params.retries.max(0) as u32,
        reasoning_effort: target.model.params.reasoning_effort.clone(),
    };
    let model = target.model.model.clone();
    // 用户词表（设置 asr_hotwords）追加在 system 末尾：专有名词纠错时按词表写法
    let glossary = glossary_block(&core.settings().asr_hotwords);
    let system_prompt =
        if glossary.is_empty() { SYSTEM_PROMPT.to_string() } else { format!("{SYSTEM_PROMPT}\n\n{glossary}") };
    let semaphore = Arc::new(tokio::sync::Semaphore::new(CONCURRENCY));
    let done = AtomicUsize::new(0);
    let window_total = windows.len();

    let run_window = |indices: Vec<usize>| {
        let semaphore = semaphore.clone();
        let original: BTreeMap<usize, String> = indices.iter().map(|i| (*i, texts[*i].clone())).collect();
        let context_tail = if options.context_chars > 0 && indices[0] > 0 {
            let end = cumulative[indices[0] - 1].min(joined_chars.len());
            let start = end.saturating_sub(options.context_chars);
            joined_chars[start..end].iter().collect::<String>()
        } else {
            String::new()
        };
        let (endpoint, model, chat_options, done, on_progress, system_prompt) =
            (&endpoint, &model, &chat_options, &done, &on_progress, &system_prompt);
        async move {
            let _permit = semaphore.acquire().await.expect("信号量未关闭");
            let mut messages = vec![
                ChatMessage::new("system", system_prompt.as_str()),
                ChatMessage::new("user", build_user_prompt(&original, &context_tail)),
            ];
            let mut requests = 0;
            let mut total_tokens = 0u64;
            let mut outcome: Result<Option<BTreeMap<usize, String>>, String> = Ok(None);
            for _ in 0..MAX_STEPS {
                on_progress(done.load(Ordering::SeqCst), window_total);
                let reply = match gateway::chat(endpoint, model, &messages, chat_options).await {
                    Ok(reply) => reply,
                    Err(error) => {
                        on_progress(done.load(Ordering::SeqCst), window_total);
                        outcome = Err(error.user_message);
                        break;
                    }
                };
                on_progress(done.load(Ordering::SeqCst), window_total);
                requests += 1;
                total_tokens = total_tokens.saturating_add(reply.total_tokens.max(0) as u64);
                if reply.truncated() {
                    outcome = Ok(None);
                    break;
                }
                let feedback = match parse_llm_json(&reply.text) {
                    None => "输出不是 JSON 对象".to_string(),
                    Some(corrected) => match validate_window(&original, &corrected) {
                        Ok(fixed) => {
                            outcome = Ok(Some(fixed));
                            break;
                        }
                        Err(feedback) => feedback,
                    },
                };
                messages.push(ChatMessage::new("assistant", reply.text));
                messages.push(ChatMessage::new(
                    "user",
                    format!("校验失败：{feedback}\n请修正后只输出完整的 JSON 对象本体。"),
                ));
            }
            let finished = done.fetch_add(1, Ordering::SeqCst) + 1;
            on_progress(finished, window_total);
            (outcome, requests, total_tokens)
        }
    };
    let results = futures_util::future::join_all(windows.iter().cloned().map(run_window)).await;

    let mut corrected = BTreeMap::new();
    let mut fallback_batches = Vec::new();
    let mut fatal = Vec::new();
    let mut requests = 0;
    let mut total_tokens = 0u64;
    for (index, (outcome, count, tokens)) in results.into_iter().enumerate() {
        requests += count;
        total_tokens = total_tokens.saturating_add(tokens);
        match outcome {
            Ok(Some(fixed)) => corrected.extend(fixed),
            Ok(None) => fallback_batches.push(index),
            Err(error) => {
                fatal.push(error);
                fallback_batches.push(index);
            }
        }
    }
    if !fatal.is_empty() && fatal.len() == windows.len() {
        tracing::warn!("LLM 校对整体不可用，降级交付 ASR 原文: {}", fatal[0]);
        return Ok(ProofreadResult {
            text: original_joined,
            windows: windows.len(),
            requests,
            total_tokens,
            error: fatal.swap_remove(0),
            ..base
        });
    }
    let mut new_segments = Vec::with_capacity(expanded.len());
    let mut source_parts: Vec<Vec<String>> = vec![Vec::new(); original_texts.len()];
    for (index, segment) in expanded.into_iter().enumerate() {
        let source_index = segment.source_index;
        let text = corrected.get(&index).cloned().unwrap_or_else(|| texts[index].clone());
        let mut segment = segment.value;
        if let Some(object) = segment.as_object_mut() {
            object.insert("text".into(), json!(text));
        }
        source_parts[source_index].push(text);
        new_segments.push(segment);
    }
    let parts: Vec<String> =
        source_parts.into_iter().map(|chunks| chunks.concat()).filter(|text| !text.is_empty()).collect();
    if !fallback_batches.is_empty() {
        tracing::warn!("部分校对窗口保留原文: {fallback_batches:?}");
    }
    Ok(ProofreadResult {
        applied: true,
        text: parts.join(" "),
        segments: new_segments,
        windows: windows.len(),
        requests,
        total_tokens,
        fallback_batches,
        ..base
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::store::{ModelItemCreate, ProfileCreate};
    use crate::llm::testing::{stub_routes, StubRoute};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn stub_core(base: &str, tag: &str) -> (CoreState, PathBuf) {
        let root = std::env::temp_dir().join(format!("att-proofread-{tag}-{}", uuid::Uuid::new_v4()));
        let core = CoreState::init(&root.join("data"), crate::Platform::Macos).unwrap();
        let profile = core
            .llm
            .create_profile(ProfileCreate {
                provider: "custom".into(),
                name: "校对桩".into(),
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

    #[test]
    fn 切窗_双上限_单段超长独立成窗() {
        let options = ProofreadOptions { batch_segments: 2, batch_chars: 10, context_chars: 0 };
        let texts: Vec<String> =
            ["一二三", "四五六", "七", "超长超长超长超长超长超长"].iter().map(|s| s.to_string()).collect();
        assert_eq!(split_windows(&texts, &options), vec![vec![0, 1], vec![2], vec![3]]);
    }

    #[test]
    fn 切窗_九千字单段按句拆成多窗且时间戳覆盖原区间() {
        let text = (0..1500).map(|index| format!("第{:03}句。", index % 1000)).collect::<String>();
        assert_eq!(text.chars().count(), 9000);
        let segments = vec![json!({ "start": 10.0, "end": 910.0, "text": text, "speaker": "甲" })];
        let expanded = split_oversized_segments(&segments, 4000);
        assert!(expanded.len() > 1);
        assert_eq!(expanded[0].value["start"].as_f64(), Some(10.0));
        assert_eq!(expanded.last().unwrap().value["end"].as_f64(), Some(910.0));
        for pair in expanded.windows(2) {
            let previous_end = pair[0].value["end"].as_f64().unwrap();
            let next_start = pair[1].value["start"].as_f64().unwrap();
            assert!((previous_end - next_start).abs() < 1e-9, "相邻子段时间戳必须连续");
        }
        assert!(expanded
            .windows(2)
            .all(|pair| { pair[0].value["end"].as_f64().unwrap() <= pair[1].value["end"].as_f64().unwrap() }));
        assert!(expanded.iter().all(|segment| segment.value["speaker"] == "甲"));
        let texts =
            expanded.iter().map(|segment| segment.value["text"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        assert_eq!(texts.concat(), segments[0]["text"].as_str().unwrap());
        let windows =
            split_windows(&texts, &ProofreadOptions { batch_segments: 40, batch_chars: 4000, context_chars: 0 });
        assert!(windows.len() > 1);
    }

    #[tokio::test]
    async fn 截断输出只请求一次并保留原文() {
        let (base, mut requests) = stub_routes(vec![StubRoute::json(
            "POST /v1/chat/completions",
            200,
            r#"{"choices":[{"message":{"content":"{\"0\":\"甲乙。\"}"},"finish_reason":"length"}],"usage":{"prompt_tokens":5,"completion_tokens":4,"total_tokens":9}}"#,
        )])
        .await;
        let (core, root) = stub_core(&base, "truncated");
        let result = run(
            &core,
            None,
            ProofreadOptions { batch_segments: 40, batch_chars: 200, context_chars: 0 },
            vec![json!({ "start": 0.0, "end": 1.0, "text": "甲乙。" })],
            |_, _| {},
        )
        .await
        .unwrap();

        assert_eq!(result.requests, 1);
        assert_eq!(result.total_tokens, 9);
        assert_eq!(result.fallback_batches, vec![0]);
        assert_eq!(result.text, "甲乙。");
        assert!(requests.recv().await.is_some());
        assert!(
            tokio::time::timeout(Duration::from_millis(100), requests.recv()).await.is_err(),
            "finish_reason=length 后不能再追问"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn 多窗校对汇总_total_tokens() {
        let first = StubRoute::json(
            "POST /v1/chat/completions",
            200,
            r#"{"choices":[{"message":{"content":"{\"0\":\"甲乙丙丁。\"}"},"finish_reason":"stop"}],"usage":{"total_tokens":7}}"#,
        )
        .when_body("甲乙丙丁");
        let second = StubRoute::json(
            "POST /v1/chat/completions",
            200,
            r#"{"choices":[{"message":{"content":"{\"1\":\"戊己庚辛。\"}"},"finish_reason":"stop"}],"usage":{"total_tokens":11}}"#,
        )
        .when_body("戊己庚辛");
        let (base, _requests) = stub_routes(vec![first, second]).await;
        let (core, root) = stub_core(&base, "tokens");
        let result = run(
            &core,
            None,
            ProofreadOptions { batch_segments: 1, batch_chars: 5, context_chars: 0 },
            vec![json!({ "start": 0.0, "end": 10.0, "text": "甲乙丙丁。戊己庚辛。" })],
            |_, _| {},
        )
        .await
        .unwrap();

        assert!(result.applied);
        assert_eq!(result.windows, 2);
        assert_eq!(result.total_tokens, 18);
        assert_eq!(result.text, "甲乙丙丁。戊己庚辛。", "拆分同一原段后不能再插入空格");
        assert_eq!(result.segments.len(), 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn 相似度_与_difflib_一致() {
        assert!((similarity("abcd", "abcd") - 1.0).abs() < 1e-9);
        // difflib.SequenceMatcher(None, "abcd", "bcde").ratio() == 0.75
        assert!((similarity("abcd", "bcde") - 0.75).abs() < 1e-9);
        assert!((similarity("今天 天气  好", "今天 天气 好") - 1.0).abs() < 1e-9, "空白折叠");
        assert_eq!(similarity("", "x"), 0.0);
    }

    #[test]
    fn 验证_键集合与改动幅度() {
        let original =
            BTreeMap::from([(0, "今天天气很好我们去公园散步吧好不好呀朋友们".to_string()), (1, "嗯".to_string())]);
        let ok: Map<String, Value> =
            serde_json::from_value(json!({"0": "今天天气很好，我们去公园散步吧，好不好呀朋友们", "1": ""})).unwrap();
        assert!(validate_window(&original, &ok).is_err(), "短文本改成空串相似度为 0");
        let ok: Map<String, Value> =
            serde_json::from_value(json!({"0": "今天天气很好，我们去公园散步吧，好不好呀朋友们", "1": "嗯。"}))
                .unwrap();
        assert_eq!(validate_window(&original, &ok).unwrap()[&1], "嗯。");
        let missing: Map<String, Value> = serde_json::from_value(json!({"0": "x"})).unwrap();
        assert!(validate_window(&original, &missing).unwrap_err().contains("缺少键"));
        let rewritten: Map<String, Value> =
            serde_json::from_value(json!({"0": "完全不同的一句话内容在这里出现了", "1": "嗯"})).unwrap();
        assert!(validate_window(&original, &rewritten).unwrap_err().contains("改动过大"));
    }

    #[test]
    fn 宽容解析_llm_json() {
        assert_eq!(parse_llm_json("```json\n{\"0\": \"a\",}\n```").unwrap()["0"], "a");
        assert_eq!(parse_llm_json("好的：{\"1\": \"b\"} 完毕").unwrap()["1"], "b");
        assert!(parse_llm_json("[1,2]").is_none());
        assert!(parse_llm_json("").is_none());
    }
}
