//! 底层键盘监听入口：平台钩子 → [`Matcher`] → 回调（按下/松开）。
//!
//! 绑定由 `binding()` 在每次键事件时读取（设置页改键即时生效，无需重启监听）；
//! 锁存期间每 250ms 用系统真实键态重同步一次（防粘滞：焦点切换/锁屏吞掉松开事件）。

use crate::matcher::Matcher;
use crate::{HotkeyAction, HotkeySpec, KeyPhase};
use att_core::settings::hotkey::ModSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub type BindingSource = Arc<dyn Fn() -> Option<HotkeySpec> + Send + Sync>;
pub type EventSink = Arc<dyn Fn(HotkeyAction, KeyPhase) + Send + Sync>;

pub struct Shared {
    matcher: Mutex<Matcher>,
    binding: BindingSource,
    sink: EventSink,
}

impl Shared {
    fn with_matcher(&self, step: impl FnOnce(&mut Matcher) -> Option<KeyPhase>) {
        let spec = (self.binding)();
        let action = spec.as_ref().map(|s| s.action);
        let mut phases = Vec::new();
        {
            let mut matcher = self.matcher.lock().unwrap_or_else(|e| e.into_inner());
            // 绑定变化时合成的 release 归属旧绑定动作；v1 只有录音动作，统一按 Record 发出
            phases.extend(matcher.set_spec(spec));
            phases.extend(step(&mut matcher));
        }
        for phase in phases {
            (self.sink)(action.unwrap_or(HotkeyAction::Record), phase);
        }
    }

    pub fn on_modifiers(&self, mods: ModSet) {
        self.with_matcher(|m| m.on_modifiers(mods));
    }

    pub fn on_key(&self, raw: u32, hid: Option<u16>, down: bool) {
        self.with_matcher(|m| m.on_key(raw, hid, down));
    }

    fn resync(&self, physical: ModSet) {
        self.with_matcher(|m| m.resync(physical));
    }

    fn is_latched(&self) -> bool {
        self.matcher.lock().map(|m| m.is_latched()).unwrap_or(false)
    }
}

/// 系统真实修饰键状态（不支持的平台返回 None）。
pub fn physical_modifiers() -> Option<ModSet> {
    #[cfg(target_os = "macos")]
    return crate::macos::physical_modifiers();
    #[cfg(windows)]
    return crate::windows::physical_modifiers();
    #[allow(unreachable_code)]
    None
}

/// 启动全局键盘监听。失败（无权限/平台不支持）返回可直接展示的中文原因。
pub fn start(binding: BindingSource, sink: EventSink) -> Result<(), String> {
    let shared = Arc::new(Shared { matcher: Mutex::new(Matcher::default()), binding, sink });

    #[cfg(target_os = "macos")]
    crate::macos::spawn(shared.clone())?;
    #[cfg(windows)]
    crate::windows::spawn(shared.clone())?;
    #[cfg(not(any(target_os = "macos", windows)))]
    return Err("当前平台暂不支持全局快捷键（Linux 将在后续版本接入）".into());

    // 防粘滞轮询：只在锁存期间做真实键态比对
    #[allow(unreachable_code)]
    std::thread::Builder::new()
        .name("att-hotkey-resync".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_millis(250));
            if shared.is_latched() {
                if let Some(physical) = physical_modifiers() {
                    shared.resync(physical);
                }
            }
        })
        .map_err(|e| format!("无法启动热键重同步线程: {e}"))?;
    Ok(())
}
