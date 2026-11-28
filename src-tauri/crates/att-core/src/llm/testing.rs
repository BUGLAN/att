//! 测试支撑：本地 HTTP 桩（模拟 LLM 上游），供 att-core 与 att-server 的测试共用。

use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// 桩响应：请求行（及可选的请求体片段）匹配时返回的状态码、Content-Type 与响应体。
#[derive(Clone)]
pub struct StubRoute {
    pub request_line: &'static str,
    /// 请求体必须包含该片段才命中（如 `"stream":true`），None = 不限
    pub body_contains: Option<&'static str>,
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
    /// 返回响应前等这么久：模拟上游排队 / 单块识别接近超时。
    pub delay: Duration,
    /// 非空时按 `(间隔, 原始字节)` 分块发送 SSE，模拟思考模型持续输出。
    pub stream_chunks: Vec<(Duration, String)>,
}

impl StubRoute {
    pub fn json(request_line: &'static str, status: u16, body: impl Into<String>) -> Self {
        Self {
            request_line,
            body_contains: None,
            status,
            content_type: "application/json",
            body: body.into(),
            delay: Duration::ZERO,
            stream_chunks: Vec::new(),
        }
    }

    pub fn sse(request_line: &'static str, events: &[&str]) -> Self {
        let body = events.iter().map(|e| format!("data: {e}\n\n")).collect::<String>() + "data: [DONE]\n\n";
        Self::sse_raw(request_line, body)
    }

    /// 原样返回 SSE 正文：调用方可自行决定是否带 `[DONE]` / `message_stop`。
    pub fn sse_raw(request_line: &'static str, body: impl Into<String>) -> Self {
        Self {
            request_line,
            body_contains: Some(r#""stream":true"#),
            status: 200,
            content_type: "text/event-stream",
            body: body.into(),
            delay: Duration::ZERO,
            stream_chunks: Vec::new(),
        }
    }

    /// 分块发送原始 SSE：每块前等待指定时长，便于验证长思考期间的首字看门狗。
    pub fn sse_chunks(request_line: &'static str, chunks: Vec<(Duration, String)>) -> Self {
        Self {
            request_line,
            body_contains: Some(r#""stream":true"#),
            status: 200,
            content_type: "text/event-stream",
            body: String::new(),
            delay: Duration::ZERO,
            stream_chunks: chunks,
        }
    }

    pub fn when_body(mut self, fragment: &'static str) -> Self {
        self.body_contains = Some(fragment);
        self
    }

    /// 响应前等待 `delay`（测试里用毫秒级延迟模拟慢上游）。
    pub fn after(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

/// 兼容旧写法：(请求行, 状态码, JSON 响应体)。
pub async fn stub_server(
    responses: Vec<(&'static str, u16, String)>,
) -> (String, tokio::sync::mpsc::UnboundedReceiver<String>) {
    stub_routes(responses.into_iter().map(|(line, status, body)| StubRoute::json(line, status, body)).collect()).await
}

/// 启动桩：按顺序找第一条命中的路由；把收到的请求原文回传给测试。
pub async fn stub_routes(routes: Vec<StubRoute>) -> (String, tokio::sync::mpsc::UnboundedReceiver<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("桩监听失败");
    let addr = listener.local_addr().expect("桩地址");
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { break };
            let routes = routes.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut buffer = Vec::with_capacity(64 * 1024);
                // (请求体起点, Content-Length)：DeepSeek 单块 WAV 约 3.7 MiB，缓冲区按需增长
                let mut body: Option<(usize, usize)> = None;
                loop {
                    if body.is_none() {
                        if let Some(pos) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&buffer[..pos + 4]).to_string();
                            let length = head
                                .lines()
                                .find_map(|l| {
                                    l.to_ascii_lowercase()
                                        .strip_prefix("content-length:")
                                        .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                                })
                                .unwrap_or(0);
                            body = Some((pos + 4, length));
                        }
                    }
                    if let Some((start, length)) = body {
                        if buffer.len() >= start + length {
                            break;
                        }
                    }
                    let mut chunk = [0u8; 64 * 1024];
                    let n = socket.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..n]);
                }
                let request = String::from_utf8_lossy(&buffer).to_string();
                let line = request.lines().next().unwrap_or("").to_string();
                let route = routes.iter().find(|r| {
                    line.contains(r.request_line) && r.body_contains.map(|f| request.contains(f)).unwrap_or(true)
                });
                let (status, content_type, body, delay, stream_chunks) = route
                    .map(|r| (r.status, r.content_type, r.body.clone(), r.delay, r.stream_chunks.clone()))
                    .unwrap_or((404, "application/json", "{}".into(), Duration::ZERO, Vec::new()));
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                let _ = tx.send(request);
                if !stream_chunks.is_empty() {
                    let head =
                        format!("HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nConnection: close\r\n\r\n");
                    if socket.write_all(head.as_bytes()).await.is_err() {
                        return;
                    }
                    for (delay, chunk) in stream_chunks {
                        tokio::time::sleep(delay).await;
                        if socket.write_all(chunk.as_bytes()).await.is_err() || socket.flush().await.is_err() {
                            return;
                        }
                    }
                    return;
                }
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(reply.as_bytes()).await;
            });
        }
    });
    (format!("http://{addr}"), rx)
}
