//! 全局热键平台后端抽象（架构文档 §4.6 / §6.3）。
//!
//! 当前可用实现：[`listener`]——底层键盘监听（macOS CGEventTap / Windows WH_KEYBOARD_LL）
//! + [`matcher`] 触发状态机（支持纯修饰键组合，如旧版默认的 Win+Alt）。
//! 下列系统级注册（吞键 + 冲突检测）仍按设计在后续阶段落地：
//! - Windows：RegisterHotKey（系统级吞键+冲突检测）+ WH_KEYBOARD_LL（key-up）
//! - macOS：Carbon RegisterEventHotKey + CGEventTap（key-up，需辅助功能权限）
//! - Linux X11：XGrabKey（抓取即吞键，天然同时收到 KeyPress/KeyRelease）
//! - Linux Wayland：XDG portal GlobalShortcuts（activated/deactivated 双信号）

pub mod listener;
pub mod matcher;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

pub use att_core::settings::hotkey::HotkeyAction;
use att_core::settings::hotkey::{HotkeyBinding, ModSet};
use std::sync::mpsc;

/// 绑定句柄（unregister 用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindingId(pub u32);

/// 待注册的组合（后端把它翻译成平台 API 的键码集合）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeySpec {
    pub action: HotkeyAction,
    pub modifiers: ModSet,
    pub hid_key: Option<u16>,
}

impl HotkeySpec {
    pub fn from_binding(binding: &HotkeyBinding) -> Self {
        Self {
            action: binding.action,
            modifiers: binding.modifiers,
            hid_key: binding.hid_key,
        }
    }
}

/// 原始键事件：press 与 release 双相位（按住模式的基石，旧版 pynput 语义的对等物）。
#[derive(Debug, Clone)]
pub struct RawKeyEvent {
    pub binding: BindingId,
    pub action: HotkeyAction,
    pub phase: KeyPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPhase {
    Pressed,
    Released,
}

#[derive(Debug, thiserror::Error)]
pub enum HotkeyError {
    /// 系统级冲突（RegisterHotKey 失败 / XGrabKey BadAccess）：被其他应用占用。
    #[error("该快捷键已被其他应用占用")]
    Conflict,
    #[error("平台不支持全局热键: {0}")]
    Unsupported(String),
    #[error("注册失败: {0}")]
    Other(String),
}

/// 后端能力探测结果（设置页状态卡数据源，修复旧版「无权限静默失效」缺陷 #14）。
#[derive(Debug, Clone)]
pub struct BackendCapability {
    pub supported: bool,
    pub can_suppress_keys: bool,
    pub permission_prompt: Option<String>,
    pub limitation: Option<String>,
}

/// 平台后端契约：注册/注销/事件流/真实键态查询（防粘滞防线 1）/全量重同步（防线 2）。
pub trait HotkeyBackend: Send {
    fn probe(&self) -> BackendCapability;

    fn register(&mut self, spec: HotkeySpec) -> Result<BindingId, HotkeyError>;

    fn unregister(&mut self, id: BindingId) -> Result<(), HotkeyError>;

    /// 事件接收端（后端线程泵 → 上层触发状态机）。
    fn events(&self) -> mpsc::Receiver<RawKeyEvent>;

    /// 查询某组合的物理按下状态（GetAsyncKeyState / CGEventSourceKeyState / XQueryKeymap）。
    fn poll_physical_state(&self, spec: &HotkeySpec) -> bool;

    /// 系统事件（休眠/锁屏/唤醒）后强制重同步全部 held 状态。
    fn resync_all(&mut self);
}

/// 3a 空实现：声明「热键尚未接入」，供 app 在 3c 之前编译与运行。
pub struct NoopBackend {
    tx: mpsc::Sender<RawKeyEvent>,
    rx: mpsc::Receiver<RawKeyEvent>,
}

impl Default for NoopBackend {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx }
    }
}

impl HotkeyBackend for NoopBackend {
    fn probe(&self) -> BackendCapability {
        BackendCapability {
            supported: false,
            can_suppress_keys: false,
            permission_prompt: None,
            limitation: Some("热键系统将在阶段 3c 接入".into()),
        }
    }

    fn register(&mut self, _spec: HotkeySpec) -> Result<BindingId, HotkeyError> {
        Err(HotkeyError::Unsupported("阶段 3c 接入".into()))
    }

    fn unregister(&mut self, _id: BindingId) -> Result<(), HotkeyError> {
        Ok(())
    }

    fn events(&self) -> mpsc::Receiver<RawKeyEvent> {
        // mpsc 无 clone：这里返回新通道占位（空实现永不发事件，仅满足签名）
        let (tx, rx) = mpsc::channel();
        drop(tx);
        // 保留原 rx 防止未使用告警
        let _ = &self.rx;
        let _ = &self.tx;
        rx
    }

    fn poll_physical_state(&self, _spec: &HotkeySpec) -> bool {
        false
    }

    fn resync_all(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 空实现_探测为不支持() {
        let mut backend = NoopBackend::default();
        assert!(!backend.probe().supported);
        assert!(backend.register(HotkeySpec {
            action: HotkeyAction::Record,
            modifiers: ModSet::new(true, false, false, false),
            hid_key: None,
        }).is_err());
    }
}
