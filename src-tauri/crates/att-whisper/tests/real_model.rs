//! 真实模型集成测试（默认忽略，避免 CI 里加载 3GB 模型）。
//!
//! 跑法（模型目录与测试音频都放好后）：
//!
//! ```bash
//! ATT_TEST_WAV=/path/to/16k.wav \
//!   cargo test -p att-whisper --test real_model -- --ignored --nocapture
//! ```
//!
//! 音频不是 16k 单声道也能跑（测试里做了平均声道 + 线性重采样，只为验证链路，不追求音质）。

use att_whisper::{LocalWhisper, WhisperConfig, SAMPLE_RATE};
use std::path::{Path, PathBuf};

/// 仓库根下的 `models/`（可用 ATT_MODELS_DIR 覆盖）。
fn models_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("ATT_MODELS_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("..").join("models")
}

/// 读 WAV → 16k 单声道 i16。
fn read_pcm16k(path: &Path) -> Vec<i16> {
    let mut reader = hound::WavReader::open(path).expect("打开测试音频");
    let spec = reader.spec();
    let channels = spec.channels.max(1) as usize;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().map(|s| s.unwrap() as f32 * scale).collect()
        }
        hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap()).collect(),
    };
    // 多声道取均值
    let mono: Vec<f32> = samples.chunks(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32).collect();
    // 线性重采样到 16k
    if spec.sample_rate as usize == SAMPLE_RATE {
        return mono.iter().map(|value| (value * 32767.0).clamp(-32768.0, 32767.0) as i16).collect();
    }
    let ratio = spec.sample_rate as f64 / SAMPLE_RATE as f64;
    let target_len = (mono.len() as f64 / ratio) as usize;
    (0..target_len)
        .map(|index| {
            let position = index as f64 * ratio;
            let low = position.floor() as usize;
            let high = (low + 1).min(mono.len().saturating_sub(1));
            let weight = (position - low as f64) as f32;
            let value = mono[low.min(mono.len() - 1)] * (1.0 - weight) + mono[high] * weight;
            (value * 32767.0).clamp(-32768.0, 32767.0) as i16
        })
        .collect()
}

#[test]
#[ignore = "需要本机 CTranslate2 Whisper 模型与测试音频"]
fn 真实音频_整段转写与分段时间戳() {
    let wav = std::env::var("ATT_TEST_WAV").expect("请用 ATT_TEST_WAV 指定测试音频");
    let models = models_dir();
    assert!(models.is_dir(), "模型根目录不存在：{}", models.display());
    let config = WhisperConfig::new("large-v3", "auto", &models);
    let engine = LocalWhisper::load(&config).expect("加载本地 Whisper");
    println!("设备：{}，模型：{}", engine.device(), engine.model_dir().display());

    let samples = read_pcm16k(Path::new(&wav));
    assert!(!samples.is_empty(), "测试音频为空");
    let outcome = engine.transcribe("zh", &samples).expect("转写失败");
    println!(
        "耗时 {:.2}s / 音频 {:.2}s / 分段 {}：{}",
        outcome.elapsed,
        outcome.duration,
        outcome.segments.len(),
        outcome.text
    );
    assert!(!outcome.text.trim().is_empty(), "转写结果为空");
    let mut previous = 0.0f64;
    for segment in &outcome.segments {
        assert!(segment.end >= segment.start, "分段结束早于开始：{segment:?}");
        assert!(segment.start >= previous - 0.01, "分段时间戳应单调递增：{segment:?}");
        previous = segment.start;
    }
}
