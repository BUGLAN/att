//! 音频工具：16k 单声道 PCM16 的 WAV 读写与线性重采样（录音落盘 / 批量转写共用）。

use std::path::Path;

pub const TARGET_RATE: u32 = 16_000;

/// 写 16k 单声道 PCM16 WAV（先写 .tmp 再 rename，异常退出不留半截文件）。
pub fn write_wav_16k(path: &Path, samples: &[i16]) -> std::io::Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("wav.tmp");
    {
        let mut writer = hound::WavWriter::create(&tmp, spec).map_err(std::io::Error::other)?;
        for sample in samples {
            writer.write_sample(*sample).map_err(std::io::Error::other)?;
        }
        writer.finalize().map_err(std::io::Error::other)?;
    }
    std::fs::rename(&tmp, path)
}

/// 编码 16k 单声道 PCM16 WAV 到内存（上传类引擎用，不落临时文件）。
pub fn wav_bytes_16k(samples: &[i16]) -> std::io::Result<Vec<u8>> {
    let spec = hound::WavSpec { channels: 1, sample_rate: TARGET_RATE, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut cursor = std::io::Cursor::new(Vec::with_capacity(samples.len() * 2 + 44));
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec).map_err(std::io::Error::other)?;
        for sample in samples {
            writer.write_sample(*sample).map_err(std::io::Error::other)?;
        }
        writer.finalize().map_err(std::io::Error::other)?;
    }
    Ok(cursor.into_inner())
}

/// 读 WAV 为 16k 单声道 i16（多声道取均值；非 16k 线性重采样）。
pub fn read_wav_16k_mono(path: &Path) -> std::io::Result<Vec<i16>> {
    let mut reader = hound::WavReader::open(path).map_err(std::io::Error::other)?;
    let spec = reader.spec();
    let channels = spec.channels.max(1) as usize;
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(Result::ok).collect(),
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample.saturating_sub(1))) as f32;
            reader.samples::<i32>().filter_map(Result::ok).map(|s| s as f32 / scale).collect()
        }
    };
    let mono: Vec<f32> = interleaved
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect();
    let mut resampler = LinearResampler::new(spec.sample_rate, TARGET_RATE);
    let mut out = Vec::with_capacity(mono.len() * TARGET_RATE as usize / spec.sample_rate.max(1) as usize + 1);
    resampler.process(&mono, &mut out);
    Ok(out.into_iter().map(f32_to_i16).collect())
}

pub fn f32_to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

/// 流式线性重采样器（跨块保持相位，语音识别场景足够）。
pub struct LinearResampler {
    step: f64,
    /// 下一个输出样本在「上一块末样本 + 当前块」坐标系中的位置
    position: f64,
    last: Option<f32>,
    passthrough: bool,
}

impl LinearResampler {
    pub fn new(from_rate: u32, to_rate: u32) -> Self {
        let from_rate = from_rate.max(1);
        Self {
            step: from_rate as f64 / to_rate.max(1) as f64,
            position: 0.0,
            last: None,
            passthrough: from_rate == to_rate,
        }
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.passthrough {
            out.extend_from_slice(input);
            return;
        }
        if input.is_empty() {
            return;
        }
        // 坐标系：索引 0 = 上一块末样本（首块时为当前块首样本）
        let offset = if self.last.is_some() { 1usize } else { 0 };
        let sample_at = |index: usize| -> f32 {
            if offset == 1 && index == 0 {
                self.last.unwrap_or(0.0)
            } else {
                input[index - offset]
            }
        };
        let available = input.len() + offset;
        while self.position + 1.0 < available as f64 {
            let base = self.position.floor() as usize;
            let frac = (self.position - base as f64) as f32;
            let a = sample_at(base);
            let b = sample_at(base + 1);
            out.push(a + (b - a) * frac);
            self.position += self.step;
        }
        // 平移坐标系：新块的索引 0 是本块末样本
        self.position -= (available - 1) as f64;
        self.last = input.last().copied();
    }
}

/// 一块 PCM 的 RMS 电平（dBFS）；全零返回 -120。
pub fn chunk_db(chunk: &[i16]) -> f64 {
    if chunk.is_empty() {
        return -120.0;
    }
    let sum: f64 = chunk.iter().map(|s| (*s as f64 / 32768.0).powi(2)).sum();
    let rms = (sum / chunk.len() as f64).sqrt();
    if rms > 0.0 { 20.0 * rms.log10() } else { -120.0 }
}

/// 多段录音的静音切段器（移植自前端 `recorder/segmentPolicy.mjs` 的 2.2s / 10s / 0.8s 策略）。
///
/// - 每块按 RMS 电平（dBFS）判定有无语音：高于 `silence_level_db` 即为语音；
/// - 本段有语音、时长 ≥ 最短 2.2s、且连续静音 ≥ `silence_threshold` → 在静音处切段；
/// - 时长到最长 10s 强制切段（避免一句话拖成超大分片）；
/// - 整段没有语音的分片直接丢弃（不送识别、不计费）。
pub struct Segmenter {
    silence_level_db: f64,
    silence_samples: usize,
    current: Vec<i16>,
    current_has_speech: bool,
    silence_run: usize,
    any_speech: bool,
}

pub const MIN_SEGMENT_SECONDS: f64 = 2.2;
pub const MAX_SEGMENT_SECONDS: f64 = 10.0;
pub const FORCE_MIN_SEGMENT_SECONDS: f64 = 0.8;

impl Segmenter {
    pub fn new(silence_level_db: f64, silence_threshold_seconds: f64) -> Self {
        Self {
            silence_level_db,
            silence_samples: (silence_threshold_seconds.max(0.2) * TARGET_RATE as f64) as usize,
            current: Vec::new(),
            current_has_speech: false,
            silence_run: 0,
            any_speech: false,
        }
    }

    fn seconds(samples: usize) -> f64 {
        samples as f64 / TARGET_RATE as f64
    }

    /// 喂一块 16k PCM；切出完整分段时返回。
    pub fn push(&mut self, chunk: &[i16]) -> Option<Vec<i16>> {
        if chunk.is_empty() {
            return None;
        }
        if chunk_db(chunk) > self.silence_level_db {
            self.current_has_speech = true;
            self.any_speech = true;
            self.silence_run = 0;
        } else {
            self.silence_run += chunk.len();
        }
        self.current.extend_from_slice(chunk);
        let duration = Self::seconds(self.current.len());
        let pause = self.current_has_speech && self.silence_run >= self.silence_samples && duration >= MIN_SEGMENT_SECONDS;
        if pause || duration >= MAX_SEGMENT_SECONDS {
            return self.take(false);
        }
        None
    }

    /// 收尾：交出最后一段（强制切分时 0.8s 以上即可；整场没说过话则丢弃）。
    pub fn finish(&mut self) -> Option<Vec<i16>> {
        self.take(true)
    }

    fn take(&mut self, force: bool) -> Option<Vec<i16>> {
        let segment = std::mem::take(&mut self.current);
        let has_speech = std::mem::replace(&mut self.current_has_speech, false);
        self.silence_run = 0;
        let duration = Self::seconds(segment.len());
        if !has_speech {
            return None;
        }
        if force && duration < FORCE_MIN_SEGMENT_SECONDS && !self.any_speech {
            return None;
        }
        Some(segment)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 重采样_48k到16k_样本数与分块无关() {
        let input: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.001).sin()).collect();
        let mut whole = Vec::new();
        LinearResampler::new(48_000, 16_000).process(&input, &mut whole);
        let mut chunked = Vec::new();
        let mut resampler = LinearResampler::new(48_000, 16_000);
        for chunk in input.chunks(441) {
            resampler.process(chunk, &mut chunked);
        }
        assert!((whole.len() as i64 - 16_000).abs() <= 1, "样本数 {}", whole.len());
        assert_eq!(whole.len(), chunked.len());
        for (a, b) in whole.iter().zip(&chunked) {
            assert!((a - b).abs() < 1e-4);
        }
    }

    #[test]
    fn wav_往返() {
        let dir = std::env::temp_dir().join(format!("att-audio-{}", std::process::id()));
        let path = dir.join("a.wav");
        let samples: Vec<i16> = (0..1600).map(|i| (i * 10) as i16).collect();
        write_wav_16k(&path, &samples).unwrap();
        assert_eq!(read_wav_16k_mono(&path).unwrap().len(), 1600);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 多段切分_静音处切段_超长强制切_无语音丢弃() {
        let speech = vec![8000i16; 1600]; // 100ms，约 -12 dBFS
        let silence = vec![0i16; 1600];
        let mut segmenter = Segmenter::new(-40.0, 0.8);
        let mut cuts = Vec::new();
        // 3s 语音 + 1s 静音 → 静音达阈值且时长 ≥ 2.2s，在静音处切段
        for _ in 0..30 {
            assert!(segmenter.push(&speech).is_none());
        }
        for _ in 0..10 {
            if let Some(segment) = segmenter.push(&silence) {
                cuts.push(segment);
            }
        }
        assert_eq!(cuts.len(), 1);
        // 纯静音 5s：不切也不交付
        for _ in 0..50 {
            assert!(segmenter.push(&silence).is_none());
        }
        // 连续语音 10s → 强制切
        let mut forced = 0;
        for _ in 0..100 {
            if segmenter.push(&speech).is_some() {
                forced += 1;
            }
        }
        assert_eq!(forced, 1, "前面 5s 静音 + 10s 语音在 10s 处强制切一次");
        segmenter.push(&speech);
        assert!(segmenter.finish().is_some(), "收尾交出最后一段");
        assert!(segmenter.finish().is_none());
    }

    #[test]
    fn 电平_满幅约0dB_静音为下限() {
        assert_eq!(chunk_db(&[]), -120.0);
        assert_eq!(chunk_db(&[0; 160]), -120.0);
        assert!(chunk_db(&[i16::MAX; 160]).abs() < 0.01);
        // 约 -40dBFS 的方波：恰好落在默认静音阈值附近
        let quiet = chunk_db(&[328; 160]);
        assert!((quiet + 40.0).abs() < 0.1, "{quiet}");
    }
}
