//! ATT 领域内核（阶段 3a 骨架）。
//!
//! 依赖规则（设计文档 §3）：本 crate 不依赖 tauri / 平台 GUI，
//! 全部功能可在无桌面环境的 CI 上 `cargo test` 验证。
//!
//! 模块地图：
//! - [`storage`]：JSON 原子写 / 宽容读 / Fernet 兼容加密
//! - [`settings`]：Settings(env) + UserSettings + 热键绑定模型（唯一归一化实现）
//! - [`records`]：Record 模型与 JSON 仓库（布局与 Python 版逐字节兼容）
//! - [`migration`]：一次性迁移（热键合并 / 音频错位修复 / 报告）
//! - [`events`]：进程内事件总线
//! - [`state`]：CoreState 组装（服务层共享句柄）

pub mod events;
pub mod migration;
pub mod records;
pub mod settings;
pub mod state;
pub mod storage;

/// 平台标识：沿用 Electron/Node 的命名（前端 stores/settings.js 按
/// 'win32' | 'darwin' | 'linux' 分支默认热键），保持前端零改动。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Macos,
    Linux,
}

impl Platform {
    /// 前端兼容字符串。
    pub fn as_str(&self) -> &'static str {
        match self {
            Platform::Windows => "win32",
            Platform::Macos => "darwin",
            Platform::Linux => "linux",
        }
    }

    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::Macos
        } else {
            Platform::Linux
        }
    }
}

/// 当前时间的 RFC3339 字符串（记录 created_at/updated_at 统一格式）。
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}
