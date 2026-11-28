//! 进程内事件总线（tokio broadcast）。
//!
//! 设计：lagged 只影响 UI（可丢弃）；关键命令走
//! RecordingSessionActor 的 mpsc，不经本总线。
//! Tauri 事件桥按「旧 WS 事件名」重放给前端（旧版 WebSocket 已移除）。

use serde::Serialize;
use tokio::sync::broadcast;

/// 领域事件全集。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppEvent {
    /// 剪贴板捕获（→ clipboard:changed）
    ClipboardCaptured { item_id: String },
    /// 剪贴板条目被删（自动清理 / 批量删除 / 清空；→ clipboard:changed，列表据此刷新）
    ClipboardRemoved { count: usize },
}

impl AppEvent {
    /// 前端 Tauri 事件名（与旧 WS/IPC 事件名对齐）。
    pub fn frontend_event_name(&self) -> &'static str {
        match self {
            AppEvent::ClipboardCaptured { .. } | AppEvent::ClipboardRemoved { .. } => "clipboard:changed",
        }
    }
}

/// 广播总线：容量 256（UI 消费慢时丢旧事件，不影响正确性）。
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<AppEvent>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(256)
    }
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    pub fn publish(&self, event: AppEvent) {
        // 无订阅者时忽略发送错误
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AppEvent> {
        self.tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn 发布订阅与事件名映射() {
        let bus = EventBus::default();
        let mut rx = bus.subscribe();
        bus.publish(AppEvent::ClipboardCaptured { item_id: "cb-1".into() });
        let event = rx.recv().await.unwrap();
        assert_eq!(event.frontend_event_name(), "clipboard:changed");
        match event {
            AppEvent::ClipboardCaptured { item_id } => assert_eq!(item_id, "cb-1"),
            _ => panic!("事件类型不符"),
        }
    }
}
