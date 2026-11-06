//! 数据面真实网络冒烟：起一个真 listener，用裸 TCP 访问 /api/health，
//! 并校验 CORS 允许回环来源——这是早期实机「全站请求失败」事故的回归锁。

use att_core::records::Record;
use att_core::state::CoreState;
use att_server::{router, self_check_health, serve, ServerState};
use axum::body::Body;
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt;

fn temp_state(tag: &str) -> Arc<ServerState> {
    let dir = std::env::temp_dir().join(format!("att-serve-smoke-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let core = CoreState::init(&dir, att_core::Platform::Windows).unwrap();
    core.records
        .insert(Record {
            id: "rec-smoke".into(),
            raw_text: Some("冒烟".into()),
            ..Default::default()
        })
        .unwrap();
    Arc::new(ServerState { core: Arc::new(core) })
}

/// 起真实服务 → 裸 TCP 自检 → 路由可访问。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn 真监听自检_health_可访问() {
    let state = temp_state("tcp");
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let port = serve(state, rx, &[0]).await.expect("服务必须能绑定");
    assert!(port > 0);

    let status_line = self_check_health(port).expect("裸 TCP 必须能连上自检");
    assert!(
        status_line.contains("200"),
        "自检应返回 200，实际: {status_line}"
    );
}

/// 回归锁：数据面必须**持续在线**（只要停机信号发送端仍存活）。
///
/// 早期实机事故：app 侧把 shutdown 发送端当局部变量 drop，watch 通道关闭被
/// `with_graceful_shutdown` 当作停机信号，数据面在启动后几秒内静默死亡。
/// 此测试让服务跨过一个真实的时间片后再探活，锁死「服务不会自行退出」。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn 发送端存活期间_数据面持续在线() {
    let state = temp_state("alive");
    let (tx, rx) = tokio::sync::watch::channel(false);
    let port = serve(state, rx, &[0]).await.expect("服务必须能绑定");

    // 模拟 setup 返回、前端加载页面后的真实时间流逝
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let status_line = self_check_health(port).expect("发送端存活时数据面必须仍在线");
    assert!(status_line.contains("200"), "300ms 后 health 仍应 200，实际: {status_line}");

    // 显式停机信号后端口应释放（语义自查）
    tx.send(true).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        self_check_health(port).is_err(),
        "发送 true 停机信号后端口应关闭"
    );
}

/// CORS：dev 页面来源（127.0.0.1:任意端口）必须被放行，
/// 否则浏览器会拦掉全部请求（早期实机四类 toast 的根因之一）。
#[tokio::test]
async fn cors_放行回环来源_拒绝外部来源() {
    let state = temp_state("cors");
    let app = router(state);

    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/health")
                .header("Origin", "http://127.0.0.1:8090")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers().get("access-control-allow-origin").unwrap(),
        "http://127.0.0.1:8090",
        "回环来源必须拿到 ACAO 头"
    );
    let _ = response.into_body().collect().await;

    let blocked = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/health")
                .header("Origin", "https://evil.example.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        blocked.headers().get("access-control-allow-origin").is_none(),
        "外部来源不得下发 ACAO"
    );
}
