//! 标准输出写入工具。
//!
//! Rust 的 `println!` 在写入遇 `EPIPE` 时会 panic。对 CLI 来说这完全正常：
//! `gg list | head -5` 就是下游提前关闭管道。这里统一吸收该错误，让管道场景
//! 像标准 Unix 工具一样安静退出，而不是抛出一段 panic 堆栈。

use crate::i18n::Language;
use anyhow::{Context, Result};
use std::io::{self, Write};

pub fn is_broken_pipe(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::BrokenPipe
}

/// 逐行写入输出流；下游提前关闭管道时视为正常结束。
pub fn write_lines<W, I>(mut out: W, lines: I, lang: Language) -> Result<()>
where
    W: Write,
    I: IntoIterator<Item = String>,
{
    for line in lines {
        if let Err(err) = writeln!(out, "{line}") {
            return map_io_error(err, lang);
        }
    }
    if let Err(err) = out.flush() {
        return map_io_error(err, lang);
    }
    Ok(())
}

/// 原样写入文本；下游提前关闭管道时视为正常结束。
pub fn write_text<W: Write>(mut out: W, text: &str, lang: Language) -> Result<()> {
    if let Err(err) = out.write_all(text.as_bytes()) {
        return map_io_error(err, lang);
    }
    if let Err(err) = out.flush() {
        return map_io_error(err, lang);
    }
    Ok(())
}

/// `BrokenPipe` 视为正常结束，其余 IO 错误原样上报。
fn map_io_error(err: io::Error, lang: Language) -> Result<()> {
    if is_broken_pipe(&err) {
        return Ok(());
    }
    Err(err).context(lang.stdout_write_failed())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_every_line_with_trailing_newline() {
        let mut buffer = Vec::new();
        write_lines(
            &mut buffer,
            vec!["a".to_string(), "b".to_string()],
            Language::Zh,
        )
        .expect("写入成功");
        assert_eq!(String::from_utf8(buffer).expect("utf8"), "a\nb\n");
    }

    #[test]
    fn write_text_preserves_content_verbatim() {
        let mut buffer = Vec::new();
        write_text(&mut buffer, "# ls\nno trailing newline", Language::Zh).expect("写入成功");
        assert_eq!(
            String::from_utf8(buffer).expect("utf8"),
            "# ls\nno trailing newline"
        );
    }

    /// 读端关闭后写入必然得到 `BrokenPipe`，此处必须被吸收。
    #[cfg(unix)]
    #[test]
    fn closed_pipe_is_not_an_error() {
        use std::os::unix::net::UnixStream;

        let (reader, writer) = UnixStream::pair().expect("创建 socketpair");
        drop(reader);

        let mut writer = writer;
        write_lines(&mut writer, vec!["x".repeat(1024 * 1024)], Language::Zh)
            .expect("下游关闭管道不应被当作失败");

        let text_result = write_text(&mut writer, &"y".repeat(1024 * 1024), Language::Zh);
        assert!(text_result.is_ok(), "实际: {text_result:?}");
    }

    #[test]
    fn other_io_errors_are_still_reported() {
        struct AlwaysFails;

        impl Write for AlwaysFails {
            fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("磁盘炸了"))
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let err = write_text(AlwaysFails, "x", Language::Zh).expect_err("真实 IO 错误必须上报");
        assert!(format!("{err:#}").contains("磁盘炸了"), "实际: {err:#}");
    }

    /// IO 错误文案跟随界面语言，且不应把「无法写入标准输出」叠两遍。
    #[test]
    fn write_errors_are_localized_once() {
        struct AlwaysFails;

        impl Write for AlwaysFails {
            fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("boom"))
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let err = write_text(AlwaysFails, "x", Language::En).expect_err("必须上报");
        let message = format!("{err:#}");
        assert_eq!(
            message.matches("Failed to write to stdout").count(),
            1,
            "{message}"
        );
    }
}
