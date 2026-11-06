//! 音频工具：16k 单声道 PCM16 的 FLAC / WAV 读写与线性重采样（录音落盘 / 批量转写共用）。
//!
//! 录音落盘用 FLAC（无损，语音约为 WAV 体积的一半）；旧版本留下的 WAV 照常可读可播。

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

/// 新录音落盘的扩展名（见 [`write_recording_16k`]）。
pub const RECORDING_EXTENSION: &str = "flac";

/// 录音落盘：16k 单声道 PCM16 无损压缩为 FLAC（先写 .tmp 再 rename，异常退出不留半截文件）。
pub fn write_recording_16k(path: &Path, samples: &[i16]) -> std::io::Result<()> {
    write_bytes_atomic(path, &flac_bytes_16k(samples)?)
}

/// 编码 16k 单声道 PCM16 FLAC 到内存。
pub fn flac_bytes_16k(samples: &[i16]) -> std::io::Result<Vec<u8>> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;
    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|(_, error)| std::io::Error::other(format!("FLAC 编码参数无效: {error:?}")))?;
    let wide: Vec<i32> = samples.iter().map(|&s| i32::from(s)).collect();
    let source = flacenc::source::MemSource::from_samples(&wide, 1, 16, TARGET_RATE as usize);
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|error| std::io::Error::other(format!("FLAC 编码失败: {error:?}")))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream.write(&mut sink).map_err(|error| std::io::Error::other(format!("FLAC 写出失败: {error:?}")))?;
    Ok(sink.into_inner())
}

/// 同目录临时文件 + rename 落盘（失败时清理临时文件）。
fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    let result = std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// 按扩展名取音频的 MIME 类型（录音回放端点用；未知按 WAV 处理，与旧版一致）。
pub fn audio_content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("flac") => "audio/flac",
        _ => "audio/wav",
    }
}

/// 读录音文件为 16k 单声道 i16：`.flac` 走 FLAC 解码，其余按 WAV 读。
pub fn read_audio_16k_mono(path: &Path) -> std::io::Result<Vec<i16>> {
    if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("flac")) {
        read_flac_16k_mono(path)
    } else {
        read_wav_16k_mono(path)
    }
}

/// 读 FLAC 为 16k 单声道 i16（本应用写出的 16k 单声道 16 位直接取样本，其余混音 + 重采样）。
pub fn read_flac_16k_mono(path: &Path) -> std::io::Result<Vec<i16>> {
    let mut reader = claxon::FlacReader::open(path).map_err(std::io::Error::other)?;
    let info = reader.streaminfo();
    let samples: Vec<i32> = reader.samples().collect::<Result<_, _>>().map_err(std::io::Error::other)?;
    if info.channels == 1 && info.bits_per_sample == 16 && info.sample_rate == TARGET_RATE {
        return Ok(samples.into_iter().map(|s| s as i16).collect());
    }
    let scale = (1i64 << info.bits_per_sample.saturating_sub(1)) as f32;
    let interleaved: Vec<f32> = samples.into_iter().map(|s| s as f32 / scale).collect();
    Ok(to_16k_mono(&interleaved, info.channels as usize, info.sample_rate))
}

/// 读 WAV 为 16k 单声道 i16（多声道取均值；非 16k 线性重采样）。
pub fn read_wav_16k_mono(path: &Path) -> std::io::Result<Vec<i16>> {
    let mut reader = hound::WavReader::open(path).map_err(std::io::Error::other)?;
    let spec = reader.spec();
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(Result::ok).collect(),
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample.saturating_sub(1))) as f32;
            reader.samples::<i32>().filter_map(Result::ok).map(|s| s as f32 / scale).collect()
        }
    };
    Ok(to_16k_mono(&interleaved, spec.channels as usize, spec.sample_rate))
}

/// 交错的 [-1, 1] 浮点样本 → 16k 单声道 i16（多声道取均值；非 16k 线性重采样）。
fn to_16k_mono(interleaved: &[f32], channels: usize, sample_rate: u32) -> Vec<i16> {
    let mono: Vec<f32> = interleaved
        .chunks(channels.max(1))
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect();
    let mut resampler = LinearResampler::new(sample_rate, TARGET_RATE);
    let mut out = Vec::with_capacity(mono.len() * TARGET_RATE as usize / sample_rate.max(1) as usize + 1);
    resampler.process(&mono, &mut out);
    out.into_iter().map(f32_to_i16).collect()
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
    fn flac_往返_无损且比wav小() {
        let dir = std::env::temp_dir().join(format!("att-audio-flac-{}", std::process::id()));
        let path = dir.join(format!("a.{RECORDING_EXTENSION}"));
        // 3 秒带噪声的「语音」：正弦叠加伪随机抖动，跨多个 FLAC 块
        let mut seed = 12345u32;
        let samples: Vec<i16> = (0..48_000)
            .map(|i| {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let noise = ((seed >> 16) % 600) as f32 - 300.0;
                ((i as f32 * 0.05).sin() * 6000.0 + noise) as i16
            })
            .collect();
        write_recording_16k(&path, &samples).unwrap();
        assert!(!dir.join(format!("a.{RECORDING_EXTENSION}.tmp")).exists(), "不留临时文件");
        assert_eq!(read_audio_16k_mono(&path).unwrap(), samples, "FLAC 必须逐样本无损");
        let flac_size = std::fs::metadata(&path).unwrap().len() as usize;
        assert!(flac_size < samples.len() * 2 * 3 / 4, "FLAC {flac_size} 字节，应明显小于 WAV");
        assert_eq!(audio_content_type(&path), "audio/flac");
        assert_eq!(audio_content_type(Path::new("x.WAV")), "audio/wav");

        // 空录音也能编码（只有头）
        write_recording_16k(&dir.join("empty.flac"), &[]).unwrap();
        assert!(read_audio_16k_mono(&dir.join("empty.flac")).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wav_往返() {
        let dir = std::env::temp_dir().join(format!("att-audio-{}", std::process::id()));
        let path = dir.join("a.wav");
        let samples: Vec<i16> = (0..1600).map(|i| (i * 10) as i16).collect();
        write_wav_16k(&path, &samples).unwrap();
        assert_eq!(read_wav_16k_mono(&path).unwrap().len(), 1600);
        assert_eq!(read_audio_16k_mono(&path).unwrap().len(), 1600, "非 .flac 按 WAV 读");
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
