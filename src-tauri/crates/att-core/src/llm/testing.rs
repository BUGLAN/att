//! 测试支撑：本地 HTTP 桩（模拟 LLM 上游），供 att-core 与 att-server 的测试共用。

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
}

impl StubRoute {
    pub fn json(request_line: &'static str, status: u16, body: impl Into<String>) -> Self {
        Self { request_line, body_contains: None, status, content_type: "application/json", body: body.into() }
    }

    pub fn sse(request_line: &'static str, events: &[&str]) -> Self {
        let body = events.iter().map(|e| format!("data: {e}\n\n")).collect::<String>() + "data: [DONE]\n\n";
        Self { request_line, body_contains: Some(r#""stream":true"#), status: 200, content_type: "text/event-stream", body }
    }

    pub fn when_body(mut self, fragment: &'static str) -> Self {
        self.body_contains = Some(fragment);
        self
    }
}

/// 兼容旧写法：(请求行, 状态码, JSON 响应体)。
pub async fn stub_server(responses: Vec<(&'static str, u16, String)>) -> (String, tokio::sync::mpsc::UnboundedReceiver<String>) {
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
                let mut buffer = vec![0u8; 1 << 20];
                let mut read = 0;
                // 读到请求头结束 + Content-Length 指定的请求体
                loop {
                    let n = socket.read(&mut buffer[read..]).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    read += n;
                    let text = String::from_utf8_lossy(&buffer[..read]).to_string();
                    if let Some(header_end) = text.find("\r\n\r\n") {
                        let length = text
                            .lines()
                            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                            .unwrap_or(0);
                        if read >= header_end + 4 + length {
                            break;
                        }
                    }
                }
                let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                let line = request.lines().next().unwrap_or("").to_string();
                let route = routes
                    .iter()
                    .find(|r| line.contains(r.request_line) && r.body_contains.map(|f| request.contains(f)).unwrap_or(true));
                let (status, content_type, body) = route
                    .map(|r| (r.status, r.content_type, r.body.clone()))
                    .unwrap_or((404, "application/json", "{}".into()));
                let _ = tx.send(request);
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
