//! 单实例的第二道防线：进程级文件锁。
//!
//! single-instance 插件在 macOS 上靠 `/tmp/<identifier>_si.sock` 这个路径判断「已有实例」：
//! 套接字文件一旦被删（系统清理 /tmp、别的实例退出时按路径清理），正在运行的实例仍在监听那个
//! 已经没有名字的套接字，新进程连不上就认定自己是唯一实例。结果两个进程同时监听全局热键：
//! 按一次录音，两边各自识别、各自输出——一边流式打字、一边整段粘贴，同一句话进输入框两遍
//! （开发态 `tauri dev` 与已安装版并存时实测出现）。
//!
//! 这里在应用数据目录（开发态与安装版、各版本共用，不随 `ATT_DATA_DIR` 变）放一个锁文件，
//! 启动时独占加锁。锁跟着打开的文件走、进程退出由系统释放：不怕文件被删后残留，也不会有过期锁。
//! 拿不到锁说明另一个实例还活着（插件没能通知到它），本进程直接退出，不再注册热键。

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

const LOCK_FILE: &str = ".instance.lock";
const RETRY_INTERVAL: Duration = Duration::from_millis(100);
/// 应用内更新后重启时，旧进程拉起新进程后才退出：留一点重叠时间，别把更新后的新进程误判成二次启动。
pub(crate) const DEFAULT_WAIT: Duration = Duration::from_secs(3);

/// 持有期间锁一直有效；随应用状态（`app.manage`）存活到进程退出。
pub(crate) struct InstanceLock {
    _file: File,
}

/// 在 `dir` 下加单实例锁，最多等 `wait`。`Ok(None)`：另一个实例持有锁。
pub(crate) fn acquire(dir: &Path, wait: Duration) -> io::Result<Option<InstanceLock>> {
    std::fs::create_dir_all(dir)?;
    let file = OpenOptions::new().create(true).truncate(false).write(true).open(dir.join(LOCK_FILE))?;
    let deadline = Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(InstanceLock { _file: file })),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => std::thread::sleep(RETRY_INTERVAL),
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 已有实例持锁时拿不到_释放后可再拿() {
        let dir = std::env::temp_dir().join(format!("att-instance-lock-{}", uuid::Uuid::new_v4()));
        let first = acquire(&dir, Duration::ZERO).unwrap().expect("第一个实例应拿到锁");
        assert!(acquire(&dir, Duration::ZERO).unwrap().is_none(), "第二个实例不应拿到锁");
        drop(first);
        assert!(acquire(&dir, Duration::ZERO).unwrap().is_some(), "上一个实例退出后应能拿到锁");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 等待期内旧实例退出_新实例拿到锁() {
        let dir = std::env::temp_dir().join(format!("att-instance-lock-{}", uuid::Uuid::new_v4()));
        let old = acquire(&dir, Duration::ZERO).unwrap().unwrap();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            drop(old);
        });
        assert!(acquire(&dir, Duration::from_secs(2)).unwrap().is_some(), "更新重启的重叠期内应等到锁");
        releaser.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
