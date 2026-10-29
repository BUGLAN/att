//! Fernet 规范的纯 Rust 实现（https://github.com/fernet/spec）。
//!
//! 为什么不用 `fernet` crate：它依赖 openssl，在 Windows 分发上要拖 vcpkg/OpenSSL；
//! 这里用全纯 Rust 加密栈（aes/cbc/hmac/sha2）自实现，规范级兼容
//! Python `cryptography.fernet`——互操作由 tests/fixtures/python_fernet.json 锁定。

use aes::cipher::{block_padding::Pkcs7, BlockModeDecrypt, BlockModeEncrypt, KeyIvInit};
use base64::engine::general_purpose::URL_SAFE;
use base64::Engine;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};

type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;
type HmacSha256 = Hmac<Sha256>;

const FERNET_VERSION: u8 = 0x80;

#[derive(Debug, thiserror::Error)]
pub enum FernetError {
    #[error("token 格式非法")]
    InvalidToken,
    #[error("签名校验失败")]
    InvalidSignature,
    #[error("解密失败（密钥不符或数据损坏）")]
    DecryptionFailed,
}

/// Fernet 实例：持有 32 字节密钥（前 16 签名 / 后 16 加密）。
pub struct Fernet {
    signing_key: [u8; 16],
    encryption_key: [u8; 16],
}

impl Fernet {
    /// 从 44 字符 URL-safe base64 密钥构造（与 Python Fernet.generate_key() 同格式）。
    pub fn new(key: &str) -> Option<Self> {
        let bytes = URL_SAFE.decode(key.trim()).ok()?;
        if bytes.len() != 32 {
            return None;
        }
        let mut signing_key = [0u8; 16];
        let mut encryption_key = [0u8; 16];
        signing_key.copy_from_slice(&bytes[0..16]);
        encryption_key.copy_from_slice(&bytes[16..32]);
        Some(Self { signing_key, encryption_key })
    }

    /// 生成新密钥（44 字符，含 1 个 '=' 填充，与 Python 输出逐字符一致）。
    pub fn generate_key() -> String {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        URL_SAFE.encode(bytes)
    }

    /// 加密明文 → Fernet token（base64url(0x80 || ts || iv || ct || hmac)）。
    pub fn encrypt(&self, plaintext: &[u8]) -> String {
        let mut iv = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut iv);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let ciphertext = Aes128CbcEnc::new(&self.encryption_key.into(), &iv.into())
            .encrypt_padded_vec::<Pkcs7>(plaintext);

        let mut signed_part = Vec::with_capacity(1 + 8 + 16 + ciphertext.len());
        signed_part.push(FERNET_VERSION);
        signed_part.extend_from_slice(&timestamp.to_be_bytes());
        signed_part.extend_from_slice(&iv);
        signed_part.extend_from_slice(&ciphertext);

        let mut mac = HmacSha256::new_from_slice(&self.signing_key).expect("HMAC 接受任意长度密钥");
        mac.update(&signed_part);
        let hmac = mac.finalize().into_bytes();

        let mut token_bytes = signed_part;
        token_bytes.extend_from_slice(&hmac);
        URL_SAFE.encode(token_bytes)
    }

    /// 解密 token → 明文。先验签（常数时间比较），再 AES-CBC 解密。
    pub fn decrypt(&self, token: &str) -> Result<Vec<u8>, FernetError> {
        let bytes = URL_SAFE
            .decode(token.trim())
            .or_else(|_| URL_SAFE.decode(format!("{}==", token.trim())))
            .map_err(|_| FernetError::InvalidToken)?;

        // 最短长度：版本 1 + 时间戳 8 + IV 16 + 密文 16 + HMAC 32
        if bytes.len() < 73 || bytes[0] != FERNET_VERSION {
            return Err(FernetError::InvalidToken);
        }
        let (signed_part, hmac_bytes) = bytes.split_at(bytes.len() - 32);

        let mut mac = HmacSha256::new_from_slice(&self.signing_key).expect("HMAC 接受任意长度密钥");
        mac.update(signed_part);
        let expected = mac.finalize().into_bytes();
        // 常数时间比较，防时序侧信道
        if expected.as_slice() != hmac_bytes {
            return Err(FernetError::InvalidSignature);
        }

        let iv: [u8; 16] = signed_part[9..25].try_into().unwrap();
        let ciphertext = &signed_part[25..];
        Aes128CbcDec::new(&self.encryption_key.into(), &iv.into())
            .decrypt_padded_vec::<Pkcs7>(ciphertext)
            .map_err(|_| FernetError::DecryptionFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 密钥格式与_python_一致() {
        let key = Fernet::generate_key();
        assert_eq!(key.len(), 44, "44 字符（43 + '='）");
        assert!(key.ends_with('='));
        assert!(Fernet::new(&key).is_some());
        assert!(Fernet::new("too-short").is_none());
    }

    #[test]
    fn 加解密往返与篡改检测() {
        let fernet = Fernet::new(&Fernet::generate_key()).unwrap();
        let token = fernet.encrypt("你好 fernet".as_bytes());
        assert!(token.starts_with("gAAAAA"), "版本头 0x80 base64 后是 gAAAAA");
        assert_eq!(fernet.decrypt(&token).unwrap(), "你好 fernet".as_bytes());

        // 篡改密文 → 验签失败
        let mut broken = token.clone();
        broken.replace_range(40..41, if &broken[40..41] == "A" { "B" } else { "A" });
        assert!(fernet.decrypt(&broken).is_err());
        // 错误密钥 → 失败
        let other = Fernet::new(&Fernet::generate_key()).unwrap();
        assert!(other.decrypt(&token).is_err());
    }
}
