//! 豆包连通性探针：用数据目录里的设置（密文密钥）把一段 WAV 送去识别。
//!
//! 用法：`cargo run -p att-core --example doubao_probe -- <data_dir> <wav> [模式] [endpoint]`
//! - 模式省略：批量识别；
//! - `stream`：按实时节奏（100ms/包）推流，模拟录音路径，打印建连与首字耗时（距「按下」）；
//! - `warm:<秒>`：先握好手（预热连接：不发全量请求，期间按应用里的间隔发心跳）、空闲这么多秒后再「按下」
//!   发请求并推流，打印首字耗时，与 `stream` 对比预热能省多少；
//! - `idle`：握手并发出全量请求后不发音频，等服务端断开，打印空闲多久被断开与 close code
//!   （实测 8.4 秒「等包超时」——预热连接因此不能提前发全量请求）；
//! - `idle-open`：只握手、不发全量请求，按应用里的间隔发心跳，看连接能备多久（预热连接的保活方式）。
//!   `idle` / `idle-open` 不读 wav，第二个参数随便填。
//!
//! 第四个参数覆盖接入点，便于对比 `bigmodel` 与 `bigmodel_async`；
//! 环境变量 `DOUBAO_END_WINDOW_SIZE` 覆盖「强制断句静音时长」（ms），便于观察停顿后何时确认。

use att_core::stt::{doubao, doubao_config};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// idle 模式最多等这么久（服务端一直不断开时也要结束）；`idle-open` 可用环境变量 `DOUBAO_IDLE_LIMIT`（秒）覆盖。
const IDLE_PROBE_LIMIT: Duration = Duration::from_secs(180);

/// 预热连接空闲 `idle` 这么久：按应用里的间隔发心跳，返回期间连接是否一直没断。
async fn keep_idle(session: &mut doubao::DoubaoSession, idle: Duration) -> bool {
    let until = Instant::now() + idle;
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return session.check_alive().is_ok();
        }
        tokio::select! {
            _ = tokio::time::sleep(left.min(doubao::WARM_PING_INTERVAL)) => {
                if session.idle_for() >= doubao::WARM_STALE || session.ping().await.is_err() {
                    return false;
                }
            }
            _ = session.wait_closed() => return false,
        }
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data_dir = PathBuf::from(args.get(1).expect("缺少 data_dir 参数"));
    let wav = PathBuf::from(args.get(2).expect("缺少 wav 参数"));
    let mode = args.get(3).cloned().unwrap_or_default();

    let core = att_core::state::CoreState::init(&data_dir, att_core::Platform::current()).expect("初始化失败");
    let mut config = doubao_config(&core);
    if let Some(endpoint) = args.get(4) {
        config.endpoint = endpoint.clone();
    }
    if let Some(window) = std::env::var("DOUBAO_END_WINDOW_SIZE").ok().and_then(|v| v.parse().ok()) {
        config.end_window_size = window;
    }
    println!("配置: {config:?}");

    if mode == "idle" {
        let started = Instant::now();
        let session = doubao::DoubaoSession::connect(&config, "zh", "", None).await.expect("建连失败");
        println!("建连耗时 {:?}，logid={}；开始空闲等待（不发音频）", started.elapsed(), session.log_id);
        let opened = Instant::now();
        match tokio::time::timeout(IDLE_PROBE_LIMIT, session.wait_closed()).await {
            Ok(()) => println!(
                "空闲 {:.1}s 后连接被关闭：close code={:?}，check_alive={:?}",
                opened.elapsed().as_secs_f64(),
                session.close_code(),
                session.check_alive().err().map(|e| e.to_string())
            ),
            Err(_) => println!("空闲 {}s 内服务端没有断开", IDLE_PROBE_LIMIT.as_secs()),
        }
        session.cancel().await;
        return;
    }

    if mode == "idle-open" {
        let limit = std::env::var("DOUBAO_IDLE_LIMIT")
            .ok()
            .and_then(|v| v.parse().ok())
            .map(Duration::from_secs)
            .unwrap_or(IDLE_PROBE_LIMIT);
        let started = Instant::now();
        let mut session = doubao::DoubaoSession::open(&config).await.expect("握手失败");
        println!("握手耗时 {:?}，logid={}；开始空闲等待（不发全量请求，只发心跳）", started.elapsed(), session.log_id);
        let opened = Instant::now();
        let alive = keep_idle(&mut session, limit).await;
        println!(
            "空闲 {:.1}s：{}（最近一次来帧在 {:.1}s 前，close code={:?}）",
            opened.elapsed().as_secs_f64(),
            if alive { "连接仍在" } else { "连接已断" },
            session.idle_for().as_secs_f64(),
            session.close_code(),
        );
        session.cancel().await;
        return;
    }

    let samples = att_core::audio::read_audio_16k_mono(&wav).expect("读取音频失败");
    println!("音频: {} 采样 ≈ {:.2}s", samples.len(), samples.len() as f64 / 16000.0);

    let warm_secs = mode.strip_prefix("warm:").and_then(|v| v.parse::<f64>().ok());
    let realtime = mode == "stream" || warm_secs.is_some();
    // 预热：先握好手（不发全量请求），空闲指定秒数后才算「按下」
    let warm = match warm_secs {
        Some(secs) => {
            let mut session = doubao::DoubaoSession::open(&config).await.expect("预热握手失败");
            println!("预热连接已握好手，空闲 {secs}s 后按下");
            let alive = keep_idle(&mut session, Duration::from_secs_f64(secs)).await;
            alive.then_some(session).or_else(|| {
                println!("预热连接空闲期间已断开");
                None
            })
        }
        None => None,
    };

    let started = Instant::now();
    let result = if realtime {
        let first_text: Arc<OnceLock<Duration>> = Arc::new(OnceLock::new());
        let mark = first_text.clone();
        let on_partial: doubao::PartialCallback = Arc::new(move |partial: doubao::Partial| {
            if !partial.text.trim().is_empty() && mark.set(started.elapsed()).is_ok() {
                println!("  首字耗时 {:?}", started.elapsed());
            }
            println!(
                "  [{:>5}ms] partial（末尾 {} 字未确认）: {}",
                started.elapsed().as_millis(),
                partial.pending_chars,
                partial.text
            );
        });
        let mut session = match warm {
            Some(mut session) => {
                session.begin(&config, "zh", "", Some(on_partial)).await.expect("发送识别请求失败");
                println!("复用预热连接（按下到可推流 {:?}）", started.elapsed());
                session
            }
            None => {
                let session =
                    doubao::DoubaoSession::connect(&config, "zh", "", Some(on_partial)).await.expect("建连失败");
                println!("建连耗时 {:?}，logid={}", started.elapsed(), session.log_id);
                session
            }
        };
        for chunk in samples.chunks(1600) {
            session.push(chunk).await.expect("推流失败");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        session.finish(doubao::FINISH_TIMEOUT, doubao::FINISH_QUIET).await
    } else {
        doubao::transcribe_pcm(&config, "zh", "", &samples).await
    };
    match result {
        Ok(outcome) => {
            println!("结果（{:?}，计费 {}ms）: {}", started.elapsed(), outcome.audio_duration_ms, outcome.text)
        }
        Err(error) => println!("失败: {error}"),
    }
}
