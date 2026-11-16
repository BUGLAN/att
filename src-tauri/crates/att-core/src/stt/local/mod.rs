//! 本地引擎的装配与常驻池（faster-whisper / Qwen3-ASR）。
//!
//! 默认 feature `local-engines` 打开时用 [`imp`]：进程内推理、模型常驻、按设置指纹重建；
//! 关闭时用 [`stub`]：接口保持可用，但所有本地引擎入口统一返回「此构建不含本地引擎」，
//! 让 CI / 纯数据面构建不必编译 CTranslate2 与 sherpa-onnx。

#[cfg(feature = "local-engines")]
mod imp;
#[cfg(feature = "local-engines")]
pub use imp::*;

#[cfg(not(feature = "local-engines"))]
mod stub;
#[cfg(not(feature = "local-engines"))]
pub use stub::*;
