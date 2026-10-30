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
}
