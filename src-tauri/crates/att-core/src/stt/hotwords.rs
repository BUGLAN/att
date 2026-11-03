//! 热词（个人词表）：归一化与 token 预算。
//!
//! 词表存在设置 `asr_hotwords`，与引擎无关；目前只有豆包能用（`corpus.context` 直传热词），
//! 其余引擎保留词表但不发送。前端 `asrEngineStatus.mjs` 有同一套估算规则，改动时两边一起改。

/// 词表最多保留多少个词（超出的丢弃，防止设置文件被意外灌爆）
pub const MAX_WORDS: usize = 200;
/// 单个热词最多多少个字符：再长就不是「词」了，按字符截断
pub const MAX_WORD_CHARS: usize = 32;

/// 归一化：去首尾空白、内部空白合并为一个空格、截断超长词、按大小写不敏感去重（保留先出现的）。
pub fn normalize<I, S>(words: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for word in words {
        let collapsed = word.as_ref().split_whitespace().collect::<Vec<_>>().join(" ");
        let word: String = collapsed.chars().take(MAX_WORD_CHARS).collect();
        let word = word.trim_end().to_string();
        if word.is_empty() || !seen.insert(word.to_lowercase()) {
            continue;
        }
        out.push(word);
        if out.len() >= MAX_WORDS {
            break;
        }
    }
    out
}

/// 估算一个热词占多少 token（服务端分词器不公开，按偏保守的经验值）：
/// 非 ASCII 字符（汉字等）各算 1；连续的英文字母 / 数字每 4 个算 1（不足 4 个也算 1）；
/// 空格与 ASCII 标点不计。至少算 1。
pub fn estimate_tokens(word: &str) -> usize {
    let mut tokens = 0;
    let mut ascii_run = 0usize;
    for c in word.chars() {
        if c.is_ascii_alphanumeric() {
            ascii_run += 1;
            continue;
        }
        tokens += ascii_run.div_ceil(4);
        ascii_run = 0;
        if !c.is_ascii() && !c.is_whitespace() {
            tokens += 1;
        }
    }
    (tokens + ascii_run.div_ceil(4)).max(1)
}

/// 按顺序取预算内的前缀：靠前的词优先，第一个放不下的词及其后面的都不发送。
pub fn within_budget(words: &[String], budget: usize) -> &[String] {
    let mut used = 0;
    for (index, word) in words.iter().enumerate() {
        used += estimate_tokens(word);
        if used > budget {
            return &words[..index];
        }
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 归一化_去空白_去重_截断() {
        let words = normalize(["  Tauri ", "", "tauri", "落地页", "Option  +  V", &"x".repeat(40)]);
        assert_eq!(words, vec!["Tauri".to_string(), "落地页".into(), "Option + V".into(), "x".repeat(MAX_WORD_CHARS)]);
    }

    #[test]
    fn 归一化_数量上限() {
        let many: Vec<String> = (0..MAX_WORDS + 10).map(|i| format!("w{i}")).collect();
        assert_eq!(normalize(&many).len(), MAX_WORDS);
    }

    #[test]
    fn token估算_汉字每字一个_英文每四字母一个() {
        assert_eq!(estimate_tokens("落地页"), 3);
        assert_eq!(estimate_tokens("Rust"), 1);
        assert_eq!(estimate_tokens("WebView"), 2);
        assert_eq!(estimate_tokens("Option + V"), 3);
        assert_eq!(estimate_tokens("CTranslate2"), 3);
        assert_eq!(estimate_tokens("Tauri 应用"), 4);
        assert_eq!(estimate_tokens("+"), 1);
    }

    #[test]
    fn 预算_按顺序截断() {
        let words: Vec<String> = ["落地页", "Rust", "WebView"].iter().map(|s| s.to_string()).collect();
        assert_eq!(within_budget(&words, 100).len(), 3);
        assert_eq!(within_budget(&words, 4), &words[..2]);
        assert_eq!(within_budget(&words, 2).len(), 0);
    }
}
