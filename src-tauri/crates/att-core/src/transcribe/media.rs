//! 媒体探测与解码：ffprobe 取时长、ffmpeg 解码为 16kHz 单声道 i16 PCM（经 stdout 管道，不落临时文件）。
//!
//! 与 Python 版一样依赖系统 ffmpeg；macOS 从 Finder/Dock 启动的应用不继承 shell 的 PATH，
//! 因此除 PATH 外还查 Homebrew / MacPorts 常见位置，并支持 `ATT_FFMPEG_DIR` 显式指定。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;

pub const VIDEO_EXTENSIONS: &[&str] = &["mp4", "flv", "webm", "mkv", "mov", "avi", "m4v", "ts"];
pub const AUDIO_EXTENSIONS: &[&str] = &["wav", "mp3", "m4a", "flac", "ogg", "opus", "aac", "wma", "amr"];
/// 单次文件转写最多解码两小时；ffprobe 估小或没有时长时，解码端仍按这个上限截断。
pub const MAX_DURATION_SECONDS: u64 = 7200;
const DECODED_SAMPLE_RATE: usize = 16_000;

/// 输入协议只允许本地文件，阻止 concat / HLS 等播放列表间接读取其他资源。
const INPUT_PROTOCOL_OPTIONS: [&str; 2] = ["-protocol_whitelist", "file"];
/// 只允许界面扩展名对应的常见音视频解复用器。
const INPUT_FORMAT_OPTIONS: [&str; 2] =
    ["-format_whitelist", "aac,amr,asf,avi,flac,flv,matroska,mj2,mov,mp3,mp4,m4a,mpegts,ogg,wav,webm,3gp,3g2"];

#[derive(Debug)]
pub enum MediaError {
    /// 系统没有 ffmpeg/ffprobe
    ToolMissing(&'static str),
    /// 文件损坏 / 没有音轨 / 不是音视频
    Undecodable(String),
}

impl std::fmt::Display for MediaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MediaError::ToolMissing(tool) => write!(
                f,
                "未找到 {tool}：文件转写需要系统安装 ffmpeg（macOS：brew install ffmpeg；Windows：安装后把 bin 目录加入 PATH）"
            ),
            MediaError::Undecodable(message) => f.write_str(message),
        }
    }
}

pub fn is_supported_extension(path: &Path) -> bool {
    let extension = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    VIDEO_EXTENSIONS.contains(&extension.as_str()) || AUDIO_EXTENSIONS.contains(&extension.as_str())
}

/// 查找 ffmpeg 工具：`ATT_FFMPEG_DIR` > PATH > 常见安装位置。
pub fn find_tool(name: &'static str) -> Result<PathBuf, MediaError> {
    let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(dir) = std::env::var("ATT_FFMPEG_DIR") {
        dirs.push(PathBuf::from(dir));
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    for extra in ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin", "/usr/bin"] {
        dirs.push(PathBuf::from(extra));
    }
    dirs.into_iter()
        .map(|dir| dir.join(&file))
        .find(|candidate| candidate.is_file())
        .ok_or(MediaError::ToolMissing(name))
}

fn command(tool: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(tool);
    command.stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    {
        // 隐藏 ffmpeg 子进程的控制台窗口（CREATE_NO_WINDOW）
        command.creation_flags(0x0800_0000);
    }
    command
}

/// ffprobe 探测时长（秒）。文件可读但没有可靠 duration 时返回 `None`，由调用方转后台任务。
pub async fn probe_duration(path: &Path) -> Result<Option<f64>, MediaError> {
    reject_active_playlist(path)?;
    let ffprobe = find_tool("ffprobe")?;
    let output = command(&ffprobe)
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "json"])
        .args(INPUT_PROTOCOL_OPTIONS)
        .args(INPUT_FORMAT_OPTIONS)
        .arg(path)
        .output()
        .await
        .map_err(|e| MediaError::Undecodable(format!("无法运行 ffprobe：{e}")))?;
    if !output.status.success() {
        return Err(MediaError::Undecodable("无法解码：文件损坏或不是有效的音视频文件".into()));
    }
    parse_probe_duration(&output.stdout)
}

/// `format_whitelist` 已禁止 concat / HLS 解复用器；这里再检查文件头，让明显伪装的播放列表
/// 在旧版 ffmpeg 或缺少工具时也稳定返回 415，而不是进入协议探测。
fn reject_active_playlist(path: &Path) -> Result<(), MediaError> {
    let mut file = std::fs::File::open(path)
        .map_err(|error| MediaError::Undecodable(format!("无法解码：读取文件失败（{error}）")))?;
    let mut head = vec![0u8; 64 * 1024];
    let read =
        file.read(&mut head).map_err(|error| MediaError::Undecodable(format!("无法解码：读取文件失败（{error}）")))?;
    let text = String::from_utf8_lossy(&head[..read]);
    let text = text.trim_start_matches('\u{feff}').trim_start();
    if text.starts_with("#EXTM3U") || text.starts_with("ffconcat version 1.0") {
        return Err(MediaError::Undecodable("无法解码：不支持播放列表或 concat 输入".into()));
    }
    Ok(())
}

fn parse_probe_duration(json: &[u8]) -> Result<Option<f64>, MediaError> {
    let value: serde_json::Value = serde_json::from_slice(json)
        .map_err(|error| MediaError::Undecodable(format!("无法解码：读取媒体时长失败（{error}）")))?;
    Ok(value
        .pointer("/format/duration")
        .and_then(|v| v.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| v.as_f64()))
        .filter(|duration| duration.is_finite() && *duration > 0.0))
}

/// 解码为 16kHz 单声道 i16 PCM（视频容器自动抽音轨）。
pub async fn decode_pcm16k(path: &Path) -> Result<Vec<i16>, MediaError> {
    let ffmpeg = find_tool("ffmpeg")?;
    let max_duration = MAX_DURATION_SECONDS.to_string();
    let output = command(&ffmpeg)
        .args(["-v", "error"])
        .args(INPUT_PROTOCOL_OPTIONS)
        .args(INPUT_FORMAT_OPTIONS)
        .arg("-i")
        .arg(path)
        .args(["-t", max_duration.as_str(), "-vn", "-ac", "1", "-ar", "16000", "-f", "s16le", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| MediaError::Undecodable(format!("无法运行 ffmpeg：{e}")))?;
    if !output.status.success() || output.stdout.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!("音轨解码失败 {}: {}", path.display(), stderr.chars().take(500).collect::<String>());
        return Err(MediaError::Undecodable("无法解码：音轨提取失败（文件损坏或没有音轨）".into()));
    }
    let mut samples: Vec<i16> = output.stdout.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect();
    // ffmpeg 偶有容器边界取整；Rust 侧再按同一上限截断，确保超过两小时的部分绝不进入识别与在线请求。
    samples.truncate(MAX_DURATION_SECONDS as usize * DECODED_SAMPLE_RATE);
    Ok(samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffprobe_缺时长视为未知_异常值也不冒充时长() {
        assert_eq!(parse_probe_duration(br#"{"format":{}}"#).unwrap(), None);
        assert_eq!(parse_probe_duration(br#"{"format":{"duration":"N/A"}}"#).unwrap(), None);
        assert_eq!(parse_probe_duration(br#"{"format":{"duration":"0"}}"#).unwrap(), None);
        assert_eq!(parse_probe_duration(br#"{"format":{"duration":"12.5"}}"#).unwrap(), Some(12.5));
        assert!(parse_probe_duration(b"not-json").is_err());
    }

    #[test]
    fn 输入限制只放本地文件与常见音视频解复用器() {
        assert_eq!(INPUT_PROTOCOL_OPTIONS, ["-protocol_whitelist", "file"]);
        let formats = INPUT_FORMAT_OPTIONS[1];
        for allowed in ["mp3", "wav", "flac", "ogg", "matroska", "mp4", "aac", "asf", "amr", "mpegts"] {
            assert!(formats.split(',').any(|item| item == allowed), "缺少格式 {allowed}");
        }
        for blocked in ["concat", "hls", "http", "https", "crypto", "data"] {
            assert!(!formats.split(',').any(|item| item == blocked), "不应放行 {blocked}");
        }
    }

    #[test]
    fn 明显伪装的播放列表在调用_ffmpeg_前被拒绝() {
        let root = std::env::temp_dir().join(format!("att-media-playlist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let hls = root.join("a.mp3");
        let concat = root.join("b.mp3");
        std::fs::write(&hls, b"\xef\xbb\xbf  \n#EXTM3U\n#EXTINF:1,\nother.wav\n").unwrap();
        std::fs::write(&concat, b"ffconcat version 1.0\nfile other.wav\n").unwrap();
        assert!(reject_active_playlist(&hls).unwrap_err().to_string().contains("播放列表"));
        assert!(reject_active_playlist(&concat).unwrap_err().to_string().contains("concat"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
