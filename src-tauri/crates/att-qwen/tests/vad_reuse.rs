//! 复现 Silero VAD 句柄跨调用复用的问题（默认忽略，需要 sherpa-onnx 推理库与模型）。
//!
//! 跑法：
//! ```bash
//! cd src-tauri
//! cargo test -p att-qwen --test vad_reuse -- --ignored --nocapture
//! ```
//!
//! 预期（bug 存在时）：第一次 `speech_ranges` 返回非空区间；对同一段音频第二次调用返回空，
//! 因为 sherpa-onnx 的 SpeechSegment.start 是「探测器生命周期内的绝对采样位置」，
//! 没有 Reset 就继续累加，第二次的起点已经超过本次音频长度，被 vad.rs 的过滤逻辑丢弃。

use att_qwen::{QwenConfig, SileroVad, SAMPLE_RATE};
use std::path::{Path, PathBuf};

fn models_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("..").join("models")
}

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
    let mono: Vec<f32> =
        samples.chunks(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32).collect();
    if spec.sample_rate as usize == SAMPLE_RATE {
        return mono.iter().map(|v| (v * 32767.0).clamp(-32768.0, 32767.0) as i16).collect();
    }
    let ratio = spec.sample_rate as f64 / SAMPLE_RATE as f64;
    let target_len = (mono.len() as f64 / ratio) as usize;
    (0..target_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let low = pos.floor() as usize;
            let high = (low + 1).min(mono.len().saturating_sub(1));
            let w = (pos - low as f64) as f32;
            let v = mono[low.min(mono.len() - 1)] * (1.0 - w) + mono[high] * w;
            (v * 32767.0).clamp(-32768.0, 32767.0) as i16
        })
        .collect()
}

#[test]
#[ignore = "需要 sherpa-onnx 推理库与 silero_vad.onnx"]
fn vad_句柄复用_第二次应返回与第一次一致() {
    let models = models_dir();
    let wav = std::env::var("ATT_TEST_WAV")
        .map(PathBuf::from)
        .unwrap_or_else(|_| models.join("qwen3-asr-0.6B-int8").join("test_wavs").join("qiqiu1.wav"));
    assert!(wav.is_file(), "缺少测试音频：{}", wav.display());
    let samples = read_pcm16k(&wav);
    assert!(!samples.is_empty());
    println!("音频 {} 采样点（{:.2}s）", samples.len(), samples.len() as f64 / SAMPLE_RATE as f64);

    let config = QwenConfig::new("qwen3-asr-0.6B-int8", &models);
    let vad = SileroVad::load(&config).expect("加载 Silero VAD");

    let mut results = Vec::new();
    for round in 1..=3 {
        let ranges = vad.speech_ranges(&samples).expect("VAD 推理");
        println!("第 {round} 次 speech_ranges：{} 个区间 -> {:?}", ranges.len(), ranges);
        results.push(ranges);
    }

    assert!(!results[0].is_empty(), "第一次应有语音区间");
    assert_eq!(
        results[0], results[1],
        "同一段音频第二次的语音区间应与第一次一致（不一致说明 VAD 句柄未重置）"
    );
    assert_eq!(
        results[0], results[2],
        "同一段音频第三次的语音区间应与第一次一致（不一致说明 VAD 句柄未重置）"
    );
}
