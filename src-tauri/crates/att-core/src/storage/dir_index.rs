//! 「一条一文件」目录的内存索引：整目录只读一次，常驻内存，本进程写入时同步更新。
//!
//! 记录（`data/records/`）与剪贴板历史（`data/clipboard/`）都是一条一个 JSON 文件，
//! 列表 / 搜索 / 去重原先每次都要把整个目录读盘并解析一遍，条目一多就慢，而且是在持锁状态下做的。
//!
//! 失效判定用目录的修改时间：目录里新增、删除、替换文件（原子写的 tmp + rename、
//! 另一个仓库实例的写入、手工恢复备份）都会改变它，读取时发现与缓存时不一致就整体重扫。
//! 不覆盖的情形：外部程序原地改写某个文件的内容（目录时间不变）。本应用自己的写入一律走原子写，
//! 手工改了文件内容后重启应用即可。
//!
//! 本类型不带锁，由调用方放进自己的 `Mutex` 里，与「读-改-写」放在同一把锁下。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub struct DirIndex<T> {
    dir: PathBuf,
    items: Vec<T>,
    /// 扫描时跳过的损坏 / 非法条目数
    skipped: usize,
    /// 缓存对应的目录修改时间
    stamp: Option<SystemTime>,
    loaded: bool,
}

impl<T> DirIndex<T> {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, items: Vec::new(), skipped: 0, stamp: None, loaded: false }
    }

    fn dir_stamp(&self) -> Option<SystemTime> {
        std::fs::metadata(&self.dir).and_then(|m| m.modified()).ok()
    }

    /// 确保缓存是最新的：未加载、拿不到目录时间或目录有外部变化时调 `scan` 重读。
    /// `scan` 返回（条目, 跳过数）。
    pub fn refresh(&mut self, scan: impl FnOnce(&Path) -> (Vec<T>, usize)) {
        // 先取时间再扫描：扫描期间若有变化，记下的是旧时间，下次读取会再扫一次（宁可多扫，不漏）
        let stamp = self.dir_stamp();
        if self.loaded && stamp.is_some() && stamp == self.stamp {
            return;
        }
        let (items, skipped) = scan(&self.dir);
        self.items = items;
        self.skipped = skipped;
        self.stamp = stamp;
        self.loaded = true;
    }

    /// 当前缓存的条目（调用前先 [`refresh`](Self::refresh)）。
    pub fn items(&self) -> &[T] {
        &self.items
    }

    pub fn skipped(&self) -> usize {
        self.skipped
    }

    /// 写入前调用：上次核对之后目录有外部变化就作废缓存（下次读取重扫），
    /// 避免本次写入后记下的新时间把外部变化「盖」过去。
    pub fn before_write(&mut self) {
        if self.loaded && self.dir_stamp() != self.stamp {
            self.invalidate();
        }
    }

    /// 写入成功后调用：缓存有效时用 `apply` 同步这次改动，并记下写入后的目录时间。
    /// 缓存还没加载过就什么都不做（下次读取自然会扫到）。
    pub fn after_write(&mut self, apply: impl FnOnce(&mut Vec<T>)) {
        if !self.loaded {
            return;
        }
        apply(&mut self.items);
        self.stamp = self.dir_stamp();
    }

    pub fn invalidate(&mut self) {
        self.loaded = false;
        self.items = Vec::new();
        self.skipped = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("att-dir-index-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn scan_names(dir: &Path, counter: &Cell<usize>) -> (Vec<String>, usize) {
        counter.set(counter.get() + 1);
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        (names, 0)
    }

    #[test]
    fn 目录未变时不重扫_本进程写入同步缓存() {
        let dir = temp_dir("own");
        std::fs::write(dir.join("a.json"), "{}").unwrap();
        let scans = Cell::new(0);
        let mut index = DirIndex::new(dir.clone());

        index.refresh(|d| scan_names(d, &scans));
        index.refresh(|d| scan_names(d, &scans));
        assert_eq!(scans.get(), 1, "目录没变不应重扫");
        assert_eq!(index.items(), ["a.json"]);

        index.before_write();
        std::fs::write(dir.join("b.json"), "{}").unwrap();
        index.after_write(|items| items.push("b.json".into()));
        index.refresh(|d| scan_names(d, &scans));
        assert_eq!(scans.get(), 1, "本进程写入后缓存已同步，不应重扫");
        assert_eq!(index.items(), ["a.json", "b.json"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 外部增删文件后重扫() {
        let dir = temp_dir("external");
        std::fs::write(dir.join("a.json"), "{}").unwrap();
        let scans = Cell::new(0);
        let mut index = DirIndex::new(dir.clone());
        index.refresh(|d| scan_names(d, &scans));

        // 目录时间精度可能是秒级（部分文件系统），等到时间确实前进再改
        let before = std::fs::metadata(&dir).unwrap().modified().unwrap();
        loop {
            std::fs::write(dir.join("probe.tmp"), "").unwrap();
            std::fs::remove_file(dir.join("probe.tmp")).unwrap();
            if std::fs::metadata(&dir).unwrap().modified().unwrap() != before {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        std::fs::write(dir.join("c.json"), "{}").unwrap();
        index.refresh(|d| scan_names(d, &scans));
        assert_eq!(scans.get(), 2, "外部新增文件后必须重扫");
        assert_eq!(index.items(), ["a.json", "c.json"]);

        // 写入前发现外部变化：作废缓存，after_write 不再基于旧缓存打补丁
        std::fs::remove_file(dir.join("a.json")).unwrap();
        index.before_write();
        index.after_write(|items| items.push("never.json".into()));
        index.refresh(|d| scan_names(d, &scans));
        assert_eq!(index.items(), ["c.json"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
