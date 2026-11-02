//! 流式输入：把自动优化的结果边生成边「打」进当前焦点输入框（见 recording.rs 的 Optimizing）。
//!
//! - 按键在专用线程里按入队顺序发出，调用方（优化增量回调）只管入队、不阻塞；
//! - 换行不模拟回车（聊天类应用里回车 = 发送），改为写内部剪贴板后粘贴一个换行；
//! - 打出去的字收不回来：只打 `cleaning::stable_prefix` 认定稳定的部分，收尾时再与最终文本对齐；
//! - 被取消 / 被新一段录音取代时置 `halt`，线程丢弃剩余队列立即停手：每发几个字就看一眼，
//!   不会把手上这一整段打完（按住开始下一段时，上一段多打出去的字既进错输入框、也是给热键监听添乱的模拟按键）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;

/// 换行粘贴后的等待：目标应用读剪贴板是异步的，太快覆写剪贴板会粘进别的内容。
const NEWLINE_SETTLE: Duration = Duration::from_millis(150);
/// 每发出这么多字（或回删这么多次）就检查一次 halt。
const HALT_CHECK_CHARS: usize = 10;

enum Op {
    Text(String),
    Backspace(usize),
}

/// 一次自动优化的流式输入会话：已打出的文本 + 发键线程。
pub struct StreamTyping {
    tx: Option<mpsc::Sender<Op>>,
    worker: Option<JoinHandle<Result<(), String>>>,
    halt: Arc<AtomicBool>,
    typed: String,
}

impl StreamTyping {
    pub fn spawn(halt: Arc<AtomicBool>) -> Self {
        let (tx, rx) = mpsc::channel::<Op>();
        let worker_halt = halt.clone();
        let worker = std::thread::Builder::new()
            .name("att-typing".into())
            .spawn(move || {
                crate::desktop::ensure_typing_ready()?;
                for op in rx {
                    if worker_halt.load(Ordering::SeqCst) {
                        break;
                    }
                    match op {
                        Op::Text(text) => type_text(&text, &worker_halt)?,
                        Op::Backspace(count) => press_backspace(count, &worker_halt)?,
                    }
                }
                Ok(())
            })
            .map_err(|error| tracing::warn!("流式输入线程启动失败: {error}"))
            .ok();
        Self { tx: Some(tx), worker, halt, typed: String::new() }
    }

    /// 已入队打出的文本（换行计入）。
    pub fn typed(&self) -> &str {
        &self.typed
    }

    /// 优化增量回调：按目前的稳定前缀补打新增部分。
    pub fn advance(&mut self, stable: &str) {
        if stable.len() > self.typed.len() && stable.starts_with(self.typed.as_str()) {
            self.send(Op::Text(stable[self.typed.len()..].to_string()));
            self.typed = stable.to_string();
        }
    }

    /// 与最终文本对齐（分叉的尾巴回删、缺的补打）并等待全部按键发出（阻塞）。
    pub fn complete(mut self, final_text: &str) -> Result<(), String> {
        let (extra, rest) = reconcile(&self.typed, final_text);
        if extra > 0 {
            self.send(Op::Backspace(extra));
        }
        if !rest.is_empty() {
            self.send(Op::Text(rest.to_string()));
        }
        self.typed = final_text.to_string();
        self.join()
    }

    /// 不再补打：等已入队的按键发完（阻塞）。
    pub fn stop(mut self) -> Result<(), String> {
        self.join()
    }

    /// 立即停手：丢弃尚未发出的按键（阻塞到线程退出）。
    pub fn abort(mut self) {
        self.halt.store(true, Ordering::SeqCst);
        let _ = self.join();
    }

    fn send(&self, op: Op) {
        if let Some(tx) = &self.tx {
            // 线程已因错误退出时发送会失败，错误在 join 时统一返回
            let _ = tx.send(op);
        }
    }

    fn join(&mut self) -> Result<(), String> {
        self.tx.take();
        match self.worker.take() {
            Some(worker) => worker.join().unwrap_or_else(|_| Err("流式输入线程异常退出".into())),
            None => Err("流式输入线程未启动".into()),
        }
    }
}

fn type_text(text: &str, halt: &AtomicBool) -> Result<(), String> {
    let mut segments = text.split('\n').peekable();
    while let Some(segment) = segments.next() {
        for piece in pieces(segment.trim_end_matches('\r'), HALT_CHECK_CHARS) {
            if halt.load(Ordering::SeqCst) {
                return Ok(());
            }
            crate::desktop::type_unicode(piece)?;
        }
        if segments.peek().is_some() {
            if halt.load(Ordering::SeqCst) {
                return Ok(());
            }
            crate::desktop::write_clipboard("\n")?;
            crate::desktop::paste_active()?;
            std::thread::sleep(NEWLINE_SETTLE);
        }
    }
    Ok(())
}

fn press_backspace(count: usize, halt: &AtomicBool) -> Result<(), String> {
    let mut left = count;
    while left > 0 && !halt.load(Ordering::SeqCst) {
        let batch = left.min(HALT_CHECK_CHARS);
        crate::desktop::press_backspace(batch)?;
        left -= batch;
    }
    Ok(())
}

/// 按字符（不按字节）把文本切成至多 `size` 个字的小段。
fn pieces(text: &str, size: usize) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let end = rest.char_indices().nth(size).map(|(i, _)| i).unwrap_or(rest.len());
        let (piece, tail) = rest.split_at(end);
        rest = tail;
        Some(piece)
    })
}

/// 已打出 `typed`、目标是 `target`：返回需要回删的字符数与随后要补打的文本。
fn reconcile<'a>(typed: &str, target: &'a str) -> (usize, &'a str) {
    let common = typed.chars().zip(target.chars()).take_while(|(a, b)| a == b).count();
    let rest = target.char_indices().nth(common).map(|(i, _)| &target[i..]).unwrap_or("");
    (typed.chars().count() - common, rest)
}

#[cfg(test)]
mod tests {
    use super::{pieces, reconcile};

    #[test]
    fn 按字切小段_不拆多字节字符() {
        assert_eq!(pieces("我明天去开会。", 3).collect::<Vec<_>>(), ["我明天", "去开会", "。"]);
        assert_eq!(pieces("ab", 10).collect::<Vec<_>>(), ["ab"]);
        assert_eq!(pieces("", 10).count(), 0);
        assert_eq!(pieces("😀a😀", 1).collect::<Vec<_>>(), ["😀", "a", "😀"]);
    }

    #[test]
    fn 收尾对齐_补打_回删_分叉() {
        assert_eq!(reconcile("我明天", "我明天去开会。"), (0, "去开会。"), "只缺尾巴：补打");
        assert_eq!(reconcile("我明天去开会。", "我明天去开会。"), (0, ""), "已一致");
        assert_eq!(reconcile("我明天去开会。 ", "我明天去开会。"), (1, ""), "多出的尾巴回删");
        assert_eq!(reconcile("我明天去开会", "我明天开会。"), (3, "开会。"), "分叉处之后回删重打");
        assert_eq!(reconcile("", "全文"), (0, "全文"));
    }
}
