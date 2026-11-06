//! 原生麦克风采集（通用采集路径）。
//!
//! ```text
//! [cpal 回调(设备率, N 声道)] → mono 混音 → rtrb SPSC 环(≈2s)
//!         │ （回调内：零分配、零锁）
//!         ▼
//! [采集线程] 每 20ms 读环 → 线性重采样 16k → i16 → on_chunk(采样, 电平)
//! ```
//!
//! cpal 的 Stream 在部分平台不是 Send，因此由专用线程创建并持有，
//! 停止时该线程先把环里剩余样本全部冲刷给回调再退出（不截断尾部）。

use att_core::audio::{f32_to_i16, LinearResampler, TARGET_RATE};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// 采集分块回调：16k 单声道 i16 样本 + 本块电平（0..1，与前端 calculateAudioLevel 同公式）。
pub type ChunkCallback = Box<dyn FnMut(&[i16], f32) + Send + 'static>;

/// 采集句柄：drop 或 [`CaptureHandle::stop`] 结束采集（阻塞到尾部冲刷完成）。
pub struct CaptureHandle {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// 设备中途不可用（拔出麦克风/耳机）的原因；置位后采集线程自行结束并关闭输出
    device_error: Arc<Mutex<Option<String>>>,
    pub device_name: String,
    pub device_rate: u32,
}

impl CaptureHandle {
    pub fn stop(mut self) {
        self.shutdown();
    }

    /// 采集途中设备失效的原因（正常运行/正常停止为 None）。
    pub fn device_error(&self) -> Option<String> {
        self.device_error.lock().ok().and_then(|e| e.clone())
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 列出输入设备名（设置页麦克风下拉框）。
pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    match host.input_devices() {
        Ok(devices) => devices.filter_map(|d| d.name().ok()).collect(),
        Err(error) => {
            tracing::warn!("枚举输入设备失败: {error}");
            Vec::new()
        }
    }
}

fn pick_device(host: &cpal::Host, preferred: Option<&str>) -> Result<cpal::Device, String> {
    if let Some(name) = preferred.map(str::trim).filter(|n| !n.is_empty() && *n != "default") {
        if let Ok(mut devices) = host.input_devices() {
            if let Some(device) = devices.find(|d| d.name().map(|n| n == name).unwrap_or(false)) {
                return Ok(device);
            }
        }
        tracing::warn!("未找到麦克风「{name}」，改用系统默认输入设备");
    }
    host.default_input_device()
        .ok_or_else(|| "未检测到可用的麦克风，请检查系统输入设备".to_string())
}

/// 启动采集。设备打开失败（无设备/无权限/格式不支持）同步返回错误。
pub fn start_capture(device: Option<String>, on_chunk: ChunkCallback) -> Result<CaptureHandle, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let device_error = Arc::new(Mutex::new(None));
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(String, u32), String>>();
    let thread_stop = stop.clone();
    let thread_error = device_error.clone();
    let thread = std::thread::Builder::new()
        .name("att-audio-capture".into())
        .spawn(move || capture_thread(device, on_chunk, thread_stop, thread_error, ready_tx))
        .map_err(|e| format!("无法启动采集线程: {e}"))?;

    match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok((device_name, device_rate))) => Ok(CaptureHandle { stop, thread: Some(thread), device_error, device_name, device_rate }),
        Ok(Err(message)) => {
            let _ = thread.join();
            Err(message)
        }
        Err(_) => {
            stop.store(true, Ordering::SeqCst);
            Err("打开麦克风超时（5 秒），请检查系统麦克风权限".into())
        }
    }
}

fn capture_thread(
    device: Option<String>,
    mut on_chunk: ChunkCallback,
    stop: Arc<AtomicBool>,
    device_error: Arc<Mutex<Option<String>>>,
    ready_tx: mpsc::Sender<Result<(String, u32), String>>,
) {
    let host = cpal::default_host();
    let device = match pick_device(&host, device.as_deref()) {
        Ok(device) => device,
        Err(message) => {
            let _ = ready_tx.send(Err(message));
            return;
        }
    };
    let device_name = device.name().unwrap_or_else(|_| "未知设备".into());
    let config = match device.default_input_config() {
        Ok(config) => config,
        Err(error) => {
            let _ = ready_tx.send(Err(format!("读取麦克风格式失败（{error}），请检查系统麦克风权限")));
            return;
        }
    };
    let sample_rate = config.sample_rate().0;
    let channels = config.channels().max(1) as usize;
    // 环容量 ≈2s 设备原生率单声道（回调永不阻塞；理论溢出视为处理线程死亡）
    let (producer, mut consumer) = rtrb::RingBuffer::<f32>::new((sample_rate as usize * 2).next_power_of_two());

    let stream = match build_stream(&device, &config, channels, producer, device_error.clone()) {
        Ok(stream) => stream,
        Err(message) => {
            let _ = ready_tx.send(Err(message));
            return;
        }
    };
    if let Err(error) = stream.play() {
        let _ = ready_tx.send(Err(format!("启动麦克风失败（{error}），请检查系统麦克风权限")));
        return;
    }
    tracing::info!("麦克风已打开: {device_name}（{sample_rate}Hz × {channels}ch）");
    let _ = ready_tx.send(Ok((device_name, sample_rate)));

    let mut resampler = LinearResampler::new(sample_rate, TARGET_RATE);
    let mut raw = Vec::with_capacity(sample_rate as usize / 10);
    let mut resampled = Vec::with_capacity(TARGET_RATE as usize / 10);
    let mut pcm = Vec::with_capacity(TARGET_RATE as usize / 10);
    let mut drain = |consumer: &mut rtrb::Consumer<f32>, on_chunk: &mut ChunkCallback| {
        raw.clear();
        while let Ok(sample) = consumer.pop() {
            raw.push(sample);
        }
        if raw.is_empty() {
            return;
        }
        resampled.clear();
        resampler.process(&raw, &mut resampled);
        if resampled.is_empty() {
            return;
        }
        let sum: f32 = resampled.iter().map(|s| s * s).sum();
        let level = ((sum / resampled.len() as f32).sqrt() * 6.0).clamp(0.0, 1.0);
        pcm.clear();
        pcm.extend(resampled.iter().copied().map(f32_to_i16));
        on_chunk(&pcm, level);
    };

    let device_lost = || device_error.lock().map(|e| e.is_some()).unwrap_or(false);
    while !stop.load(Ordering::SeqCst) && !device_lost() {
        std::thread::sleep(Duration::from_millis(20));
        drain(&mut consumer, &mut on_chunk);
    }
    // 停止顺序：先停设备，再冲刷环里的尾部样本（不截断）。
    // 设备失效时同样走这里：线程结束 → on_chunk 被释放 → 下游音频通道关闭，录音按正常收尾保存已录内容
    drop(stream);
    drain(&mut consumer, &mut on_chunk);
    if let Some(error) = device_error.lock().ok().and_then(|e| e.clone()) {
        tracing::warn!("麦克风采集中断: {error}");
    }
}

fn build_stream(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    channels: usize,
    mut producer: rtrb::Producer<f32>,
    device_error: Arc<Mutex<Option<String>>>,
) -> Result<cpal::Stream, String> {
    let stream_config: cpal::StreamConfig = config.clone().into();
    // 设备被拔出等致命错误：记下原因，由采集线程据此结束（否则录音会一直「收」空数据且无任何提示）
    let on_error = move |error: cpal::StreamError| {
        tracing::error!("麦克风流错误: {error}");
        if matches!(error, cpal::StreamError::DeviceNotAvailable) {
            if let Ok(mut slot) = device_error.lock() {
                slot.get_or_insert_with(|| "麦克风已断开或不可用".to_string());
            }
        }
    };
    macro_rules! build {
        ($sample:ty, $to_f32:expr) => {
            device.build_input_stream(
                &stream_config,
                move |data: &[$sample], _: &cpal::InputCallbackInfo| {
                    for frame in data.chunks(channels) {
                        let sum: f32 = frame.iter().map(|s| $to_f32(*s)).sum();
                        // 环满时丢弃（不可达设计：处理线程 20ms 一读，环容量 2s）
                        let _ = producer.push(sum / frame.len() as f32);
                    }
                },
                on_error,
                None,
            )
        };
    }
    let result = match config.sample_format() {
        cpal::SampleFormat::F32 => build!(f32, |s: f32| s),
        cpal::SampleFormat::I16 => build!(i16, |s: i16| s as f32 / 32768.0),
        cpal::SampleFormat::I32 => build!(i32, |s: i32| s as f32 / 2_147_483_648.0),
        cpal::SampleFormat::U16 => build!(u16, |s: u16| (s as f32 - 32768.0) / 32768.0),
        other => return Err(format!("不支持的麦克风采样格式: {other:?}")),
    };
    result.map_err(|e| format!("打开麦克风失败（{e}），请检查系统麦克风权限"))
}
