//! 把图片写进系统剪贴板：格式选错，别的应用就「粘不出图」。
//!
//! 为什么不用 arboard 的 `set_image`（Windows）：它只放 `CF_DIBV5` + 注册格式 `PNG`，
//! 而 DIBV5 头用的是 `BI_BITFIELDS`（32bpp + RGB 掩码）。系统据此合成的 `CF_DIB` 也是
//! `BI_BITFIELDS`，但 `CF_DIB` 的 BI_BITFIELDS 要求掩码紧跟 40 字节头之后——合成出来的没有，
//! 结构不合规。Skia/Chromium 系（Electron 应用、VS Code、微信/飞书/WPS、以及本应用的 WebView2）
//! 读 `CF_DIB` 只认 `BI_RGB`，也不看私有的 `PNG` 格式，于是粘贴图片「什么也没有」；
//! 而 Office / 画图走 OLE 读 DIBV5，反而正常——现象看起来就像「有的地方能粘、有的不行」。
//!
//! 所以 Windows 上自己写剪贴板：`CF_DIB`（24bpp BI_RGB）+ `CF_DIBV5`（BI_RGB）+ `PNG`，
//! 并用真实窗口做 owner（arboard 用 NULL owner，偶发 `SetClipboardData` 1418「线程没有打开的
//! 剪贴板」失败，而且失败发生在 `EmptyClipboard` 之后——会把剪贴板清空，粘贴等于什么都没粘上）。
//! 打开/写入整体重试：剪贴板同一时刻只允许一个线程打开，别的程序正在读写时会短暂占用。

use std::time::Duration;

/// 标准位图：40 字节 `BITMAPINFOHEADER` + 像素（能读它的应用最多）
const CF_DIB: u32 = 8;
/// V5 头位图（Office / 终端等偏好的格式）
const CF_DIBV5: u32 = 17;
/// 重试次数与间隔（覆盖别的程序短暂占用剪贴板的窗口）
const WRITE_ATTEMPTS: usize = 5;
const RETRY_INTERVAL: Duration = Duration::from_millis(40);

/// 把一张 RGBA 图片写进系统剪贴板。`rgba` 必须是 `width * height * 4` 字节。
/// `png`：手上已有的同一张图的 PNG 字节（如剪贴板历史里的原图文件），有就直接用，省掉一次整图编码。
pub fn write_rgba(app: &tauri::AppHandle, rgba: &[u8], width: u32, height: u32, png: Option<&[u8]>) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("图片尺寸为 0".into());
    }
    if rgba.len() != width as usize * height as usize * 4 {
        return Err("图片数据与尺寸不符".into());
    }

    #[cfg(windows)]
    {
        platform::write(app, rgba, width, height, png)
    }

    #[cfg(not(windows))]
    {
        let _ = (app, png);
        // macOS：arboard 走 NSImage，各应用都能读
        let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
        clipboard
            .set_image(arboard::ImageData { width: width as usize, height: height as usize, bytes: rgba.into() })
            .map_err(|e| e.to_string())
    }
}

/// 每行字节数：24bpp 位图按 4 字节对齐
fn stride24(width: u32) -> usize {
    (width as usize * 3).div_ceil(4) * 4
}

/// 40 字节 `BITMAPINFOHEADER` + 自下而上的 24bpp BGR 像素（`BI_RGB`）。
///
/// 自下而上是 DIB 的约定（正数 `biHeight`），自上而下的图像要翻行；
/// 24bpp 而不是 32bpp：老应用对 24bpp 的支持最普遍，CF_DIB 本身也不带 alpha 语义。
fn dib24(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let stride = stride24(width);
    let pixel_bytes = stride * height as usize;
    let mut out = vec![0u8; 40 + pixel_bytes];
    out[0..4].copy_from_slice(&40u32.to_le_bytes());
    out[4..8].copy_from_slice(&(width as i32).to_le_bytes());
    out[8..12].copy_from_slice(&(height as i32).to_le_bytes());
    out[12..14].copy_from_slice(&1i16.to_le_bytes());
    out[14..16].copy_from_slice(&24i16.to_le_bytes());
    // biCompression = BI_RGB(0) / biSizeImage：其余字段留 0
    out[20..24].copy_from_slice(&(pixel_bytes as u32).to_le_bytes());
    for row in 0..height as usize {
        let source_row = height as usize - 1 - row;
        let source = source_row * width as usize * 4;
        let target = 40 + row * stride;
        for x in 0..width as usize {
            let s = source + x * 4;
            let t = target + x * 3;
            out[t] = rgba[s + 2];
            out[t + 1] = rgba[s + 1];
            out[t + 2] = rgba[s];
        }
    }
    out
}

/// 124 字节 `BITMAPV5HEADER` + 自下而上的 32bpp BGRA 像素（`BI_RGB`，带 alpha）。
fn dibv5(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    /// 色彩空间：sRGB（`LCS_sRGB`）
    const LCS_SRGB: u32 = 0x7352_4742;
    /// 渲染意图：`LCS_GM_IMAGES`
    const LCS_GM_IMAGES: u32 = 4;
    let pixel_bytes = width as usize * height as usize * 4;
    let mut out = vec![0u8; 124 + pixel_bytes];
    out[0..4].copy_from_slice(&124u32.to_le_bytes());
    out[4..8].copy_from_slice(&(width as i32).to_le_bytes());
    out[8..12].copy_from_slice(&(height as i32).to_le_bytes());
    out[12..14].copy_from_slice(&1i16.to_le_bytes());
    out[14..16].copy_from_slice(&32i16.to_le_bytes());
    // bV5Compression = BI_RGB(0)：掩码字段在 BI_RGB 下无意义，保持 0
    out[20..24].copy_from_slice(&(pixel_bytes as u32).to_le_bytes());
    // bV5CSType（偏移 56）与 bV5Intent（偏移 108）
    out[56..60].copy_from_slice(&LCS_SRGB.to_le_bytes());
    out[108..112].copy_from_slice(&LCS_GM_IMAGES.to_le_bytes());
    for row in 0..height as usize {
        let source_row = height as usize - 1 - row;
        let source = source_row * width as usize * 4;
        let target = 124 + row * width as usize * 4;
        for x in 0..width as usize {
            let s = source + x * 4;
            let t = target + x * 4;
            out[t] = rgba[s + 2];
            out[t + 1] = rgba[s + 1];
            out[t + 2] = rgba[s];
            out[t + 3] = rgba[s + 3];
        }
    }
    out
}

/// PNG 字节：注册格式 `PNG` 用（部分新式应用优先读它，剪贴板监控读回也走这条路）
fn encode_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|e| format!("PNG 编码失败: {e}"))?;
    Ok(png)
}

#[cfg(windows)]
mod platform {
    use super::{encode_png, CF_DIB, CF_DIBV5, RETRY_INTERVAL, WRITE_ATTEMPTS};
    use windows_sys::Win32::Foundation::{GlobalFree, HANDLE, HWND};
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

    /// 已打开的剪贴板：`Drop` 时关闭，任何一步失败都不会把剪贴板锁住。
    struct Opened;

    impl Drop for Opened {
        fn drop(&mut self) {
            unsafe { CloseClipboard() };
        }
    }

    fn open(hwnd: HWND) -> Option<Opened> {
        (unsafe { OpenClipboard(hwnd) } != 0).then_some(Opened)
    }

    fn last_error(what: &str) -> String {
        match std::io::Error::last_os_error().raw_os_error() {
            Some(code) => format!("{what}（错误码 {code}）"),
            None => what.to_string(),
        }
    }

    /// 把字节装进 GMEM_MOVEABLE 全局内存交给剪贴板；成功后所有权归系统，不能再 free。
    fn put(format: u32, bytes: &[u8]) -> Result<(), String> {
        unsafe {
            let handle = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
            if handle.is_null() {
                return Err(last_error("分配剪贴板内存失败"));
            }
            let target = GlobalLock(handle);
            if target.is_null() {
                GlobalFree(handle);
                return Err(last_error("锁定剪贴板内存失败"));
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), target as *mut u8, bytes.len());
            GlobalUnlock(handle);
            if SetClipboardData(format, handle as HANDLE).is_null() {
                GlobalFree(handle);
                return Err(last_error(&format!("写入剪贴板格式 {format} 失败")));
            }
        }
        Ok(())
    }

    fn png_format() -> Option<u32> {
        let name: Vec<u16> = "PNG\0".encode_utf16().collect();
        let id = unsafe { RegisterClipboardFormatW(name.as_ptr()) };
        (id != 0).then_some(id)
    }

    /// 剪贴板 owner 窗口：用主窗口句柄，数据随窗口存活（托盘应用里主窗口只隐藏不销毁）。
    /// 主窗口取不到时退到悬浮框/悬浮条——它们也是启动期创建、随应用存活的窗口。
    fn owner(app: &tauri::AppHandle) -> HWND {
        use tauri::Manager;
        for label in ["main", crate::clipboard_panel::LABEL, crate::floating::LABEL] {
            if let Some(handle) = app.get_webview_window(label).and_then(|window| window.hwnd().ok()) {
                return handle.0;
            }
        }
        // 全都没有（理论不可达：三个窗口都在启动期创建）：退回 NULL owner，数据只活到当前线程结束
        tracing::warn!("写图片剪贴板找不到 owner 窗口，退回 NULL owner（剪贴板数据存活时间变短）");
        std::ptr::null_mut()
    }

    pub fn write(app: &tauri::AppHandle, rgba: &[u8], width: u32, height: u32, png: Option<&[u8]>) -> Result<(), String> {
        write_with_owner(owner(app), rgba, width, height, png)
    }

    /// 真正落剪贴板的实现；`hwnd` 为 NULL 时由当前线程「充当」owner（仅用于测试）。
    pub fn write_with_owner(hwnd: HWND, rgba: &[u8], width: u32, height: u32, png: Option<&[u8]>) -> Result<(), String> {
        // 先编码好，真正动剪贴板的时间越短越好（打开剪贴板期间全系统都写不了）。
        // 已有原图 PNG 就直接用：2K~4K 截图重新编码一遍要几百毫秒
        let dib = super::dib24(rgba, width, height);
        let dibv5 = super::dibv5(rgba, width, height);
        let png = match png {
            Some(bytes) => bytes.to_vec(),
            None => encode_png(rgba, width, height)?,
        };
        let png = png_format().map(|id| (id, png)).ok_or_else(|| "注册 PNG 剪贴板格式失败".to_string())?;

        let mut last = String::new();
        for attempt in 0..WRITE_ATTEMPTS {
            if attempt > 0 {
                std::thread::sleep(RETRY_INTERVAL);
            }
            let Some(_opened) = open(hwnd) else {
                last = last_error("剪贴板正被其他程序占用");
                continue;
            };
            if unsafe { EmptyClipboard() } == 0 {
                last = last_error("清空剪贴板失败");
                continue;
            }
            // CF_DIB 先放：只认它的老应用（Chromium/Skia 系）靠它
            let result = put(CF_DIB, &dib).and_then(|()| put(CF_DIBV5, &dibv5)).and_then(|()| put(png.0, &png.1));
            match result {
                Ok(()) => return Ok(()),
                Err(error) => last = error,
            }
        }
        Err(format!("写入图片到剪贴板失败：{last}"))
    }

    /// 读回某个剪贴板格式的字节（调用方负责先 open、最后 close）。
    #[cfg(test)]
    pub fn read_format(format: u32) -> Option<Vec<u8>> {
        use windows_sys::Win32::System::DataExchange::GetClipboardData;
        use windows_sys::Win32::System::Memory::GlobalSize;
        unsafe {
            let handle = GetClipboardData(format);
            if handle.is_null() {
                return None;
            }
            let size = GlobalSize(handle);
            let source = GlobalLock(handle);
            if source.is_null() || size == 0 {
                return None;
            }
            let bytes = std::slice::from_raw_parts(source as *const u8, size).to_vec();
            GlobalUnlock(handle);
            Some(bytes)
        }
    }

    /// 测试用：打开剪贴板读一遍，由 `read` 决定读什么，读完关闭。
    #[cfg(test)]
    pub fn with_open_clipboard<T>(read: impl FnOnce() -> T) -> Option<T> {
        open(std::ptr::null_mut()).map(|_opened| read())
    }

    /// 测试用：注册格式 id（`PNG`）。
    #[cfg(test)]
    pub fn png_clipboard_format() -> Option<u32> {
        png_format()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2x2：左上红、右上绿、左下蓝、右下白
    fn sample() -> Vec<u8> {
        vec![
            255, 0, 0, 255, // (0,0) 红
            0, 255, 0, 255, // (1,0) 绿
            0, 0, 255, 255, // (0,1) 蓝
            255, 255, 255, 255, // (1,1) 白
        ]
    }

    #[test]
    fn dib24_头部字段符合_bitmapinfoheader() {
        let dib = dib24(&sample(), 2, 2);
        assert_eq!(u32::from_le_bytes(dib[0..4].try_into().unwrap()), 40, "biSize");
        assert_eq!(i32::from_le_bytes(dib[4..8].try_into().unwrap()), 2, "biWidth");
        assert_eq!(i32::from_le_bytes(dib[8..12].try_into().unwrap()), 2, "biHeight 正数=自下而上");
        assert_eq!(i16::from_le_bytes(dib[12..14].try_into().unwrap()), 1, "biPlanes");
        assert_eq!(i16::from_le_bytes(dib[14..16].try_into().unwrap()), 24, "biBitCount");
        assert_eq!(i32::from_le_bytes(dib[16..20].try_into().unwrap()), 0, "BI_RGB");
        // 宽 2 → 每行 6 字节，按 4 字节对齐成 8
        assert_eq!(u32::from_le_bytes(dib[20..24].try_into().unwrap()), 16, "biSizeImage");
        assert_eq!(dib.len(), 40 + 16);
    }

    #[test]
    fn dib24_行自下而上_像素是_bgr() {
        let dib = dib24(&sample(), 2, 2);
        let pixels = &dib[40..];
        // 行宽 8（6 像素字节 + 2 对齐）；第一行是图像最后一行：蓝、白（BGR 顺序）
        assert_eq!(&pixels[0..3], &[255, 0, 0]);
        assert_eq!(&pixels[3..6], &[255, 255, 255]);
        // 第二行是图像第一行：红、绿
        assert_eq!(&pixels[8..11], &[0, 0, 255]);
        assert_eq!(&pixels[11..14], &[0, 255, 0]);
    }

    #[test]
    fn dib24_行宽按四字节对齐() {
        // 宽 3：每行 9 字节 → 补到 12
        let rgba = vec![0u8; 3 * 2 * 4];
        let dib = dib24(&rgba, 3, 2);
        assert_eq!(stride24(3), 12);
        assert_eq!(dib.len(), 40 + 12 * 2);
        // 宽 4：正好对齐
        assert_eq!(stride24(4), 12);
        assert_eq!(stride24(1), 4);
    }

    #[test]
    fn dibv5_头部字段符合_bitmapv5header() {
        let dib = dibv5(&sample(), 2, 2);
        assert_eq!(u32::from_le_bytes(dib[0..4].try_into().unwrap()), 124, "bV5Size");
        assert_eq!(i16::from_le_bytes(dib[14..16].try_into().unwrap()), 32, "bV5BitCount");
        assert_eq!(i32::from_le_bytes(dib[16..20].try_into().unwrap()), 0, "BI_RGB");
        assert_eq!(u32::from_le_bytes(dib[56..60].try_into().unwrap()), 0x7352_4742, "LCS_sRGB");
        assert_eq!(u32::from_le_bytes(dib[108..112].try_into().unwrap()), 4, "LCS_GM_IMAGES");
        assert_eq!(dib.len(), 124 + 16);
        // 第一行是图像最后一行：蓝(0,0,255,255) 白
        assert_eq!(&dib[124..128], &[255, 0, 0, 255]);
        assert_eq!(&dib[128..132], &[255, 255, 255, 255]);
    }

    #[test]
    fn png_编码尺寸正确() {
        let png = encode_png(&sample(), 2, 2).unwrap();
        let decoded = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (2, 2));
        assert_eq!(decoded.as_raw(), &sample());
    }

    /// 真机验证（会占用系统剪贴板，默认 ignore）：
    /// `cargo test -p att-app --lib 手动_写图片剪贴板 -- --ignored --nocapture`
    #[cfg(windows)]
    #[test]
    #[ignore = "会改写系统剪贴板"]
    fn 手动_写图片剪贴板后三种格式都合规() {
        use super::platform;
        let rgba = sample();
        platform::write_with_owner(std::ptr::null_mut(), &rgba, 2, 2, None).expect("写剪贴板失败");

        let (dib, dibv5, png) = platform::with_open_clipboard(|| {
            (
                platform::read_format(CF_DIB),
                platform::read_format(CF_DIBV5),
                platform::read_format(platform::png_clipboard_format().unwrap()),
            )
        })
        .expect("打开剪贴板失败");

        let dib = dib.expect("缺少 CF_DIB");
        assert_eq!(i16::from_le_bytes(dib[14..16].try_into().unwrap()), 24, "CF_DIB 位深");
        assert_eq!(i32::from_le_bytes(dib[16..20].try_into().unwrap()), 0, "CF_DIB 必须是 BI_RGB");

        let dibv5 = dibv5.expect("缺少 CF_DIBV5");
        assert_eq!(i32::from_le_bytes(dibv5[16..20].try_into().unwrap()), 0, "CF_DIBV5 必须是 BI_RGB");

        let png = png.expect("缺少 PNG 格式");
        assert_eq!(image::load_from_memory(&png).unwrap().to_rgba8().as_raw(), &rgba, "PNG 内容应可还原");

        // 剪贴板监控（arboard）走 PNG 读回：写完关掉剪贴板后必须仍读得到、且尺寸一致
        let read_back = arboard::Clipboard::new().and_then(|mut c| c.get_image()).expect("arboard 读不回图片");
        assert_eq!((read_back.width, read_back.height), (2, 2));
        assert_eq!(read_back.bytes.as_ref(), rgba.as_slice());
    }
}
