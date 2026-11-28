//! 底层键盘监听入口：平台钩子 → 每个动作一台 [`Matcher`] → 回调（动作 + 按下/松开）。
//!
//! 绑定由 `bindings()` 在每次键事件时读取设置层发布的无锁快照（改键即时生效，无需重启监听）；
//! 锁存期间每 250ms 用系统真实键态重同步一次（防粘滞：焦点切换/锁屏吞掉松开事件）。
//! 每次触发（按下 / 松开）都记一行日志，带触发来源与当时的修饰键：按住录音被意外打断时据此判断是谁报的松开。

use crate::matcher::Matcher;
use crate::{HotkeyAction, HotkeySpec, KeyPhase};
use att_core::settings::hotkey::ModSet;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 监听能不能吞掉命中的主键（macOS 有「辅助功能」权限时装的是可拦截监听；Windows 低级钩子总能吞）。
/// 只在启动监听时确定：macOS 运行中补授权不会让已装的只读监听变成可拦截，要重启应用。
static SUPPRESSES_KEYS: AtomicBool = AtomicBool::new(false);

pub(crate) fn set_suppresses_keys(value: bool) {
    SUPPRESSES_KEYS.store(value, Ordering::SeqCst);
}

/// 快捷键的主键会不会被吞掉（不送到前台应用）。
pub fn suppresses_keys() -> bool {
    SUPPRESSES_KEYS.load(Ordering::SeqCst)
}

/// 当前启用的全部绑定快照（录音、剪贴板悬浮框…），每个动作至多一条。
/// 推动匹配器的事件来源（写进触发日志）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// 修饰键变化（macOS FlagsChanged / Windows 修饰键按下松开）
    Modifiers,
    /// 普通按键事件（及其携带的修饰键）
    Key,
    /// 锁存期间按系统真实键态的定时校正
    Resync,
}

impl Source {
    fn label(self) -> &'static str {
        match self {
            Source::Modifiers => "修饰键变化",
            Source::Key => "按键",
            Source::Resync => "键态校正",
        }
    }
}

/// 修饰键集合的日志写法（⌘⌃⌥⇧，空集写「无」）。
fn mods_label(mods: ModSet) -> String {
    let label: String = [(mods.meta, '⌘'), (mods.ctrl, '⌃'), (mods.alt, '⌥'), (mods.shift, '⇧')]
        .iter()
        .filter_map(|(on, symbol)| on.then_some(*symbol))
        .collect();
    if label.is_empty() {
        "无".into()
    } else {
        label
    }
}

pub type BindingSource = Arc<dyn Fn() -> Arc<Vec<HotkeySpec>> + Send + Sync>;
pub type EventSink = Arc<dyn Fn(HotkeyAction, KeyPhase) + Send + Sync>;

/// 录音会话期间的内部 Esc 取消绑定。
///
/// `None` = 不在会话中，Esc 完全不受影响；`Some(modifiers)` = 按 Esc 取消并吞掉本次按键。
/// 修饰键取当前录音方式实际按着的集合：点按录音为空；按住录音取录音快捷键的修饰键，
/// 否则用户还按着快捷键时按 Esc 会因为修饰键不匹配而失效。
#[derive(Clone, Default)]
pub struct CancelBinding {
    modifiers: Arc<Mutex<Option<ModSet>>>,
}

impl CancelBinding {
    /// 会话开始时传 `Some`，回到 Idle 时传 `None`。
    pub fn set_modifiers(&self, modifiers: Option<ModSet>) {
        *self.modifiers.lock().unwrap_or_else(|error| error.into_inner()) = modifiers;
    }

    /// 当前应追加到监听器的内部绑定（不来自设置项）。
    pub fn spec(&self) -> Option<HotkeySpec> {
        let modifiers = *self.modifiers.lock().unwrap_or_else(|error| error.into_inner());
        modifiers.map(|modifiers| HotkeySpec {
            action: HotkeyAction::CancelRecording,
            modifiers,
            hid_key: Some(crate::HID_ESCAPE),
        })
    }
}

pub struct Shared {
    /// 出现过的动作各留一台：绑定被停用后它仍跟踪键态，重新启用时不会带着过期的「已按下」
    matchers: Mutex<BTreeMap<HotkeyAction, Matcher>>,
    bindings: BindingSource,
    sink: EventSink,
}

impl Shared {
    /// 推进全部匹配器并派发触发；返回 `owns_hid` 这个主键是否归某个绑定（处理前或处理后锁存着它）。
    fn with_matchers(
        &self,
        source: Source,
        owns_hid: Option<u16>,
        step: impl Fn(&mut Matcher) -> Option<KeyPhase>,
    ) -> bool {
        let specs = (self.bindings)();
        let mut fired = Vec::new();
        let mut owned = false;
        {
            let mut matchers = self.matchers.lock().unwrap_or_else(|e| e.into_inner());
            for spec in specs.iter() {
                matchers.entry(spec.action).or_default();
            }
            for (action, matcher) in matchers.iter_mut() {
                // 绑定变化（含停用）时锁存中的会合成一次 release，归属原动作
                let spec = specs.iter().find(|s| s.action == *action).cloned();
                if let Some(phase) = matcher.set_spec(spec) {
                    tracing::info!(action = ?action, ?phase, "热键触发：绑定变化");
                    fired.push((*action, phase));
                }
                // 松开事件要在处理前判断（处理后已解锁），按下 / 重复在处理后判断
                owned |= matcher.owns_key(owns_hid);
                if let Some(phase) = step(matcher) {
                    tracing::info!(
                        action = ?action,
                        ?phase,
                        mods = %mods_label(matcher.mods()),
                        other_keys = matcher.other_keys(),
                        "热键触发：{}",
                        source.label()
                    );
                    fired.push((*action, phase));
                }
                owned |= matcher.owns_key(owns_hid);
            }
        }
        for (action, phase) in fired {
            (self.sink)(action, phase);
        }
        owned
    }

    pub fn on_modifiers(&self, mods: ModSet, source: Source) {
        self.with_matchers(source, None, |m| m.on_modifiers(mods));
    }

    /// 返回 true 表示这次按键属于某个快捷键，平台钩子应当吞掉它（不再送给前台应用）。
    pub fn on_key(&self, raw: u32, hid: Option<u16>, down: bool) -> bool {
        self.with_matchers(Source::Key, hid, |m| m.on_key(raw, hid, down))
    }

    fn resync(&self, physical: ModSet) {
        self.with_matchers(Source::Resync, None, |m| m.resync(physical));
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
            bindings: Arc::new(|| {
                Arc::new(vec![
                    spec(HotkeyAction::Record, ModSet::new(false, true, false, true), None),
                    spec(HotkeyAction::ClipboardPanel, ModSet::new(true, true, false, false), Some('v')),
                ])
            }),
            sink: Arc::new(move |action, phase| sink_log.lock().unwrap().push((action, phase))),
        };
        const KEY_V: u32 = 9;
        let ctrl_alt = ModSet::new(true, true, false, false);
        shared.on_modifiers(ctrl_alt, Source::Modifiers);
        assert!(shared.on_key(KEY_V, hid_from_char('v'), true), "命中的主键按下要吞");
        assert!(shared.on_key(KEY_V, hid_from_char('v'), false), "对应的松开也吞");
        shared.on_modifiers(ModSet::default(), Source::Modifiers);
        assert!(!shared.on_key(KEY_V, hid_from_char('v'), true), "只按 V 不吞");
        assert!(!shared.on_key(KEY_V, hid_from_char('v'), false));
        assert_eq!(
            *fired.lock().unwrap(),
            vec![(HotkeyAction::ClipboardPanel, KeyPhase::Pressed), (HotkeyAction::ClipboardPanel, KeyPhase::Released)]
        );

        fired.lock().unwrap().clear();
        shared.on_modifiers(ModSet::new(false, true, false, true), Source::Modifiers);
        shared.on_modifiers(ModSet::default(), Source::Modifiers);
        assert_eq!(
            *fired.lock().unwrap(),
            vec![(HotkeyAction::Record, KeyPhase::Pressed), (HotkeyAction::Record, KeyPhase::Released)]
        );
    }

    #[test]
    fn 录音取消绑定_只在会话激活时匹配并吞掉_esc() {
        const KEY_ESC: u32 = 0x35;
        let cancel = CancelBinding::default();
        let binding_cancel = cancel.clone();
        let fired = Arc::new(Mutex::new(Vec::new()));
        let sink_log = fired.clone();
        let shared = Shared {
            matchers: Mutex::new(BTreeMap::new()),
            bindings: Arc::new(move || Arc::new(binding_cancel.spec().into_iter().collect())),
            sink: Arc::new(move |action, phase| sink_log.lock().unwrap().push((action, phase))),
        };

        assert!(!shared.on_key(KEY_ESC, Some(crate::HID_ESCAPE), true), "Idle 时 Esc 不受影响");
        assert!(!shared.on_key(KEY_ESC, Some(crate::HID_ESCAPE), false));

        cancel.set_modifiers(Some(ModSet::default()));
        assert!(shared.on_key(KEY_ESC, Some(crate::HID_ESCAPE), true), "会话中点按录音：无修饰 Esc 命中并吞键");
        assert!(shared.on_key(KEY_ESC, Some(crate::HID_ESCAPE), false), "松开也吞，不留半次按键给前台");
        assert_eq!(
            *fired.lock().unwrap(),
            vec![
                (HotkeyAction::CancelRecording, KeyPhase::Pressed),
                (HotkeyAction::CancelRecording, KeyPhase::Released)
            ]
        );

        fired.lock().unwrap().clear();
        cancel.set_modifiers(None);
        assert!(!shared.on_key(KEY_ESC, Some(crate::HID_ESCAPE), true), "回到 Idle 立即停用");
    }

    #[test]
    fn 录音取消绑定_按住录音时允许快捷键自身的修饰键() {
        const KEY_ESC: u32 = 0x35;
        const KEY_R: u32 = 0x0F;
        const CMD_SHIFT: ModSet = ModSet { ctrl: false, alt: false, shift: true, meta: true };
        let hid_r = hid_from_char('r');
        let cancel = CancelBinding::default();
        let binding_cancel = cancel.clone();
        let shared = Shared {
            matchers: Mutex::new(BTreeMap::new()),
            bindings: Arc::new(move || Arc::new(binding_cancel.spec().into_iter().collect())),
            sink: Arc::new(|_, _| {}),
        };

        cancel.set_modifiers(Some(CMD_SHIFT));
        assert!(!shared.on_key(KEY_ESC, Some(crate::HID_ESCAPE), true), "没有按住录音快捷键时不命中");
        shared.on_modifiers(CMD_SHIFT, Source::Modifiers);
        assert!(!shared.on_key(KEY_R, hid_r, true), "录音主键仍在按着，不单独触发取消");
        assert!(shared.on_key(KEY_ESC, Some(crate::HID_ESCAPE), true), "按住 ⌘⇧ 录音时 Esc 同样能取消");
        assert!(shared.on_key(KEY_ESC, Some(crate::HID_ESCAPE), false));
    }
}
