//! 热键匹配状态机（纯逻辑，平台无关；语义移植自旧版 pynput 桌面代理）。
//!
//! - 匹配条件：当前修饰键集合 **精确等于** 绑定的修饰键；
//!   有主键时按下的非修饰键恰好只有该主键，纯修饰键组合时不能有其他键按下。
//! - 匹配成立且未锁存 → `Pressed`（锁存）；锁存后条件不再成立 → `Released`。
//! - 纯修饰键组合锁存期间按下其他键：只要修饰键集合不变就保持（不误发 release）。
//! - 键自动重复不会重复触发（锁存去重，替代旧版 250ms 防抖）。

use crate::{HotkeySpec, KeyPhase};
use att_core::settings::hotkey::ModSet;
use std::collections::BTreeSet;

#[derive(Debug, Default)]
pub struct Matcher {
    spec: Option<HotkeySpec>,
    mods: ModSet,
    /// 当前按下的非修饰键（平台原始键码）
    keys: BTreeSet<u32>,
    /// 当前按下且能映射到 HID 的非修饰键
    hid_keys: BTreeSet<u16>,
    latched: bool,
}

impl Matcher {
    pub fn is_latched(&self) -> bool {
        self.latched
    }

    /// 这个主键事件该不该吞掉：绑定有主键、正是这个键、且组合处于锁存（按下 / 自动重复 / 松开前）。
    /// 纯修饰键组合不吞（修饰键本身不会打出字）。
    pub fn owns_key(&self, hid: Option<u16>) -> bool {
        self.latched && hid.is_some() && self.spec.as_ref().is_some_and(|spec| spec.hid_key == hid)
    }

    /// 切换绑定：变化时清空状态，锁存中则合成一次 release（换键不留粘滞，缺陷 #9）。
    pub fn set_spec(&mut self, spec: Option<HotkeySpec>) -> Option<KeyPhase> {
        if self.spec == spec {
            return None;
        }
        self.spec = spec;
        if std::mem::replace(&mut self.latched, false) {
            return Some(KeyPhase::Released);
        }
        None
    }

    /// 修饰键全集更新（macOS 每个事件都带完整 flags；Windows 由键态推导）。
    pub fn on_modifiers(&mut self, mods: ModSet) -> Option<KeyPhase> {
        self.mods = mods;
        self.refresh()
    }

    pub fn on_key(&mut self, raw: u32, hid: Option<u16>, down: bool) -> Option<KeyPhase> {
        if down {
            self.keys.insert(raw);
            if let Some(hid) = hid {
                self.hid_keys.insert(hid);
            }
        } else {
            self.keys.remove(&raw);
            if let Some(hid) = hid {
                self.hid_keys.remove(&hid);
            }
        }
        self.refresh()
    }

    /// 物理键态重同步（防粘滞）：修饰键以系统真实状态为准，非修饰键全部视为已松开。
    pub fn resync(&mut self, physical_mods: ModSet) -> Option<KeyPhase> {
        if !self.latched {
            return None;
        }
        self.mods = physical_mods;
        let Some(spec) = &self.spec else {
            return self.refresh();
        };
        if spec.hid_key.is_some() && physical_mods != spec.modifiers {
            self.keys.clear();
            self.hid_keys.clear();
        }
        self.refresh()
    }

    fn matches(&self) -> bool {
        let Some(spec) = &self.spec else {
            return false;
        };
        if self.mods != spec.modifiers {
            return false;
        }
        match spec.hid_key {
            None => self.keys.is_empty(),
            Some(key) => self.keys.len() == 1 && self.hid_keys.len() == 1 && self.hid_keys.contains(&key),
        }
    }

    fn refresh(&mut self) -> Option<KeyPhase> {
        if self.matches() {
            if !self.latched {
                self.latched = true;
                return Some(KeyPhase::Pressed);
            }
            return None;
        }
        let modifier_only_still_held = self
            .spec
            .as_ref()
            .map(|spec| spec.hid_key.is_none() && self.mods == spec.modifiers)
            .unwrap_or(false);
        if self.latched && modifier_only_still_held {
            return None;
        }
        if self.latched {
            self.latched = false;
            return Some(KeyPhase::Released);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use att_core::settings::hotkey::{hid_from_char, HotkeyAction};

    fn spec(mods: ModSet, key: Option<char>) -> Option<HotkeySpec> {
        Some(HotkeySpec { action: HotkeyAction::Record, modifiers: mods, hid_key: key.and_then(hid_from_char) })
    }

    const CMD_ALT: ModSet = ModSet { ctrl: false, alt: true, shift: false, meta: true };
    const CMD: ModSet = ModSet { ctrl: false, alt: false, shift: false, meta: true };

    #[test]
    fn 纯修饰键_按下松开() {
        let mut m = Matcher::default();
        m.set_spec(spec(CMD_ALT, None));
        assert_eq!(m.on_modifiers(CMD), None);
        assert_eq!(m.on_modifiers(CMD_ALT), Some(KeyPhase::Pressed));
        // 锁存期间按其他键：修饰键不变就保持
        assert_eq!(m.on_key(49, None, true), None);
        assert_eq!(m.on_key(49, None, false), None);
        assert_eq!(m.on_modifiers(CMD), Some(KeyPhase::Released));
        assert_eq!(m.on_modifiers(ModSet::default()), None);
    }

    #[test]
    fn 纯修饰键_有其他键按着时不触发() {
        let mut m = Matcher::default();
        m.set_spec(spec(CMD_ALT, None));
        m.on_key(49, None, true);
        assert_eq!(m.on_modifiers(CMD_ALT), None);
    }

    #[test]
    fn 修饰键加主键_自动重复不重复触发() {
        let mut m = Matcher::default();
        m.set_spec(spec(CMD, Some('r')));
        let r = hid_from_char('r');
        m.on_modifiers(CMD);
        assert_eq!(m.on_key(15, r, true), Some(KeyPhase::Pressed));
        assert_eq!(m.on_key(15, r, true), None, "自动重复");
        assert_eq!(m.on_key(15, r, false), Some(KeyPhase::Released));
    }

    #[test]
    fn 主键吞键_按下重复松开都归绑定_纯修饰键不吞() {
        const ALT: ModSet = ModSet { ctrl: false, alt: true, shift: false, meta: false };
        let mut m = Matcher::default();
        m.set_spec(spec(ALT, Some('v')));
        let v = hid_from_char('v');
        m.on_modifiers(ALT);
        m.on_key(9, v, true);
        assert!(m.owns_key(v), "按下即锁存，吞掉");
        m.on_key(9, v, true);
        assert!(m.owns_key(v), "自动重复也吞");
        assert!(!m.owns_key(hid_from_char('c')), "别的键不吞");

        let mut pure = Matcher::default();
        pure.set_spec(spec(CMD_ALT, None));
        pure.on_modifiers(CMD_ALT);
        assert!(pure.is_latched());
        assert!(!pure.owns_key(None));
    }

    #[test]
    fn 换绑定与重同步_合成释放() {
        let mut m = Matcher::default();
        m.set_spec(spec(CMD_ALT, None));
        m.on_modifiers(CMD_ALT);
        assert!(m.is_latched());
        assert_eq!(m.set_spec(spec(CMD, Some('r'))), Some(KeyPhase::Released));

        m.set_spec(spec(CMD_ALT, None));
        m.on_modifiers(CMD_ALT);
        // 松开事件丢失（焦点切换/安全输入），物理键态已全部松开
        assert_eq!(m.resync(ModSet::default()), Some(KeyPhase::Released));
    }
}
