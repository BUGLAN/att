//! 热键绑定模型（阶段 3a：数据与归一化；平台注册后端见 att-hotkey，3c 接入）。
//!
//! 设计要点：
//! - 主键用 **USB HID Usage ID**（硬件键位，布局无关）；配置跨平台可移植
//! - 多绑定模型：一个绑定 = 动作 + 触发模式 + 修饰键集合 + 可选主键
//! - 提供与旧版 `{modifiers:["win","alt"], key:"r"|null, display}`
//!   的双向转换（迁移与前端兼容视图用）

use crate::Platform;
use serde::{Deserialize, Serialize};

/// 快捷键动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyAction {
    /// 按住录音：按下开始、松开结束
    Record,
    /// 点按录音：按一下开始、再按一下结束（长段口述不用一直按着）。
    /// 与按住录音各有一条绑定，可以用不同的键；同一个键只能启用其中一种（撞键校验会拒绝）
    RecordToggle,
    /// 显示/唤起主窗口（预留给 3c）
    ShowMainWindow,
    /// 唤起 / 收起剪贴板悬浮框（点按切换）
    ClipboardPanel,
    /// 优化选中文本：取前台应用的选中文字，按「选中文本优化」预设改写后流式输回原处（点按触发）
    RewriteSelection,
}

impl HotkeyAction {
    /// 动作对应的触发方式：按住录音是按住，其余（点按录音、剪贴板悬浮框、优化选中文本）都是点按。
    pub fn trigger(self) -> TriggerMode {
        match self {
            HotkeyAction::Record => TriggerMode::Hold,
            _ => TriggerMode::Toggle,
        }
    }

    /// 是否是录音动作（按住 / 点按）。
    pub fn is_record(self) -> bool {
        matches!(self, HotkeyAction::Record | HotkeyAction::RecordToggle)
    }

    /// 中文名（撞键提示等文案用）。
    pub fn label(self) -> &'static str {
        match self {
            HotkeyAction::Record => "按住录音",
            HotkeyAction::RecordToggle => "点按录音",
            HotkeyAction::ShowMainWindow => "显示主窗口",
            HotkeyAction::ClipboardPanel => "剪贴板悬浮框",
            HotkeyAction::RewriteSelection => "优化选中文本",
        }
    }
}

/// 触发模式：由动作决定（见 [`HotkeyAction::trigger`]），存盘字段只为兼容旧数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerMode {
    /// 点按：按下切换开始/停止
    Toggle,
    /// 按住：按下开始、松开结束（旧版唯一模式，迁移默认值）
    Hold,
}

/// 修饰键集合（旧版的 win/cmd 统一归一到 meta；mac 上显示为 ⌘）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModSet {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ctrl: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub alt: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shift: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub meta: bool,
}

impl ModSet {
    pub fn new(ctrl: bool, alt: bool, shift: bool, meta: bool) -> Self {
        Self { ctrl, alt, shift, meta }
    }

    pub fn is_empty(&self) -> bool {
        !self.ctrl && !self.alt && !self.shift && !self.meta
    }

    pub fn len(&self) -> usize {
        self.ctrl as usize + self.alt as usize + self.shift as usize + self.meta as usize
    }

    /// 是否包含另一集合的全部修饰键。
    pub fn contains(&self, other: &ModSet) -> bool {
        (self.ctrl || !other.ctrl)
            && (self.alt || !other.alt)
            && (self.shift || !other.shift)
            && (self.meta || !other.meta)
    }

    /// 规范化字符串列表（展示顺序：meta < ctrl < alt < shift）。
    pub fn to_name_list(&self) -> Vec<&'static str> {
        let mut list = Vec::new();
        if self.meta {
            list.push("meta");
        }
        if self.ctrl {
            list.push("ctrl");
        }
        if self.alt {
            list.push("alt");
        }
        if self.shift {
            list.push("shift");
        }
        list
    }

    /// 从旧版修饰键名解析（win/cmd → meta；未知项忽略；去重）。
    pub fn from_legacy_names(names: &[serde_json::Value]) -> Self {
        let mut set = ModSet::default();
        for name in names {
            match name.as_str().map(str::trim).map(str::to_lowercase).as_deref() {
                Some("ctrl") => set.ctrl = true,
                Some("alt") => set.alt = true,
                Some("shift") => set.shift = true,
                Some("win") | Some("cmd") | Some("meta") | Some("super") => set.meta = true,
                _ => {}
            }
        }
        set
    }
}

/// 一条快捷键绑定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotkeyBinding {
    pub action: HotkeyAction,
    pub trigger: TriggerMode,
    pub modifiers: ModSet,
    /// USB HID Usage ID（键盘/键区页 0x07）。None = 纯修饰键组合。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hid_key: Option<u16>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

impl HotkeyBinding {
    /// 平台默认绑定（与旧版 desktop-settings.js 的默认值一致）：
    /// Windows `Win+Alt`、macOS `⌘⇧R`、Linux `Ctrl+Shift+R`，触发模式默认按住。
    pub fn default_binding(platform: Platform) -> Self {
        let (modifiers, hid_key) = match platform {
            Platform::Windows => (ModSet::new(false, true, false, true), None),
            Platform::Macos => (ModSet::new(false, false, true, true), hid_from_char('r')),
            Platform::Linux => (ModSet::new(true, false, true, false), hid_from_char('r')),
        };
        Self {
            action: HotkeyAction::Record,
            trigger: TriggerMode::Hold,
            modifiers,
            hid_key,
            enabled: true,
        }
    }

    /// 各动作的平台默认绑定。剪贴板悬浮框默认 `⌃⌥V` / `Ctrl+Alt+V`：
    /// 监听是只听不吞的，组合键同时会送到前台应用，所以避开会打出字符的 `⌥V` 和常见的 `⌘⇧V`。
    /// 优化选中文本默认 `⌥A` / `Alt+A`：有「辅助功能」权限时主键会被吞掉（不会打出 `å`），
    /// 而这个功能本身就要靠辅助功能权限去复制选区、输回文字。
    pub fn default_for(action: HotkeyAction, platform: Platform) -> Self {
        match action {
            HotkeyAction::ClipboardPanel => Self {
                action,
                trigger: TriggerMode::Toggle,
                modifiers: ModSet::new(true, true, false, false),
                hid_key: hid_from_char('v'),
                enabled: true,
            },
            HotkeyAction::RewriteSelection => Self {
                action,
                trigger: TriggerMode::Toggle,
                modifiers: ModSet::new(false, true, false, false),
                hid_key: hid_from_char('a'),
                enabled: true,
            },
            // 点按录音默认不启用（快捷键为空）：组合与按住录音的默认值相同，用户录入新键后才启用
            HotkeyAction::RecordToggle => Self {
                action,
                trigger: TriggerMode::Toggle,
                enabled: false,
                ..Self::default_binding(platform)
            },
            _ => Self { action, ..Self::default_binding(platform) },
        }
    }

    /// 两条绑定是否是同一组按键（不看动作、触发模式与开关）。
    pub fn same_combo(&self, other: &HotkeyBinding) -> bool {
        self.modifiers == other.modifiers && self.hid_key == other.hid_key
    }

    /// 兼容旧版的主键字符（仅字母数字在旧格式里存在）。
    pub fn legacy_key_char(&self) -> Option<String> {
        self.hid_key.and_then(char_from_hid).map(|c| c.to_string())
    }

    /// 旧版视图 `{modifiers, key, display}`（前端 stores/settings.js 仍消费此形状）。
    pub fn to_legacy(&self, platform: Platform) -> serde_json::Value {
        serde_json::json!({
            "modifiers": self.legacy_modifier_names(platform),
            "key": self.legacy_key_char(),
            "display": self.display(platform),
        })
    }

    /// 旧版修饰键名（win32/linux 上 meta→"win"；darwin 上 meta→"cmd"）。
    pub fn legacy_modifier_names(&self, platform: Platform) -> Vec<&'static str> {
        let mut names = Vec::new();
        let meta_name = match platform {
            Platform::Macos => "cmd",
            _ => "win",
        };
        // 与旧版排序优先级一致：cmd/win < ctrl < alt < shift
        if self.modifiers.meta {
            names.push(meta_name);
        }
        if self.modifiers.ctrl {
            names.push("ctrl");
        }
        if self.modifiers.alt {
            names.push("alt");
        }
        if self.modifiers.shift {
            names.push("shift");
        }
        names
    }

    /// 展示名：macOS 用符号紧凑拼接（⌘⇧R），其余用 `Ctrl+Shift+R`。
    pub fn display(&self, platform: Platform) -> String {
        let key = self.hid_key.and_then(char_from_hid);
        if platform == Platform::Macos {
            let mut out = String::new();
            if self.modifiers.meta {
                out.push('⌘');
            }
            if self.modifiers.ctrl {
                out.push('⌃');
            }
            if self.modifiers.alt {
                out.push('⌥');
            }
            if self.modifiers.shift {
                out.push('⇧');
            }
            if let Some(k) = key {
                out.push(k.to_ascii_uppercase());
            }
            out
        } else {
            let parts: Vec<String> = self
                .legacy_modifier_names(platform)
                .iter()
                .map(|m| match *m {
                    "win" => "Win".to_string(),
                    other => other.replace("ctrl", "Ctrl").replace("alt", "Alt").replace("shift", "Shift"),
                })
                .chain(key.map(|k| k.to_ascii_uppercase().to_string()))
                .collect();
            parts.join("+")
        }
    }
}

// ============ USB HID 键码表（键盘页 0x07，字母数字区） ============
// 完整表在 3c 接平台后端时补全（F 区/方向键等）；3a 只需要旧数据迁移用到的字母数字。

const HID_A: u16 = 0x04;
const HID_ONE: u16 = 0x1E;
const HID_ZERO: u16 = 0x27;

/// 单字符（a-z / 0-9）→ HID Usage ID。
pub fn hid_from_char(c: char) -> Option<u16> {
    let c = c.to_ascii_lowercase();
    if c.is_ascii_lowercase() {
        Some(HID_A + (c as u16 - 'a' as u16))
    } else if c.is_ascii_digit() {
        if c == '0' {
            Some(HID_ZERO)
        } else {
            Some(HID_ONE + (c as u16 - '1' as u16))
        }
    } else {
        None
    }
}

/// HID Usage ID → 单字符（仅覆盖字母数字区，其余返回 None，展示由平台层按键名解析）。
pub fn char_from_hid(hid: u16) -> Option<char> {
    match hid {
        HID_A..=0x1D => Some((b'a' + (hid - HID_A) as u8) as char),
        HID_ONE..=0x26 => Some((b'1' + (hid - HID_ONE) as u8) as char),
        HID_ZERO => Some('0'),
        _ => None,
    }
}

/// 旧版 `{modifiers, key, display}` → 新绑定（迁移 & 前端 PUT 兼容入口）。
/// key 仅接受单字母数字（旧版约束）；不合法时回落平台默认。
pub fn binding_from_legacy(platform: Platform, value: &serde_json::Value) -> HotkeyBinding {
    combo_from_legacy(value)
        .map(|(modifiers, hid_key)| HotkeyBinding {
            action: HotkeyAction::Record,
            trigger: TriggerMode::Hold,
            modifiers,
            hid_key,
            enabled: true,
        })
        .unwrap_or_else(|| HotkeyBinding::default_binding(platform))
}

/// 旧版 `{modifiers, key}` → 组合键；不合法（单修饰键且无主键等）返回 None。
pub fn combo_from_legacy(value: &serde_json::Value) -> Option<(ModSet, Option<u16>)> {
    let modifiers = value
        .get("modifiers")
        .and_then(|m| m.as_array())
        .map(|arr| ModSet::from_legacy_names(arr))
        .unwrap_or_default();
    let hid_key = value
        .get("key")
        .and_then(|k| k.as_str())
        .and_then(|s| s.chars().next())
        .and_then(hid_from_char);

    // 旧版规则：无主键时至少 2 个修饰键，否则视为非法
    let valid = if hid_key.is_some() {
        !modifiers.is_empty()
    } else {
        modifiers.len() >= 2
    };
    valid.then_some((modifiers, hid_key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 默认绑定_三平台与旧版一致() {
        let win = HotkeyBinding::default_binding(Platform::Windows);
        assert_eq!(win.display(Platform::Windows), "Win+Alt");
        assert!(win.hid_key.is_none());

        let mac = HotkeyBinding::default_binding(Platform::Macos);
        assert_eq!(mac.display(Platform::Macos), "⌘⇧R");
        assert_eq!(mac.hid_key, Some(0x15)); // 'r' = 0x04 + 17

        let linux = HotkeyBinding::default_binding(Platform::Linux);
        assert_eq!(linux.display(Platform::Linux), "Ctrl+Shift+R");
    }

    #[test]
    fn 旧格式往返转换() {
        let legacy = serde_json::json!({"modifiers": ["win", "alt"], "key": null});
        let binding = binding_from_legacy(Platform::Windows, &legacy);
        assert!(binding.modifiers.meta && binding.modifiers.alt);
        assert!(binding.hid_key.is_none());
        assert_eq!(binding.display(Platform::Windows), "Win+Alt");

        let legacy_r = serde_json::json!({"modifiers": ["shift", "ctrl"], "key": "R"});
        let binding = binding_from_legacy(Platform::Linux, &legacy_r);
        assert_eq!(binding.legacy_key_char().as_deref(), Some("r"));
        assert_eq!(binding.display(Platform::Linux), "Ctrl+Shift+R");
        assert_eq!(binding.display(Platform::Macos), "⌃⇧R");
    }

    #[test]
    fn 旧格式非法时回退平台默认() {
        // 单修饰键且无主键 → 旧版判非法
        let bad = serde_json::json!({"modifiers": ["ctrl"], "key": null});
        let binding = binding_from_legacy(Platform::Windows, &bad);
        assert_eq!(binding, HotkeyBinding::default_binding(Platform::Windows));
    }

    #[test]
    fn 剪贴板悬浮框默认_点按的_ctrl_alt_v() {
        let binding = HotkeyBinding::default_for(HotkeyAction::ClipboardPanel, Platform::Macos);
        assert_eq!(binding.trigger, TriggerMode::Toggle);
        assert_eq!(binding.display(Platform::Macos), "⌃⌥V");
        assert_eq!(binding.display(Platform::Windows), "Ctrl+Alt+V");
        assert!(!binding.same_combo(&HotkeyBinding::default_binding(Platform::Macos)));
    }

    #[test]
    fn 优化选中文本默认_点按的_alt_a() {
        let binding = HotkeyBinding::default_for(HotkeyAction::RewriteSelection, Platform::Macos);
        assert_eq!(binding.trigger, TriggerMode::Toggle);
        assert_eq!(binding.display(Platform::Macos), "⌥A");
        assert_eq!(binding.display(Platform::Windows), "Alt+A");
        for other in [
            HotkeyBinding::default_binding(Platform::Macos),
            HotkeyBinding::default_binding(Platform::Windows),
            HotkeyBinding::default_for(HotkeyAction::ClipboardPanel, Platform::Macos),
        ] {
            assert!(!binding.same_combo(&other));
        }
        assert_eq!(serde_json::to_value(HotkeyAction::RewriteSelection).unwrap(), "rewrite_selection");
    }

    #[test]
    fn 点按录音默认_未启用且触发方式为点按() {
        let binding = HotkeyBinding::default_for(HotkeyAction::RecordToggle, Platform::Windows);
        assert!(!binding.enabled, "默认快捷键为空，不和按住录音抢键");
        assert_eq!(binding.trigger, TriggerMode::Toggle);
        assert_eq!(HotkeyAction::RecordToggle.trigger(), TriggerMode::Toggle);
        assert_eq!(HotkeyAction::Record.trigger(), TriggerMode::Hold);
        assert_eq!(serde_json::to_value(HotkeyAction::RecordToggle).unwrap(), "record_toggle");
    }

    #[test]
    fn hid_字母数字映射() {
        assert_eq!(hid_from_char('a'), Some(0x04));
        assert_eq!(hid_from_char('Z'), Some(0x1D));
        assert_eq!(hid_from_char('1'), Some(0x1E));
        assert_eq!(hid_from_char('0'), Some(0x27));
        assert_eq!(hid_from_char('!'), None, "非字母数字（旧版 Shift+数字的 char）不在映射内");
        assert_eq!(char_from_hid(0x04), Some('a'));
    }
}
