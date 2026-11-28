//! 原子文件与 JSON 持久化原语：语义与 Python 版 `app/repositories/json_store.py` 对齐。
//!
//! - [`write_bytes_atomic`]：同目录 tmp → fsync → rename，二进制文件与密钥共用。
//! - [`write_json_atomic`]：同目录 tmp → fsync → rename（Windows 上
//!   `std::fs::rename` 走 MoveFileEx(REPLACE_EXISTING)，同卷原子覆盖）。
//!   任何一步失败都清理临时文件，目标文件要么是旧内容要么是完整新内容。
//! - [`read_json_lenient`]：缺失/损坏一律返回默认值（绝不让半截 JSON 卡死启动）。
//! - [`with_resource_lock`]：按规范化资源路径划分的进程内互斥锁（读-改-写串行化）。

use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// 全局「资源路径 → 锁」注册表：保证同一文件在整个进程内只有一个写序列。
fn lock_registry() -> &'static Mutex<HashMap<PathBuf, Arc<Mutex<()>>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 获取（或创建）某个资源路径专属的互斥锁。
pub fn resource_lock(path: &Path) -> Arc<Mutex<()>> {
    // 规范化键：Path::components 会消掉 `./` 与重复分隔符
    let mut key = PathBuf::new();
    for comp in path.components() {
        key.push(comp.as_os_str());
    }
    let mut registry = lock_registry().lock().unwrap_or_else(|e| e.into_inner());
    registry.entry(key).or_insert_with(|| Arc::new(Mutex::new(()))).clone()
}

/// 在资源锁内执行闭包（毒锁自愈：被 panic 污染的锁直接复用）。
pub fn with_resource_lock<T>(path: &Path, f: impl FnOnce() -> T) -> T {
    let lock = resource_lock(path);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    f()
}

/// 原子写 JSON（pretty 格式，与 Python 版 indent=2 对齐）。
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    write_json_atomic_with(path, value, |_| Ok(()))
}

/// 原子写 JSON，并允许在 rename 前调整临时文件（例如先收紧密钥权限）。
pub fn write_json_atomic_with<T: Serialize>(
    path: &Path,
    value: &T,
    prepare: impl FnOnce(&Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    with_resource_lock(path, || write_json_atomic_locked_with(path, value, prepare))
}

/// 已持有资源锁时的写入口（供「锁内读-改-写」组合使用，避免双重加锁）。
pub fn write_json_atomic_locked<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    write_json_atomic_locked_with(path, value, |_| Ok(()))
}

/// 已持有资源锁时的可注入写入入口。
pub fn write_json_atomic_locked_with<T: Serialize>(
    path: &Path,
    value: &T,
    prepare: impl FnOnce(&Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let data = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    write_bytes_atomic_locked_with(path, &data, prepare)
}

/// 原子写任意字节（同目录 tmp → fsync → rename）。
pub fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_bytes_atomic_with(path, bytes, |_| Ok(()))
}

/// 原子写任意字节，并允许在 rename 前调整临时文件。
pub fn write_bytes_atomic_with(
    path: &Path,
    bytes: &[u8],
    prepare: impl FnOnce(&Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    with_resource_lock(path, || write_bytes_atomic_locked_with(path, bytes, prepare))
}

/// 流式写同目录临时文件；回调返回并 fsync 成功后才原子替换目标文件。
pub fn write_file_atomic<T>(
    path: &Path,
    write: impl FnOnce(&std::fs::File) -> std::io::Result<T>,
) -> std::io::Result<T> {
    write_file_atomic_with(path, write, |_| Ok(()))
}

/// 流式原子写，并允许在 rename 前调整临时文件。
pub fn write_file_atomic_with<T>(
    path: &Path,
    write: impl FnOnce(&std::fs::File) -> std::io::Result<T>,
    prepare: impl FnOnce(&Path) -> std::io::Result<()>,
) -> std::io::Result<T> {
    with_resource_lock(path, || write_file_atomic_locked_with(path, write, prepare))
}

/// 已持有资源锁时原子写任意字节。
pub fn write_bytes_atomic_locked(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_bytes_atomic_locked_with(path, bytes, |_| Ok(()))
}

/// 已持有资源锁时的底层原子写。写入与 fsync 全部完成后才 rename。
pub fn write_bytes_atomic_locked_with(
    path: &Path,
    bytes: &[u8],
    prepare: impl FnOnce(&Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    write_file_atomic_locked_with(path, |mut file| file.write_all(bytes), prepare)
}

/// 已持有资源锁时的流式原子写。
pub fn write_file_atomic_locked<T>(
    path: &Path,
    write: impl FnOnce(&std::fs::File) -> std::io::Result<T>,
) -> std::io::Result<T> {
    write_file_atomic_locked_with(path, write, |_| Ok(()))
}

/// 已持有资源锁时的底层流式原子写。
pub fn write_file_atomic_locked_with<T>(
    path: &Path,
    write: impl FnOnce(&std::fs::File) -> std::io::Result<T>,
    prepare: impl FnOnce(&Path) -> std::io::Result<()>,
) -> std::io::Result<T> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = sibling_tmp_path(path);
    let result = (|| {
        let file = std::fs::File::create(&tmp)?;
        let value = write(&file)?;
        file.sync_all()?;
        prepare(&tmp)?;
        std::fs::rename(&tmp, path)?;
        Ok(value)
    })();
    if result.is_err() {
        // 失败必须清理临时文件，避免积累半截残留
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// 同目录临时文件路径：`settings.json` → `settings.json.tmp`。
fn sibling_tmp_path(path: &Path) -> PathBuf {
    let file_name =
        path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "data.json".to_string());
    path.parent().unwrap_or_else(|| Path::new(".")).join(format!("{file_name}.tmp"))
}

/// 宽容读：文件不存在 / 内容损坏 → 一律返回默认值。
pub fn read_json_lenient<T: Default + DeserializeOwned>(path: &Path) -> T {
    match read_json_file::<T>(path) {
        Ok(value) => value,
        Err(error) => {
            if path.exists() {
                tracing::warn!("JSON 解析失败，已回落默认值: {} ({error})", path.display());
            }
            T::default()
        }
    }
}

/// 严格读：损坏时返回错误（调用方决定兜底策略）。
///
/// 开头的 UTF-8 BOM 会被剥掉：Windows 记事本保存的 JSON 带 BOM，serde_json 不认，
/// 原样解析会被当成损坏文件整份回落默认值。
pub fn read_json_file<T: DeserializeOwned>(path: &Path) -> std::io::Result<T> {
    let raw = std::fs::read_to_string(path)?;
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
    serde_json::from_str(raw).map_err(std::io::Error::other)
}

/// 顶层非对象收敛为 `{}`（对应 Python `ensure_dict`：`null`/`[]`/字符串都按空对象处理）。
pub fn ensure_object(value: serde_json::Value) -> serde_json::Value {
    if value.is_object() {
        value
    } else {
        serde_json::Value::Object(serde_json::Map::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn 原子写_覆盖旧内容且不留临时文件() {
        let dir = std::env::temp_dir().join(format!("att-test-atomic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("data.json");

        write_json_atomic(&path, &json!({"a": 1})).unwrap();
        write_json_atomic(&path, &json!({"a": 2, "b": "x"})).unwrap();

        let loaded: serde_json::Value = read_json_file(&path).unwrap();
        assert_eq!(loaded["a"], json!(2));
        assert!(!dir.join("data.json.tmp").exists(), "临时文件必须被 rename 掉");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 原子写_提交前失败保留旧内容并清理临时文件() {
        let dir = std::env::temp_dir().join(format!("att-test-atomic-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("data.bin");
        std::fs::write(&path, b"old").unwrap();

        let result = write_bytes_atomic_with(&path, b"new", |_| Err(std::io::Error::other("注入失败")));

        assert!(result.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        assert!(!dir.join("data.bin.tmp").exists(), "失败后必须清理临时文件");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 宽容读_损坏文件回落默认值() {
        let dir = std::env::temp_dir().join(format!("att-test-lenient-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.json");
        std::fs::write(&path, "{ 这不是合法 JSON").unwrap();

        // Value 的默认值是 Null（对象收敛交给 ensure_object，与 Python
        // read_json_lenient + ensure_dict 的分工一致）
        let value: serde_json::Value = read_json_lenient(&path);
        assert!(value.is_null(), "损坏 JSON 应回落 T::default()（Value 即 Null）");
        assert!(ensure_object(value).as_object().unwrap().is_empty(), "ensure_object 再收敛为空对象");

        // 不存在的文件同样回落
        let missing: serde_json::Value = read_json_lenient(&dir.join("missing.json"));
        assert!(missing.is_null());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_object_顶层形状收敛() {
        assert!(ensure_object(json!(null)).as_object().unwrap().is_empty());
        assert!(ensure_object(json!([1, 2])).as_object().unwrap().is_empty());
        assert_eq!(ensure_object(json!({"k": 1}))["k"], json!(1));
    }

    #[test]
    fn 资源锁_规范化后同一路径共享同一把锁() {
        let a = resource_lock(Path::new("some/./nested/x.json"));
        let b = resource_lock(Path::new("some/nested/x.json"));
        assert!(Arc::ptr_eq(&a, &b), "`./` 必须被规范化，同一路径共享锁");
    }

    #[test]
    fn 严格读_剥掉_utf8_bom() {
        let dir = std::env::temp_dir().join(format!("att-json-bom-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bom.json");
        std::fs::write(&path, "\u{feff}{\"a\": 1}").unwrap();
        let value: serde_json::Value = read_json_file(&path).unwrap();
        assert_eq!(value["a"], 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
