//! 存储层：JSON 原子写 / 宽容读 / 按资源路径的进程内锁 / 一条一文件目录的内存索引 / Fernet 兼容加密。

pub mod crypto;
pub mod dir_index;
pub mod fernet;
pub mod json_store;
