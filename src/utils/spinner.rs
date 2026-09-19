//! 等待外部 AI 返回时的转圈动画。
//!
//! 只写 stderr，且仅在 stderr 是终端时生效；`Drop` 时保证擦除，
//! 因此早期返回或 panic 都不会在终端留下半行字符。

use std::io::{self, IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const FRAME_INTERVAL: Duration = Duration::from_millis(80);

/// 擦除整行并把光标移回行首。
const CLEAR_LINE: &str = "\r\x1b[2K";

const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

pub struct Spinner {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    active: bool,
}

impl Spinner {
    /// 启动动画。`enabled = false` 时退化为空操作，由调用方自行输出静态提示。
    pub fn start(label: impl Into<String>, enabled: bool) -> Self {
        if !enabled {
            return Self {
                stop: Arc::new(AtomicBool::new(true)),
                handle: None,
                active: false,
            };
        }

        let label = label.into();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            let started = Instant::now();
            let mut stderr = io::stderr();
            let mut index = 0usize;

            while !worker_stop.load(Ordering::Relaxed) {
                let frame = FRAMES[index % FRAMES.len()];
                let _ = write!(
                    stderr,
                    "{CLEAR_LINE}{}",
                    frame_text(frame, &label, started.elapsed())
                );
                let _ = stderr.flush();
                index += 1;
                thread::sleep(FRAME_INTERVAL);
            }
        });

        Self {
            stop,
            handle: Some(handle),
            active: true,
        }
    }

    /// 是否真的在显示动画 —— 调用方据此决定是否还需要打印静态提示。
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// 停止动画并擦除该行。
    pub fn stop(&mut self) {
        if !self.active {
            return;
        }

        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }

        let mut stderr = io::stderr();
        let _ = write!(stderr, "{CLEAR_LINE}");
        let _ = stderr.flush();
        self.active = false;
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 是否适合显示动画：stderr 是终端，且不是 `TERM=dumb`。
pub fn stderr_supports_animation() -> bool {
    io::stderr().is_terminal()
        && std::env::var("TERM")
            .map(|term| term != "dumb")
            .unwrap_or(true)
}

fn frame_text(frame: char, label: &str, elapsed: Duration) -> String {
    format!("{frame} {label} [{}s]", elapsed.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_text_includes_label_and_elapsed_seconds() {
        assert_eq!(
            frame_text('⠋', "正在生成", Duration::from_secs(5)),
            "⠋ 正在生成 [5s]"
        );
        assert_eq!(frame_text('⠙', "x", Duration::ZERO), "⠙ x [0s]");
    }

    #[test]
    fn frames_are_distinct_and_non_empty() {
        assert!(FRAMES.len() > 1);
        assert!(!FRAMES.contains(&' '));
        let mut sorted = FRAMES;
        sorted.sort_unstable();
        let unique = sorted.windows(2).all(|pair| pair[0] != pair[1]);
        assert!(unique, "帧序列存在重复: {FRAMES:?}");
    }

    #[test]
    fn disabled_spinner_is_inactive_and_stop_is_noop() {
        let mut spinner = Spinner::start("不该显示", false);
        assert!(!spinner.is_active(), "非终端下不应显示动画");
        spinner.stop();
        spinner.stop();
    }

    /// 动画结束后必须回到「未激活」，否则 Drop 会重复擦除。
    #[test]
    fn stop_is_idempotent() {
        let mut spinner = Spinner::start("测试", true);
        assert!(spinner.is_active());
        spinner.stop();
        assert!(!spinner.is_active());
        spinner.stop();
        assert!(!spinner.is_active());
    }
}
