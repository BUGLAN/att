//! 输出清洗（移植自 Python `app/llm/output_cleaning.py`）：模型不听话时的最后一道兜底。
//!
//! 三层：剥成对思考块 → 剥整段围栏 → 循环剥开头套话。每层都「宁可漏剥不可误伤」，
//! 剥完为空则回退；唯一例外是思考块层（剥完为空说明正文本来就只有思考链）。

use regex::Regex;
use std::sync::LazyLock;

/// `<think …>…</think>` 成对才剥；`<thinking>` 与未闭合的 `<think>` 不动。
static THINK_BLOCK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<think\b[^>]*>.*?</think\s*>").expect("正则合法"));

/// 只收「模型自述行为」的措辞（按长度降序匹配，长前缀优先）。
const BOILERPLATE_PREFIXES: &[&str] = &[
    "以下是结构化整理的内容",
    "以下是整理后的内容",
    "以下是整理后的文字",
    "以下是整理后的文本",
    "以下是优化后的内容",
    "以下是优化后的文字",
    "以下是优化后的文本",
    "以下是整理好的内容",
    "以下是润色后的内容",
    "根据你提供的内容",
    "根据您提供的内容",
    "结构化整理如下",
    "根据你给的内容",
    "根据您给的内容",
    "根据上述内容",
    "根据以上内容",
    "我整理如下",
    "整理如下",
    "优化如下",
    "润色如下",
];
const BOUNDARY_CHARS: &[char] = &['。', '：', ':', '；', ';', '\n'];
const MAX_BOILERPLATE_ROUNDS: usize = 4;

pub fn strip_think_blocks(text: &str) -> String {
    THINK_BLOCK.replace_all(text, "").into_owned()
}

/// 整个输出恰好被一对 ``` 包住时剥掉首尾围栏行；正文中间有围栏不动。
pub fn strip_markdown_fences(text: &str) -> String {
    let stripped = text.trim();
    if !(stripped.starts_with("```") && stripped.ends_with("```")) || stripped.matches("```").count() != 2 {
        return text.to_string();
    }
    let Some(first_newline) = stripped.find('\n') else {
        return text.to_string();
    };
    let inner = &stripped[first_newline + 1..];
    let Some(close) = inner.rfind("```") else {
        return text.to_string();
    };
    let body = inner[..close].trim_matches('\n');
    if body.is_empty() { text.to_string() } else { body.to_string() }
}

/// 循环剥开头自述套话，剥到第一个边界符（含）为止；找不到边界或剥完为空则停。
pub fn strip_leading_boilerplate(text: &str) -> String {
    let mut current = text.trim_start().to_string();
    for _ in 0..MAX_BOILERPLATE_ROUNDS {
        let Some(hit) = BOILERPLATE_PREFIXES.iter().find(|p| current.starts_with(**p)) else {
            break;
        };
        let rest = &current[hit.len()..];
        let Some(offset) = rest.find(BOUNDARY_CHARS) else {
            break;
        };
        let boundary_len = rest[offset..].chars().next().map(char::len_utf8).unwrap_or(1);
        let candidate = rest[offset + boundary_len..].trim_start();
        if candidate.is_empty() {
            break;
        }
        current = candidate.to_string();
    }
    current
}

/// 未闭合思考块的开头（成对的已先剥掉）。
static THINK_OPEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<think\b").expect("正则合法"));

/// 流式输出里已能确定的前缀：之后无论再来什么增量，`clean_output(全文)` 通常都以它开头。
///
/// 用于把优化结果边生成边打进输入框（打出去就收不回来），所以凡是收尾清洗还可能剥掉的都先扣住：
/// 未闭合的思考块、尾部疑似 `<think` 的半截标签、以反引号开头的正文（整段围栏只能收尾判定）、
/// 还没说完或还没到边界的开头套话、尾部空白。极端情况下仍可能与最终结果分叉，由调用方收尾对齐。
pub fn stable_prefix(raw: &str) -> String {
    let mut text = strip_think_blocks(raw);
    if let Some(open) = THINK_OPEN.find(&text) {
        text.truncate(open.start());
    }
    if let Some(lt) = text.rfind('<') {
        let tail = text[lt..].to_ascii_lowercase();
        if "<think".starts_with(tail.as_str()) {
            text.truncate(lt);
        }
    }
    let trimmed = text.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('`') {
        return String::new();
    }
    let stripped = strip_leading_boilerplate(trimmed);
    let undecided = BOILERPLATE_PREFIXES
        .iter()
        .any(|p| p.starts_with(stripped.as_str()) || stripped.starts_with(p));
    if undecided {
        return String::new();
    }
    stripped.trim_end().to_string()
}

/// 三层清洗；仅当输入只有思考块/空白时返回空串（由调用方按「无有效正文」处理）。
pub fn clean_output(text: &str) -> String {
    if text.trim().is_empty() {
        return String::new();
    }
    let out = strip_think_blocks(text);
    if out.trim().is_empty() {
        return String::new();
    }
    let out = strip_markdown_fences(&out);
    strip_leading_boilerplate(&out).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 三层清洗() {
        assert_eq!(clean_output("<think type=\"r\">想想</think>\n正文"), "正文");
        assert_eq!(clean_output("<think>只有思考</think>"), "");
        assert_eq!(clean_output("<thinking>保留</thinking>正文"), "<thinking>保留</thinking>正文");
        assert_eq!(clean_output("```markdown\n第一行\n第二行\n```"), "第一行\n第二行");
        assert_eq!(clean_output("前言\n```\ncode\n```"), "前言\n```\ncode\n```");
        assert_eq!(clean_output("以下是整理后的内容：\n我明天去开会。"), "我明天去开会。");
        assert_eq!(clean_output("我整理如下：整理如下：结果"), "结果");
        // 找不到边界 / 剥完为空：不剥
        assert_eq!(clean_output("整理如下"), "整理如下");
        assert_eq!(clean_output("以下是整理后的内容："), "以下是整理后的内容：");
    }

    /// 按字符逐个喂入，每一步的稳定前缀都必须是最终清洗结果的前缀，且单调增长。
    fn assert_streams_cleanly(full: &str) {
        let expected = clean_output(full);
        let mut previous = String::new();
        let mut raw = String::new();
        for ch in full.chars() {
            raw.push(ch);
            let stable = stable_prefix(&raw);
            assert!(expected.starts_with(&stable), "{full:?} 在 {raw:?} 处输出了 {stable:?}，最终是 {expected:?}");
            assert!(stable.starts_with(&previous), "{full:?} 在 {raw:?} 处回退：{previous:?} → {stable:?}");
            previous = stable;
        }
    }

    #[test]
    fn 稳定前缀_流式不会打出收尾会剥掉的内容() {
        for full in [
            "我明天去开会。",
            "以下是整理后的内容：\n我明天去开会。",
            "我整理如下：整理如下：结果",
            "<think>先想想\n再想想</think>\n正文第一句。第二句。",
            "正文<think>中途思考</think>继续",
            "```markdown\n第一行\n第二行\n```",
            "  前后空白  \n",
            "我们<thinking>保留</thinking>正文",
            "整理如下",
        ] {
            assert_streams_cleanly(full);
        }
    }

    #[test]
    fn 稳定前缀_扣住未决内容() {
        assert_eq!(stable_prefix("以下是整理"), "", "套话还没说完");
        assert_eq!(stable_prefix("以下是整理后的内容"), "", "套话还没到边界");
        assert_eq!(stable_prefix("<think>想"), "", "思考块未闭合");
        assert_eq!(stable_prefix("正文<thi"), "正文", "半截标签先扣住");
        assert_eq!(stable_prefix("```\ncode"), "", "围栏只能收尾判定");
        assert_eq!(stable_prefix("我明天 "), "我明天", "尾部空白扣住");
        assert_eq!(stable_prefix("以下是整理后的内容：我明天"), "我明天");
    }
}
