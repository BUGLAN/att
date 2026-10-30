//! 媒体探测与解码：ffprobe 取时长、ffmpeg 解码为 16kHz 单声道 i16 PCM（经 stdout 管道，不落临时文件）。
//!
//! 与 Python 版一样依赖系统 ffmpeg；macOS 从 Finder/Dock 启动的应用不继承 shell 的 PATH，
//! 因此除 PATH 外还查 Homebrew / MacPorts 常见位置，并支持 `ATT_FFMPEG_DIR` 显式指定。

use std::path::{Path, PathBuf};
use std::process::Stdio;

pub const VIDEO_EXTENSIONS: &[&str] = &["mp4", "flv", "webm", "mkv", "mov", "avi", "m4v", "ts"];
pub const AUDIO_EXTENSIONS: &[&str] = &["wav", "mp3", "m4a", "flac", "ogg", "opus", "aac", "wma", "amr"];

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
    dirs.into_iter().map(|dir| dir.join(&file)).find(|candidate| candidate.is_file()).ok_or(MediaError::ToolMissing(name))
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

/// ffprobe 探测时长（秒）。
pub async fn probe_duration(path: &Path) -> Result<f64, MediaError> {
    let ffprobe = find_tool("ffprobe")?;
    let output = command(&ffprobe)
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "json"])
        .arg(path)
        .output()
        .await
        .map_err(|e| MediaError::Undecodable(format!("无法运行 ffprobe：{e}")))?;
    if !output.status.success() {
        return Err(MediaError::Undecodable("无法解码：文件损坏或不是有效的音视频文件".into()));
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_default();
    let duration = value
        .pointer("/format/duration")
        .and_then(|v| v.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| v.as_f64()));
    duration.ok_or_else(|| MediaError::Undecodable("无法解码：读取媒体时长失败".into()))
}

/// 解码为 16kHz 单声道 i16 PCM（视频容器自动抽音轨）。
pub async fn decode_pcm16k(path: &Path) -> Result<Vec<i16>, MediaError> {
    let ffmpeg = find_tool("ffmpeg")?;
    let output = command(&ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-f", "s16le", "-"])
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
    Ok(output.stdout.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect())
}
