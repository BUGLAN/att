//! 底层键盘监听入口：平台钩子 → 每个动作一台 [`Matcher`] → 回调（动作 + 按下/松开）。
//!
//! 绑定由 `bindings()` 在每次键事件时读取（设置页改键即时生效，无需重启监听）；
//! 锁存期间每 250ms 用系统真实键态重同步一次（防粘滞：焦点切换/锁屏吞掉松开事件）。

use crate::matcher::Matcher;
use crate::{HotkeyAction, HotkeySpec, KeyPhase};
use att_core::settings::hotkey::ModSet;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 当前启用的全部绑定（录音、剪贴板悬浮框…），每个动作至多一条。
pub type BindingSource = Arc<dyn Fn() -> Vec<HotkeySpec> + Send + Sync>;
pub type EventSink = Arc<dyn Fn(HotkeyAction, KeyPhase) + Send + Sync>;

pub struct Shared {
    /// 出现过的动作各留一台：绑定被停用后它仍跟踪键态，重新启用时不会带着过期的「已按下」
    matchers: Mutex<BTreeMap<HotkeyAction, Matcher>>,
    bindings: BindingSource,
    sink: EventSink,
}

impl Shared {
    /// 推进全部匹配器并派发触发；返回 `owns_hid` 这个主键是否归某个绑定（处理前或处理后锁存着它）。
    fn with_matchers(&self, owns_hid: Option<u16>, step: impl Fn(&mut Matcher) -> Option<KeyPhase>) -> bool {
        let specs = (self.bindings)();
        let mut fired = Vec::new();
        let mut owned = false;
        {
            let mut matchers = self.matchers.lock().unwrap_or_else(|e| e.into_inner());
            for spec in &specs {
                matchers.entry(spec.action).or_default();
            }
            for (action, matcher) in matchers.iter_mut() {
                // 绑定变化（含停用）时锁存中的会合成一次 release，归属原动作
                let spec = specs.iter().find(|s| s.action == *action).cloned();
                fired.extend(matcher.set_spec(spec).map(|phase| (*action, phase)));
                // 松开事件要在处理前判断（处理后已解锁），按下 / 重复在处理后判断
                owned |= matcher.owns_key(owns_hid);
                fired.extend(step(matcher).map(|phase| (*action, phase)));
                owned |= matcher.owns_key(owns_hid);
            }
        }
        for (action, phase) in fired {
            (self.sink)(action, phase);
        }
        owned
    }

    pub fn on_modifiers(&self, mods: ModSet) {
        self.with_matchers(None, |m| m.on_modifiers(mods));
    }

    /// 返回 true 表示这次按键属于某个快捷键，平台钩子应当吞掉它（不再送给前台应用）。
    pub fn on_key(&self, raw: u32, hid: Option<u16>, down: bool) -> bool {
        self.with_matchers(hid, |m| m.on_key(raw, hid, down))
    }

    fn resync(&self, physical: ModSet) {
        self.with_matchers(None, |m| m.resync(physical));
    }

    fn is_latched(&self) -> bool {
        self.matchers.lock().map(|m| m.values().any(Matcher::is_latched)).unwrap_or(false)
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
pub fn start(bindings: BindingSource, sink: EventSink) -> Result<(), String> {
    let shared = Arc::new(Shared { matchers: Mutex::new(BTreeMap::new()), bindings, sink });

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

#[cfg(test)]
mod tests {
    use super::*;
    use att_core::settings::hotkey::hid_from_char;

    fn spec(action: HotkeyAction, mods: ModSet, key: Option<char>) -> HotkeySpec {
        HotkeySpec { action, modifiers: mods, hid_key: key.and_then(hid_from_char) }
    }

    #[test]
    fn 两个绑定各自触发_互不串台() {
        let fired = Arc::new(Mutex::new(Vec::new()));
        let sink_log = fired.clone();
        let shared = Shared {
            matchers: Mutex::new(BTreeMap::new()),
            bindings: Arc::new(|| vec![
                spec(HotkeyAction::Record, ModSet::new(false, true, false, true), None),
                spec(HotkeyAction::ClipboardPanel, ModSet::new(true, true, false, false), Some('v')),
            ]),
            sink: Arc::new(move |action, phase| sink_log.lock().unwrap().push((action, phase))),
        };
        const KEY_V: u32 = 9;
        let ctrl_alt = ModSet::new(true, true, false, false);
        shared.on_modifiers(ctrl_alt);
        assert!(shared.on_key(KEY_V, hid_from_char('v'), true), "命中的主键按下要吞");
        assert!(shared.on_key(KEY_V, hid_from_char('v'), false), "对应的松开也吞");
        shared.on_modifiers(ModSet::default());
        assert!(!shared.on_key(KEY_V, hid_from_char('v'), true), "只按 V 不吞");
        assert!(!shared.on_key(KEY_V, hid_from_char('v'), false));
        assert_eq!(
            *fired.lock().unwrap(),
            vec![(HotkeyAction::ClipboardPanel, KeyPhase::Pressed), (HotkeyAction::ClipboardPanel, KeyPhase::Released)]
        );

        fired.lock().unwrap().clear();
        shared.on_modifiers(ModSet::new(false, true, false, true));
        shared.on_modifiers(ModSet::default());
        assert_eq!(
            *fired.lock().unwrap(),
            vec![(HotkeyAction::Record, KeyPhase::Pressed), (HotkeyAction::Record, KeyPhase::Released)]
        );
    }
}
