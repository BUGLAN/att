//! 本地引擎接线集成测试（默认忽略）：验证 resolve_engine → 本地推理 → payload/缓存 全链路。
//!
//! 与 `att-whisper` / `att-qwen` 里的 `real_model` 测试互补：那两个只验证引擎本身，
//! 这里验证**应用侧接线**（provider 解析、ffmpeg 解码、分段整形、缓存落盘）。
//!
//! 跑法：
//!
//! ```bash
//! ATT_TEST_WAV=/path/to/audio.wav \
//!   cargo test -p att-core --test local_engine_wiring -- --ignored --nocapture
//! ```

use att_core::state::CoreState;
use att_core::transcribe::{self, Outcome, TranscribeRequest};
use att_core::Platform;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn repo_models() -> PathBuf {
    if let Ok(dir) = std::env::var("ATT_MODELS_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("..").join("models");
    std::env::set_var("ATT_MODELS_DIR", &dir);
    dir
}

fn prepare(name: &str, provider: &str) -> Arc<CoreState> {
    repo_models();
    let dir = std::env::temp_dir().join(format!("att-wiring-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let core = CoreState::init(&dir, Platform::Windows).expect("初始化核心状态");
    core.apply_settings_update(&json!({ "asr_provider": provider })).expect("写入设置");
    Arc::new(core)
}

fn request(path: String, engine: &str) -> TranscribeRequest {
    TranscribeRequest {
        path,
        language: "zh".into(),
        want_segments: true,
        engine: engine.into(),
        proofread: false,
        proofread_model_id: None,
        proofread_options: None,
    }
}

async fn run(name: &str, provider: &str) {
    let wav = std::env::var("ATT_TEST_WAV").expect("请用 ATT_TEST_WAV 指定测试音频（wav/mp3/mp4 均可）");
    let core = prepare(name, provider);
    let outcome = transcribe::handle(core.clone(), request(wav.clone(), provider))
        .await
        .unwrap_or_else(|error| panic!("转写应成功，实际 {}：{}", error.status, error.message));
    let payload = match outcome {
        Outcome::Result(payload) => payload,
        Outcome::Job(job_id) => panic!("短文件不应走异步任务：{job_id}"),
    };
    println!("payload = {}", serde_json::to_string_pretty(&payload).unwrap());
    assert!(!payload["text"].as_str().unwrap_or("").trim().is_empty(), "转写文本为空");
    let segments = payload["segments"].as_array().expect("应返回分段");
    assert!(!segments.is_empty(), "分段为空");
    let engine = payload["engine"].as_str().unwrap_or("");
    assert!(engine.starts_with(provider), "engine 字段应为 {provider}:<模型>，实际 {engine}");
    assert!(core.data_dir.join("transcribe_file_cache.json").is_file(), "结果应落缓存");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要本地 faster-whisper 模型、ffmpeg 与测试音频"]
async fn whisper_文件转写全链路() {
    run("whisper", "whisper").await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要 sherpa-onnx 推理库、Qwen3-ASR ONNX 模型、ffmpeg 与测试音频"]
async fn qwen_文件转写全链路() {
    run("qwen", "qwen_asr").await;
}
