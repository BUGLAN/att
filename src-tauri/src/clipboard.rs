//! 系统剪贴板后台监控（移植自 electron/clipboardMonitor）：800ms 轮询，文本/图片去重入库。
//!
//! - 文本：规范化（CRLF→LF、trim）后与上次观测比较，变化才入库；
//! - 图片：先对原始 RGBA 做哈希判断是否变化（避免每次轮询都编码 PNG），变化后编码 PNG，
//!   按 PNG 字节 SHA-256 命名落 `data/clipboard/images/`（同图只存一份）；
//! - 本应用自己写入剪贴板（自动复制、托盘复制并粘贴、剪贴板页复制）在 5 秒内标记为内部写入，
//!   监控看到时跳过，避免转写结果反向污染剪贴板历史。

use att_core::clipboard::{hex, normalize_text, CaptureRequest};
use att_core::events::AppEvent;
use att_core::state::CoreState;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(800);
const INTERNAL_WRITE_TTL: Duration = Duration::from_secs(5);
const SOURCE: &str = "system_clipboard_monitor";

/// 内部写入标记：(规范化文本或 `image:<RGBA 哈希>`, 过期时刻)。
static INTERNAL_WRITES: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());

pub fn mark_internal_write(text: &str) {
    let normalized = normalize_text(text);
    if !normalized.is_empty() {
        push_mark(normalized);
    }
}

/// 写入图片前标记（键与监控读到的原始 RGBA 哈希一致）。
pub fn mark_internal_image(rgba: &[u8]) {
    push_mark(format!("image:{}", hex(&Sha256::digest(rgba))));
}

fn push_mark(key: String) {
    if let Ok(mut marks) = INTERNAL_WRITES.lock() {
        let now = Instant::now();
        marks.retain(|(_, expires)| *expires > now);
        marks.push((key, now + INTERNAL_WRITE_TTL));
    }
}

fn consume_mark(key: &str) -> bool {
    let Ok(mut marks) = INTERNAL_WRITES.lock() else { return false };
    let now = Instant::now();
    marks.retain(|(_, expires)| *expires > now);
    match marks.iter().position(|(k, _)| k == key) {
        Some(index) => {
            marks.remove(index);
            true
        }
        None => false,
    }
}

/// 启动后台监控线程（应用生命周期内常驻）。
pub fn start(core: Arc<CoreState>) {
    let spawned = std::thread::Builder::new().name("att-clipboard-monitor".into()).spawn(move || {
        if let Err(error) = std::fs::create_dir_all(core.clipboard.images_dir()) {
            tracing::warn!("创建剪贴板图片目录失败: {error}");
        }
        let mut clipboard: Option<arboard::Clipboard> = None;
        // 基线：启动时剪贴板里已有的内容不入库
        let mut last_observed = observe(&mut clipboard).map(|o| o.key).unwrap_or_default();
        tracing::info!("剪贴板监控已启动（{}ms 轮询）", POLL_INTERVAL.as_millis());
        loop {
            std::thread::sleep(POLL_INTERVAL);
            let Some(observation) = observe(&mut clipboard) else { continue };
            if observation.key == last_observed {
                continue;
            }
            last_observed = observation.key.clone();
            if consume_mark(&observation.key) {
                tracing::debug!("跳过本应用写入的剪贴板内容");
                continue;
            }
            let captured = match observation.content {
                Content::Text(text) => core.clipboard.capture(CaptureRequest { content: text, source: SOURCE.into(), ..Default::default() }),
                Content::Image { width, height, rgba } => capture_image(&core, width, height, rgba),
            };
            match captured {
                Ok(item) => core.events.publish(AppEvent::ClipboardCaptured { item_id: item.id }),
                Err(error) => tracing::warn!("剪贴板入库失败: {error}"),
            }
        }
    });
    if let Err(error) = spawned {
        tracing::warn!("无法启动剪贴板监控线程: {error}");
    }
}

enum Content {
    Text(String),
    Image { width: usize, height: usize, rgba: Vec<u8> },
}

struct Observation {
    /// 变化判断键：规范化文本，或 `image:<RGBA 哈希>`
    key: String,
    content: Content,
}

/// 读一次剪贴板：图片优先（与旧版一致），否则文本；剪贴板句柄失效时下次重建。
fn observe(clipboard: &mut Option<arboard::Clipboard>) -> Option<Observation> {
    if clipboard.is_none() {
        *clipboard = arboard::Clipboard::new().ok();
    }
    let board = clipboard.as_mut()?;
    if let Ok(image) = board.get_image() {
        if image.width > 0 && image.height > 0 {
            let rgba = image.bytes.into_owned();
            return Some(Observation {
                key: format!("image:{}", hex(&Sha256::digest(&rgba))),
                content: Content::Image { width: image.width, height: image.height, rgba },
            });
        }
    }
    match board.get_text() {
        Ok(text) => {
            let normalized = normalize_text(&text);
            (!normalized.is_empty()).then(|| Observation { key: normalized.clone(), content: Content::Text(normalized) })
        }
        Err(arboard::Error::ContentNotAvailable) => None,
        Err(_) => {
            *clipboard = None;
            None
        }
    }
}

/// 编码 PNG → 按 PNG 字节哈希落盘 → 入库（元数据形状与旧版一致，剪贴板页据此显示尺寸与预览）。
fn capture_image(core: &CoreState, width: usize, height: usize, rgba: Vec<u8>) -> std::io::Result<att_core::clipboard::ClipboardItem> {
    let buffer = image::RgbaImage::from_raw(width as u32, height as u32, rgba).ok_or_else(|| std::io::Error::other("剪贴板图片尺寸与数据不符"))?;
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(buffer)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(std::io::Error::other)?;
    let image_hash = hex(&Sha256::digest(&png));
    let file_name = format!("{image_hash}.png");
    let path = core.clipboard.images_dir().join(&file_name);
    if !path.exists() {
        std::fs::write(&path, &png)?;
    }
    let display_name = format!("剪切图片-{}.png", chrono::Local::now().format("%Y-%m-%d-%H-%M-%S"));
    core.clipboard.capture(CaptureRequest {
        content: format!("image:{image_hash}"),
        source: SOURCE.into(),
        item_type: Some("image".into()),
        title: Some(display_name.clone()),
        summary: Some(format!("{width} x {height}")),
        metadata: serde_json::from_value(json!({
            "image_path": format!("images/{file_name}"),
            "file_name": display_name,
            "stored_file_name": file_name,
            "image_hash": image_hash,
            "width": width,
            "height": height,
            "file_size": png.len(),
            "mime_type": "image/png",
        }))
        .ok(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 内部写入标记_命中一次即消费() {
        mark_internal_write(" 转写结果\r\n ");
        assert!(consume_mark("转写结果"));
        assert!(!consume_mark("转写结果"), "标记只抵消一次");
        mark_internal_image(&[1, 2, 3]);
        assert!(consume_mark(&format!("image:{}", hex(&Sha256::digest([1u8, 2, 3])))));
    }
}
