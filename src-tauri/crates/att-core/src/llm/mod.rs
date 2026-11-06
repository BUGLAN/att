//! LLM v2：服务商注册表 + 档案/模型存储 + 调用网关（移植自 Python `app/llm/`）。
//!
//! - [`registry`]：服务商目录（只登记协议前缀与元信息，不登记模型清单）
//! - [`store`]：`data/llm_v2.json`（与 Python 版同格式），含 v1 `llm_configs.json` 自动迁移
//! - [`gateway`]：按协议直发 HTTP（OpenAI 兼容 / Anthropic），验证与模型列取
//! - [`errors`]：错误分级与密钥脱敏
//! - [`prompt`] / [`cleaning`] / [`timeouts`]：提示词信封、输出清洗、自适应超时（纯函数）
//! - [`optimizer`]：记录优化编排（流式 + 首字前失败回退非流式 + 取消 + 状态复位）

pub mod cleaning;
pub mod errors;
pub mod gateway;
pub mod optimizer;
pub mod prompt;
pub mod registry;
pub mod store;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod timeouts;
