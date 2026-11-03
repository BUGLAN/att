//! ASR 文本 → LLM 滑动窗口校对（移植自 Python `app/services/transcribe_proofread.py`）。
//!
//! - 窗口双上限：每窗 ≤ batch_segments 段且 ≤ batch_chars 字，只切在段边界；可带前文尾部作参考；
//! - 每窗「LLM → 验证 → 反馈 → 重试」最多 [`MAX_STEPS`] 次：键集合完全相等 + 逐条相似度不越阈；
//! - 验证耗尽的窗口保留原文（计入 fallback_batches）；全部窗口都因 LLM 错误失败 → 整体降级不算失败；
//! - 窗口并发 [`CONCURRENCY`]，temperature=0.2。

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
    pub fallback_batches: Vec<usize>,
    pub error: String,
    pub reason: String,
}

impl ProofreadResult {
    /// 响应 proofread 字段（不含正文）。
    pub fn meta(&self) -> Value {
        let mut meta = json!({
            "applied": self.applied,
            "llm_model": if self.model_label.is_empty() { self.model_id.clone().unwrap_or_else(|| "default".into()) } else { self.model_label.clone() },
            "windows": self.windows,
            "requests": self.requests,
            "fallback_batches": self.fallback_batches,
            "prompt_version": PROMPT_VERSION,
        });
        if !self.error.is_empty() {
            meta["error"] = json!(self.error);
        }
        if !self.reason.is_empty() {
            meta["reason"] = json!(self.reason);
        }
        meta
    }
}

/// 段索引切窗：双上限，只切在段边界；单段超过字数上限时独立成窗。
pub fn split_windows(texts: &[String], options: &ProofreadOptions) -> Vec<Vec<usize>> {
    let mut windows = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut current_chars = 0;
    for (index, text) in texts.iter().enumerate() {
        let length = text.chars().count();
        if !current.is_empty() && (current.len() >= options.batch_segments || current_chars + length > options.batch_chars) {
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
pub fn validate_window(original: &BTreeMap<usize, String>, corrected: &Map<String, Value>) -> Result<BTreeMap<usize, String>, String> {
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
        let threshold = if clean_for_similarity(raw).len() <= SHORT_TEXT_CHARS { SIMILARITY_SHORT } else { SIMILARITY_DEFAULT };
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

/// 对 ASR segments（`[{start,end,text}]`，时间戳原样保留）执行滑窗校对。
pub async fn run(
    core: &CoreState,
    model_id: Option<&str>,
    options: ProofreadOptions,
    segments: Vec<Value>,
    on_progress: impl Fn(usize, usize) + Send + Sync,
) -> Result<ProofreadResult, LlmError> {
    let target = core.llm.resolve_runtime(model_id)?;
    let spec = registry::get(&target.profile.provider)
        .ok_or_else(|| LlmError::plain(CODE_UNSUPPORTED_PROVIDER, format!("未知服务商：{}", target.profile.provider)))?;
    let endpoint = Endpoint::resolve(spec, target.profile.api_base.as_deref(), &target.api_key, &target.profile.extra_headers)?;
    let label = if target.model.display_name.is_empty() { target.model.model.clone() } else { target.model.display_name.clone() };
    let texts: Vec<String> = segments.iter().map(|s| s.get("text").and_then(Value::as_str).unwrap_or("").to_string()).collect();
    let joined: String = texts.iter().filter(|t| !t.is_empty()).cloned().collect::<Vec<_>>().join(" ");
    let base = ProofreadResult {
        applied: false,
        text: String::new(),
        segments: segments.clone(),
        model_id: model_id.map(str::to_string),
        model_label: label,
        windows: 0,
        requests: 0,
        fallback_batches: Vec::new(),
        error: String::new(),
        reason: String::new(),
    };
    if joined.trim().is_empty() {
        return Ok(ProofreadResult { reason: "asr_empty".into(), ..base });
    }
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
    let system_prompt = if glossary.is_empty() { SYSTEM_PROMPT.to_string() } else { format!("{SYSTEM_PROMPT}\n\n{glossary}") };
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
            let mut messages =
                vec![ChatMessage::new("system", system_prompt.as_str()), ChatMessage::new("user", build_user_prompt(&original, &context_tail))];
            let mut requests = 0;
            let mut outcome: Result<Option<BTreeMap<usize, String>>, String> = Ok(None);
            for _ in 0..MAX_STEPS {
                let reply = match gateway::chat(endpoint, model, &messages, chat_options).await {
                    Ok(reply) => reply,
                    Err(error) => {
                        outcome = Err(error.user_message);
                        break;
                    }
                };
                requests += 1;
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
                messages.push(ChatMessage::new("user", format!("校验失败：{feedback}\n请修正后只输出完整的 JSON 对象本体。")));
            }
            let finished = done.fetch_add(1, Ordering::SeqCst) + 1;
            on_progress(finished, window_total);
            (outcome, requests)
        }
    };
    let results = futures_util::future::join_all(windows.iter().cloned().map(run_window)).await;

    let mut corrected = BTreeMap::new();
    let mut fallback_batches = Vec::new();
    let mut fatal = Vec::new();
    let mut requests = 0;
    for (index, (outcome, count)) in results.into_iter().enumerate() {
        requests += count;
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
        return Ok(ProofreadResult { text: joined, windows: windows.len(), requests, error: fatal.swap_remove(0), ..base });
    }
    let mut new_segments = Vec::with_capacity(segments.len());
    let mut parts = Vec::new();
    for (index, segment) in segments.into_iter().enumerate() {
        let text = corrected.get(&index).cloned().unwrap_or_else(|| texts[index].clone());
        let mut segment = segment;
        if let Some(object) = segment.as_object_mut() {
            object.insert("text".into(), json!(text));
        }
        if !text.is_empty() {
            parts.push(text);
        }
        new_segments.push(segment);
    }
    if !fallback_batches.is_empty() {
        tracing::warn!("部分校对窗口保留原文: {fallback_batches:?}");
    }
    Ok(ProofreadResult {
        applied: true,
        text: parts.join(" "),
        segments: new_segments,
        windows: windows.len(),
        requests,
        fallback_batches,
        ..base
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 切窗_双上限_单段超长独立成窗() {
        let options = ProofreadOptions { batch_segments: 2, batch_chars: 10, context_chars: 0 };
        let texts: Vec<String> = ["一二三", "四五六", "七", "超长超长超长超长超长超长"].iter().map(|s| s.to_string()).collect();
        assert_eq!(split_windows(&texts, &options), vec![vec![0, 1], vec![2], vec![3]]);
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
        let original = BTreeMap::from([(0, "今天天气很好我们去公园散步吧好不好呀朋友们".to_string()), (1, "嗯".to_string())]);
        let ok: Map<String, Value> = serde_json::from_value(json!({"0": "今天天气很好，我们去公园散步吧，好不好呀朋友们", "1": ""})).unwrap();
        assert!(validate_window(&original, &ok).is_err(), "短文本改成空串相似度为 0");
        let ok: Map<String, Value> = serde_json::from_value(json!({"0": "今天天气很好，我们去公园散步吧，好不好呀朋友们", "1": "嗯。"})).unwrap();
        assert_eq!(validate_window(&original, &ok).unwrap()[&1], "嗯。");
        let missing: Map<String, Value> = serde_json::from_value(json!({"0": "x"})).unwrap();
        assert!(validate_window(&original, &missing).unwrap_err().contains("缺少键"));
        let rewritten: Map<String, Value> = serde_json::from_value(json!({"0": "完全不同的一句话内容在这里出现了", "1": "嗯"})).unwrap();
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
