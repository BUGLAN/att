//! 豆包连通性探针：用数据目录里的设置（密文密钥）把一段 WAV 送去识别。
//!
//! 用法：`cargo run -p att-core --example doubao_probe -- <data_dir> <wav> [stream]`
//! 第三个参数为 `stream` 时按实时节奏（200ms/包）推流，模拟录音路径。

use att_core::stt::{doubao, doubao_config};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data_dir = PathBuf::from(args.get(1).expect("缺少 data_dir 参数"));
    let wav = PathBuf::from(args.get(2).expect("缺少 wav 参数"));
    let realtime = args.get(3).map(|s| s == "stream").unwrap_or(false);

    let core = att_core::state::CoreState::init(&data_dir, att_core::Platform::current()).expect("初始化失败");
    let config = doubao_config(&core);
    println!("配置: {config:?}");
    let samples = att_core::audio::read_wav_16k_mono(&wav).expect("读取 WAV 失败");
    println!("音频: {} 采样 ≈ {:.2}s", samples.len(), samples.len() as f64 / 16000.0);

    let started = Instant::now();
    let result = if realtime {
        let on_partial: doubao::PartialCallback = Arc::new(|text| println!("  partial: {text}"));
        let mut session = doubao::DoubaoSession::connect(&config, "zh", "", Some(on_partial))
            .await
            .expect("建连失败");
        println!("建连耗时 {:?}，logid={}", started.elapsed(), session.log_id);
        for chunk in samples.chunks(3200) {
            session.push(chunk).await.expect("推流失败");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        session.finish(doubao::FINISH_TIMEOUT, doubao::FINISH_QUIET).await
    } else {
        doubao::transcribe_pcm(&config, "zh", "", &samples).await
    };
    match result {
        Ok(outcome) => println!("结果（{:?}，计费 {}ms）: {}", started.elapsed(), outcome.audio_duration_ms, outcome.text),
        Err(error) => println!("失败: {error}"),
    }
}
