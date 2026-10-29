//! 存储层：JSON 原子写 / 宽容读 / 按资源路径的进程内锁 / Fernet 兼容加密。

pub mod crypto;
pub mod fernet;
pub mod json_store;
