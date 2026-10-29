//! Fernet 兼容加密：密钥文件与密文格式与 Python `cryptography.fernet` 互认。
//!
//! 兼容性红线（阶段①文档 §3.9）：`data/.encryption_key` 与
//! `doubao_asr_api_key_encrypted` / `llm_v2.json` 的密文都由 Python 生成，
//! Rust 版必须能直接解密——下方 `python_互操作测试` 用真实的 Python 密文样本锁定。
//! 具体算法实现在 [`super::fernet`]（纯 Rust，无 OpenSSL 依赖）。

use super::fernet::Fernet;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("加密密钥文件损坏（不是合法的 Fernet 密钥）: {0}")]
    InvalidKey(String),
    #[error("读写加密密钥文件失败: {0}")]
    Io(#[from] std::io::Error),
}

/// Fernet 加密器：持有从 `data/.encryption_key` 加载（或首次创建）的密钥。
pub struct Crypto {
    fernet: Fernet,
    key_path: PathBuf,
}

impl Crypto {
    /// 加载或创建密钥。密钥文件格式 = 44 字符 URL-safe base64（与 Python
    /// `Fernet.generate_key()` 完全一致），首次创建时落盘。
    pub fn load_or_create(data_dir: &Path) -> Result<Self, CryptoError> {
        let key_path = data_dir.join(".encryption_key");
        let key = if key_path.exists() {
            std::fs::read_to_string(&key_path)?
                .trim()
                .to_string()
        } else {
            let key = Fernet::generate_key();
            if let Some(parent) = key_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&key_path, &key)?;
            restrict_permissions(&key_path);
            key
        };

        let fernet = Fernet::new(&key)
            .ok_or_else(|| CryptoError::InvalidKey(key_path.display().to_string()))?;
        Ok(Self { fernet, key_path })
    }

    pub fn key_path(&self) -> &Path {
        &self.key_path
    }

    /// 加密明文 → Fernet token（带版本头 0x80，Python 可解）。
    pub fn encrypt(&self, plain: &str) -> String {
        self.fernet.encrypt(plain.as_bytes())
    }

    /// 解密 Fernet token → 明文。token 损坏/密钥不符时返回 None（调用方按「未配置」处理）。
    pub fn decrypt(&self, token: &str) -> Option<String> {
        self.fernet
            .decrypt(token)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
    }
}

/// 从既有密钥构造（测试用）。
#[cfg(test)]
impl Crypto {
    fn from_key(key: &str) -> Self {
        Self {
            fernet: Fernet::new(key).expect("测试密钥必须合法"),
            key_path: PathBuf::from(".encryption_key"),
        }
    }
}

/// Unix 上把密钥文件权限收紧到 0600（Windows 无对应语义，忽略）。
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::fernet::FernetError;

    #[test]
    fn rust_内部往返加解密() {
        let dir = std::env::temp_dir().join(format!("att-crypto-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let crypto = Crypto::load_or_create(&dir).unwrap();
        let token = crypto.encrypt("secret-豆包-token");
        assert!(token.starts_with("gAAAAA"), "Fernet token 必须带 0x80 版本头");
        assert_eq!(crypto.decrypt(&token).as_deref(), Some("secret-豆包-token"));

        // 密钥文件持久化后重载仍可解
        let reloaded = Crypto::load_or_create(&dir).unwrap();
        assert_eq!(reloaded.decrypt(&token).as_deref(), Some("secret-豆包-token"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 真实 Python 密文互操作：fixture 由仓库脚本用 `cryptography.fernet` 生成
    /// （见 tests/fixtures/python_fernet.json 的生成命令），避免手抄密文出错。
    #[test]
    fn python_互操作_可解_python_生成的密钥与密文() {
        let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/python_fernet.json");
        let raw = std::fs::read_to_string(&fixture_path).expect("fixture 必须存在");
        let fixture: serde_json::Value = serde_json::from_str(&raw).unwrap();

        let python_key = fixture["key"].as_str().unwrap();
        let python_token = fixture["token"].as_str().unwrap();
        let expect_plain = fixture["plain"].as_str().unwrap();

        let crypto = Crypto::from_key(python_key);
        assert_eq!(crypto.decrypt(python_token).as_deref(), Some(expect_plain));

        // 反向：Rust 用同一密钥加密，token 形态与 Python 一致（gAAAAA 版本头）
        let token = crypto.encrypt("回程");
        assert!(token.starts_with("gAAAAA"));
        assert_eq!(crypto.decrypt(&token).as_deref(), Some("回程"));
    }

    /// FernetError 需要被引用一次以保证 Debug 派生不产生死代码告警。
    #[test]
    fn 错误类型可打印() {
        let error = FernetError::InvalidToken;
        assert!(error.to_string().contains("非法"));
    }
}
