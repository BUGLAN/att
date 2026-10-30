//! 自适应超时（移植自 Python `app/llm/timeouts.py`）：按输入长度把模型级定值超时拆成三个阶段预算。
//!
//! - 生成总预算：`max(配置值, 20 + 0.03 × 字数)`，封顶 600s（配置值是下界）；
//! - 首 token 预算（仅流式）：`30 + 0.05 × 字数`，不超过总预算；
//! - 块间空闲预算（仅流式）：固定 30s。

use std::time::Duration;

const MAX_TIMEOUT_S: f64 = 600.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestTimeouts {
    pub request: Duration,
    pub first_token: Duration,
    pub chunk_idle: Duration,
}

pub fn derive(base_timeout_s: i64, input_chars: usize) -> RequestTimeouts {
    let chars = input_chars as f64;
    let request_s = (base_timeout_s as f64).max(20.0 + 0.03 * chars).ceil().min(MAX_TIMEOUT_S);
    let first_token_s = (30.0 + 0.05 * chars).ceil().min(request_s);
    let chunk_idle_s = 30.0_f64.min(request_s);
    RequestTimeouts {
        request: Duration::from_secs(request_s as u64),
        first_token: Duration::from_secs(first_token_s as u64),
        chunk_idle: Duration::from_secs(chunk_idle_s as u64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 按字数放大_配置值为下界_封顶600() {
        let short = derive(60, 100);
        assert_eq!(short.request.as_secs(), 60);
        assert_eq!(short.first_token.as_secs(), 35);
        assert_eq!(short.chunk_idle.as_secs(), 30);
        let long = derive(60, 5000);
        assert_eq!(long.request.as_secs(), 170);
        assert_eq!(long.first_token.as_secs(), 170, "首 token 不超过总预算");
        assert_eq!(derive(60, 100_000).request.as_secs(), 600);
        assert_eq!(derive(5, 0).chunk_idle.as_secs(), 20);
    }
}
