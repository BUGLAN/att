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

/// 人声估计的分析帧：20ms 一帧。
const VOICE_FRAME: usize = 320;
/// 逐帧功率按 100ms（5 帧）滑动平均后再判定：压住底噪逐帧的随机起伏，又不至于抹掉一个音节。
const VOICE_WINDOW_FRAMES: usize = 5;
/// 低于这个电平当作没有信号（采集刚起来的全零帧），不参与估计底噪。
const DIGITAL_SILENCE_DB: f64 = -90.0;
/// 底噪取有信号帧电平的第 10 百分位：按下后开口前、字与字之间的停顿、松开缓冲都落在这一档。
const NOISE_FLOOR_PERCENTILE: f64 = 0.10;
/// 底噪估计的上限：一整段几乎不停顿的说话会把第 10 百分位抬到语音电平上；
/// 真实环境的底噪极少高过 -30 dBFS（实测用户机器 -37 左右），封顶后这类录音照样数得出语音。
const NOISE_FLOOR_CAP_DB: f64 = -30.0;
/// 「清楚的声音」：高出底噪 10dB。底噪自身的起伏（滑动平均后）几乎碰不到这一档。
const CLEAR_MARGIN_DB: f64 = 10.0;
/// 「隐约的声音」：高出底噪 6dB。嘈杂环境里小声说话也过得了这一档，但偶发的杂声同样会过，所以要求更长。
const FAINT_MARGIN_DB: f64 = 6.0;
/// 清楚的声音累计这么久就算听到人声（一个短词）。
const CLEAR_MIN_SECONDS: f64 = 0.5;
/// 隐约的声音累计这么久也算（小声说了一两句）。
const FAINT_MIN_SECONDS: f64 = 1.2;
/// 短录音里的单字：清楚的声音至少这么久，且占整段的比例不低于 [`SHORT_CLEAR_MIN_RATIO`]。
const SHORT_CLEAR_MIN_SECONDS: f64 = 0.2;
const SHORT_CLEAR_MIN_RATIO: f64 = 0.05;

/// 一段录音里人声的粗略估计（见 [`speech_activity`]）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeechActivity {
    /// 录音总时长（秒）
    pub total_seconds: f64,
    /// 估计的底噪电平（dBFS）；整段没有信号时为 -120
    pub noise_floor_db: f64,
    /// 高出底噪 10dB 的累计时长（秒）
    pub clear_seconds: f64,
    /// 高出底噪 6dB 的累计时长（秒，含 clear 部分）
    pub faint_seconds: f64,
}

impl SpeechActivity {
    /// 是否听到了人声。判定刻意偏保守（宁可把噪声放过去，也不把小声说话判成没声音），满足任一条就算听到：
    /// - 清楚的声音累计 ≥ 0.5s（正常音量说一个短词）；
    /// - 隐约的声音累计 ≥ 1.2s（嘈杂环境里小声说话，只高出底噪 6dB 左右）；
    /// - 清楚的声音 ≥ 0.2s 且占整段 ≥ 5%（短录音里说一个字）。
    ///
    /// 判成没听到的典型情况：按住十几秒没说话，期间只有一下不到半秒的咳嗽 / 哼声 / 碰麦。
    /// 代价是「按住很久、只说一个字」也会被判成没听到（录音和识别结果仍会保存）。
    pub fn heard_voice(&self) -> bool {
        self.clear_seconds >= CLEAR_MIN_SECONDS
            || self.faint_seconds >= FAINT_MIN_SECONDS
            || (self.clear_seconds >= SHORT_CLEAR_MIN_SECONDS && self.clear_seconds >= self.total_seconds * SHORT_CLEAR_MIN_RATIO)
    }
}

fn power_db(power: f64) -> f64 {
    if power > 0.0 { 10.0 * power.log10() } else { -120.0 }
}

/// 按短帧能量估计一段 16k 录音里有多少「像人声」的声音：底噪自适应（取安静帧的电平），
/// 再数高出底噪一定分贝的时长。只看能量，不区分人声与其他声音（敲键盘也算），所以只用来兜「整段没人说话」。
pub fn speech_activity(samples: &[i16]) -> SpeechActivity {
    let total_seconds = samples.len() as f64 / TARGET_RATE as f64;
    let powers: Vec<f64> = samples
        .chunks_exact(VOICE_FRAME)
        .map(|frame| frame.iter().map(|&s| (s as f64 / 32768.0).powi(2)).sum::<f64>() / VOICE_FRAME as f64)
        .collect();
    // 100ms 滑动平均（当前帧及之前 4 帧；开头不足 5 帧按实有帧数平均）
    let mut levels = Vec::with_capacity(powers.len());
    let mut window = 0.0;
    for (index, power) in powers.iter().enumerate() {
        window += power;
        if index >= VOICE_WINDOW_FRAMES {
            window -= powers[index - VOICE_WINDOW_FRAMES];
        }
        let count = (index + 1).min(VOICE_WINDOW_FRAMES);
        levels.push(power_db(window.max(0.0) / count as f64));
    }
    let mut audible: Vec<f64> = levels.iter().copied().filter(|db| *db > DIGITAL_SILENCE_DB).collect();
    if audible.is_empty() {
        return SpeechActivity { total_seconds, noise_floor_db: -120.0, clear_seconds: 0.0, faint_seconds: 0.0 };
    }
    audible.sort_by(f64::total_cmp);
    let floor = audible[(audible.len() as f64 * NOISE_FLOOR_PERCENTILE) as usize].min(NOISE_FLOOR_CAP_DB);
    let frame_seconds = VOICE_FRAME as f64 / TARGET_RATE as f64;
    let seconds_above = |margin: f64| levels.iter().filter(|db| **db > floor + margin).count() as f64 * frame_seconds;
    SpeechActivity {
        total_seconds,
        noise_floor_db: floor,
        clear_seconds: seconds_above(CLEAR_MARGIN_DB),
        faint_seconds: seconds_above(FAINT_MARGIN_DB),
    }
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

    /// 确定性伪随机数（LCG），取值 [-1, 1)。
    struct Lcg(u32);

    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (self.0 >> 8) as f64 / (1u32 << 23) as f64 - 1.0
        }
    }

    fn amplitude(db: f64) -> f64 {
        10f64.powf(db / 20.0) * 32768.0
    }

    /// 白噪声底噪：RMS 约 `db` dBFS，每 100ms 电平随机起伏 ±3dB（真实底噪并不平稳）。
    fn noise(seconds: f64, db: f64, seed: u32) -> Vec<f64> {
        let mut rng = Lcg(seed);
        let mut gain = 1.0;
        (0..(seconds * 16_000.0) as usize)
            .map(|i| {
                if i % 1600 == 0 {
                    gain = 10f64.powf(rng.next() * 3.0 / 20.0);
                }
                // 均匀分布的 RMS 是幅度的 1/√3
                rng.next() * 3f64.sqrt() * amplitude(db) * gain
            })
            .collect()
    }

    /// 类人声：140Hz 基频带两个谐波，响的部分 RMS 约 `db` dBFS。
    /// `syllables`：按音节开合（每 250ms 响 180ms，首尾 20ms 渐变）；否则一直响。
    fn voice(seconds: f64, db: f64, syllables: bool) -> Vec<f64> {
        // sin + 0.5·sin2 + 0.25·sin3 的 RMS
        let rms = ((1.0 + 0.25 + 0.0625) / 2.0f64).sqrt();
        let scale = amplitude(db) / rms;
        (0..(seconds * 16_000.0) as usize)
            .map(|i| {
                let t = i as f64 / 16_000.0;
                let w = 2.0 * std::f64::consts::PI * 140.0 * t;
                let tone = w.sin() + 0.5 * (2.0 * w).sin() + 0.25 * (3.0 * w).sin();
                let envelope = if syllables {
                    let p = t % 0.25;
                    if p >= 0.18 { 0.0 } else { (p / 0.02).min((0.18 - p) / 0.02).min(1.0) }
                } else {
                    1.0
                };
                tone * scale * envelope
            })
            .collect()
    }

    fn overlay(base: &mut [f64], at_seconds: f64, sound: &[f64]) {
        let offset = (at_seconds * 16_000.0) as usize;
        for (target, value) in base[offset..].iter_mut().zip(sound) {
            *target += value;
        }
    }

    fn activity(signal: &[f64]) -> SpeechActivity {
        let pcm: Vec<i16> = signal.iter().map(|v| v.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16).collect();
        speech_activity(&pcm)
    }

    #[test]
    fn 人声估计_长时间底噪里一下短促的声音_不算听到() {
        // 仿实测误识别样本：按住 16s，底噪约 -36 dBFS，中间只有一下 0.3s、-20 dBFS 的哼声
        let mut held = noise(16.0, -36.0, 7);
        overlay(&mut held, 4.0, &voice(0.3, -20.0, false));
        let result = activity(&held);
        assert!(!result.heard_voice(), "{result:?}");
        assert!((result.noise_floor_db + 38.0).abs() < 3.0, "底噪估计: {result:?}");

        // 同样一下放在 2 秒的短录音里：当成说了一个字
        let mut short = noise(2.0, -36.0, 7);
        overlay(&mut short, 0.8, &voice(0.3, -20.0, false));
        assert!(activity(&short).heard_voice(), "{:?}", activity(&short));
    }

    #[test]
    fn 人声估计_正常说话与小声说话都算听到() {
        let cases: [(&str, Vec<f64>); 5] = [
            ("正常音量说 3 秒", {
                let mut s = noise(5.0, -36.0, 11);
                overlay(&mut s, 0.5, &voice(3.0, -22.0, true));
                s
            }),
            ("嘈杂环境小声说话：只比底噪高 6dB", {
                let mut s = noise(5.0, -36.0, 13);
                overlay(&mut s, 0.5, &voice(3.0, -30.0, true));
                s
            }),
            ("安静房间小声说话", {
                let mut s = noise(4.0, -60.0, 17);
                overlay(&mut s, 0.5, &voice(2.0, -45.0, true));
                s
            }),
            ("一句 0.6 秒的短话", {
                let mut s = noise(3.0, -36.0, 19);
                overlay(&mut s, 1.0, &voice(0.6, -24.0, true));
                s
            }),
            ("整段几乎不停顿（底噪估计被语音抬高，靠封顶兜住）", {
                let mut s = voice(3.0, -20.0, true);
                overlay(&mut s, 0.0, &noise(3.0, -60.0, 23));
                s
            }),
        ];
        for (name, signal) in cases {
            let result = activity(&signal);
            assert!(result.heard_voice(), "{name}: {result:?}");
        }
    }

    #[test]
    fn 人声估计_纯底噪与无信号不算听到() {
        for (db, seed) in [(-36.0, 29), (-60.0, 31)] {
            let result = activity(&noise(10.0, db, seed));
            assert!(!result.heard_voice(), "{db} dBFS 底噪: {result:?}");
        }
        let silent = speech_activity(&[0; 32_000]);
        assert!(!silent.heard_voice());
        assert_eq!(silent.noise_floor_db, -120.0);
        let empty = speech_activity(&[]);
        assert!(!empty.heard_voice());
        assert_eq!(empty.total_seconds, 0.0);
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
