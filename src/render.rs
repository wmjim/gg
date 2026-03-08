use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn render_markdown(markdown: &str) {
    if let Err(err) = render_with_glow(markdown) {
        eprintln!("glow 渲染失败（{err}），已回退为原始 Markdown 输出。");
        print_raw(markdown);
    }
}

fn render_with_glow(markdown: &str) -> io::Result<()> {
    let bin = glow_bin();

    match run_glow_stdin(&bin, markdown) {
        Ok(()) => Ok(()),
        Err(stdin_err) => match run_glow_file(&bin, markdown) {
            Ok(()) => Ok(()),
            Err(file_err) => Err(io::Error::other(format!(
                "stdin 模式失败：{stdin_err}; 文件模式失败：{file_err}"
            ))),
        },
    }
}

fn run_glow_stdin(bin: &OsString, markdown: &str) -> io::Result<()> {
    let mut child = Command::new(bin)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|err| map_spawn_error(bin, err))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(markdown.as_bytes())?;
    }

    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "glow exited with status {status} (args: -)"
        )))
    }
}

fn run_glow_file(bin: &OsString, markdown: &str) -> io::Result<()> {
    let temp_path = temp_markdown_path();
    fs::write(&temp_path, markdown)?;

    let status_result = Command::new(bin)
        .arg(&temp_path)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|err| map_spawn_error(bin, err));

    let _ = fs::remove_file(&temp_path);

    let status = status_result?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "glow exited with status {status} (file: {})",
            temp_path.display()
        )))
    }
}

fn map_spawn_error(bin: &OsString, err: io::Error) -> io::Error {
    match err.kind() {
        io::ErrorKind::NotFound => io::Error::other(format!(
            "未找到 glow 可执行文件 `{}`，请先安装 glow 或设置 GG_GLOW_BIN",
            bin.to_string_lossy()
        )),
        _ => err,
    }
}

fn glow_bin() -> OsString {
    env::var_os("GG_GLOW_BIN").unwrap_or_else(|| OsString::from("glow"))
}

fn temp_markdown_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    env::temp_dir().join(format!("gg-render-{}-{nanos}.md", std::process::id()))
}

fn print_raw(markdown: &str) {
    print!("{markdown}");
    if !markdown.ends_with('\n') {
        println!();
    }
}
