use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use pulldown_cmark::{html, Options, Parser};

pub fn render_markdown(markdown: &str) {
    render_markdown_to_terminal(markdown);
}

pub fn render_markdown_in_browser(markdown: &str) -> io::Result<()> {
    let temp_path = temp_html_path();
    let html = convert_md_to_html(markdown);
    fs::write(&temp_path, html)?;

    open_in_browser(&temp_path)?;
    Ok(())
}

fn convert_md_to_html(markdown: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>gg notes</title>
    <style>
        :root {{
            --bg-color: #ffffff;
            --text-color: #333333;
            --text-secondary: #666666;
            --primary-color: #3b78e7;
            --border-color: #eaecef;
            --code-bg: #f6f8fa;
            --code-border: #d0d7de;
            --blockquote-border: #d0d7de;
            --blockquote-bg: #f8f9fa;
        }}
        @media (prefers-color-scheme: dark) {{
            :root {{
                --bg-color: #0d1117;
                --text-color: #c9d1d9;
                --text-secondary: #9da7b3;
                --primary-color: #58a6ff;
                --border-color: #30363d;
                --code-bg: #161b22;
                --code-border: #30363d;
                --blockquote-border: #30363d;
                --blockquote-bg: #161b22;
            }}
        }}
        * {{
            box-sizing: border-box;
        }}
        body {{
            font-family: "Source Serif Pro", "PingFang SC", "Noto Serif", "Times New Roman", serif;
            font-size: 16px;
            line-height: 1.8;
            color: var(--text-color);
            background: var(--bg-color);
            margin: 0;
            padding: 24px;
        }}
        .container {{
            max-width: 960px;
            margin: 0 auto;
            padding: 32px 28px 80px;
        }}
        h1, h2, h3, h4, h5, h6 {{
            margin-top: 2rem;
            margin-bottom: 1rem;
            font-weight: 700;
            line-height: 1.35;
            color: var(--text-color);
        }}
        h1 {{
            font-size: 2.2em;
            padding-bottom: 0.3em;
            border-bottom: 1px solid var(--border-color);
        }}
        h2 {{
            font-size: 1.75em;
            padding-bottom: 0.3em;
            border-bottom: 1px solid var(--border-color);
        }}
        h3 {{ font-size: 1.5em; }}
        h4 {{ font-size: 1.25em; }}
        h5 {{ font-size: 1.1em; }}
        h6 {{ font-size: 1em; color: var(--text-secondary); }}
        p, blockquote, ul, ol, dl, table {{
            margin: 0.8rem 0;
        }}
        a {{
            color: var(--primary-color);
            text-decoration: none;
            border-bottom: 1px solid transparent;
            transition: all 0.2s ease;
        }}
        a:hover {{
            border-bottom-color: var(--primary-color);
        }}
        code {{
            font-family: "Maple Mono", "Fira Code", ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
            font-size: 0.9em;
            background: #fff5f5;
            color: #c0392b;
            padding: 0.1rem 0.4rem;
            margin: 0 0.2rem;
            border-radius: 4px;
            border: 1px solid rgba(238, 238, 238, 0.5);
            vertical-align: middle;
        }}
        pre {{
            background: var(--code-bg);
            border: 1px solid var(--border-color);
            border-radius: 8px;
            padding: 16px 16px 12px;
            overflow-x: auto;
            margin: 1.5rem 0;
            line-height: 1.6;
            box-shadow: 0 2px 6px rgba(0,0,0,0.02);
        }}
        pre code {{
            background: none;
            border: none;
            padding: 0;
            margin: 0;
            font-size: 0.9em;
            color: inherit;
        }}
        blockquote {{
            border-left: 4px solid var(--primary-color);
            padding: 10px 15px;
            margin-left: 0;
            margin-right: 0;
            color: var(--text-secondary);
            background-color: var(--blockquote-bg);
            border-radius: 0 4px 4px 0;
        }}
        ul, ol {{
            padding-left: 2em;
        }}
        li > p {{
            margin: 0.2rem 0;
        }}
        hr {{
            height: 1px;
            padding: 0;
            margin: 32px 0;
            background-color: var(--border-color);
            border: 0;
        }}
        table {{
            border-collapse: separate;
            border-spacing: 0;
            width: 100%;
            overflow: hidden;
            margin-bottom: 20px;
            border: 1px solid var(--border-color);
            border-radius: 6px;
            box-shadow: 0 1px 3px rgba(0,0,0,0.05);
        }}
        table thead tr {{
            background-color: #eef1f5;
        }}
        table th {{
            padding: 12px 16px;
            font-weight: 700;
            color: #24292f;
            background-color: #eef1f5;
            border-top: none;
            border-left: none;
            border-right: 1px solid #d0d7de;
            border-bottom: 2px solid #d0d7de;
            text-align: left;
        }}
        table td {{
            padding: 10px 16px;
            border-top: none;
            border-left: none;
            border-bottom: 1px solid var(--border-color);
            border-right: 1px solid var(--border-color);
            background-color: #ffffff;
            color: var(--text-color);
        }}
        table tbody tr:nth-child(2n) {{
            background-color: #f9fbfd;
        }}
        table tbody tr:nth-child(2n) td {{
            background-color: #f9fbfd;
        }}
        table tbody tr:hover td {{
            background-color: #f1f3f5;
        }}
        table th:last-child,
        table td:last-child {{
            border-right: none;
        }}
        table tr:last-child td {{
            border-bottom: none;
        }}
        img {{
            display: block;
            max-width: 100%;
            height: auto;
            margin: 1em auto;
            border-radius: 4px;
        }}
        .task-list-item {{
            list-style: none;
            margin-left: -1.3em;
        }}
        .task-list-item input {{
            margin-right: 0.6em;
        }}
        .command {{
            background: var(--code-bg);
            border: 1px solid var(--border-color);
            border-radius: 6px;
            padding: 12px 16px;
            font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
            font-size: 14px;
            overflow-x: auto;
        }}
        </style>
</head>
<body>
    <div class="container">
{body}
    </div>
</body>
</html>"#,
        body = markdown_to_html_body(markdown)
    )
}

fn markdown_to_html_body(markdown: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_HEADING_ATTRIBUTES);
    options.insert(Options::ENABLE_SMART_PUNCTUATION);

    let parser = Parser::new_ext(markdown, options);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);
    html_output
}

fn open_in_browser(path: &PathBuf) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        Command::new("cmd")
            .args(["/C", "start", "", &path.to_string_lossy()])
            .spawn()?;
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(path).spawn()?;
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        // Check if running in WSL (Windows Subsystem for Linux)
        let is_wsl = std::env::var("WSL_DISTRO_NAME").is_ok()
            || std::fs::read_to_string("/proc/version")
                .map(|v| v.to_lowercase().contains("microsoft"))
                .unwrap_or(false);

        if is_wsl {
            // In WSL, convert path to Windows format first
            let windows_path = Command::new("wslpath")
                .arg("-w")
                .arg(path)
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_else(|_| path.to_string_lossy().to_string());

            // Try wslview first if available (wslu)
            if let Ok(status) = Command::new("wslview").arg(&windows_path).status() {
                if status.success() {
                    return Ok(());
                }
            }

            // Prefer Windows PowerShell Start-Process for reliability in WSL
            if let Ok(status) = Command::new("powershell.exe")
                .args(["-NoProfile", "-Command", "Start-Process", &windows_path])
                .status()
            {
                if status.success() {
                    return Ok(());
                }
            }

            // PowerShell 7 (pwsh) fallback
            if let Ok(status) = Command::new("pwsh.exe")
                .args(["-NoProfile", "-Command", "Start-Process", &windows_path])
                .status()
            {
                if status.success() {
                    return Ok(());
                }
            }

            // Fallback to cmd.exe /c start
            if let Ok(status) = Command::new("cmd.exe")
                .args(["/C", "start", "", &windows_path])
                .status()
            {
                if status.success() {
                    return Ok(());
                }
            }

            // Try common absolute Windows paths if PATH interop is missing
            let powershell_path = "/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe";
            if let Ok(status) = Command::new(powershell_path)
                .args(["-NoProfile", "-Command", "Start-Process", &windows_path])
                .status()
            {
                if status.success() {
                    return Ok(());
                }
            }

            let cmd_path = "/mnt/c/Windows/System32/cmd.exe";
            if let Ok(status) = Command::new(cmd_path)
                .args(["/C", "start", "", &windows_path])
                .status()
            {
                if status.success() {
                    return Ok(());
                }
            }

            let explorer_path = "/mnt/c/Windows/explorer.exe";
            if let Ok(status) = Command::new(explorer_path)
                .arg(&windows_path)
                .status()
            {
                if status.success() {
                    return Ok(());
                }
            }

            return Err(io::Error::other(
                "WSL 无法打开 Windows 默认浏览器。建议尝试:\n\
                 1. 确认 WSL 互操作已开启 (WSL 2 推荐)\n\
                 2. 将 /mnt/c/Windows/System32 加入 PATH\n\
                 3. 安装 wslu 并使用 wslview\n\
                 4. 或在 Linux 端安装浏览器 (firefox, chromium)"
            ));
        }

        // Try common browsers on Linux

        let browsers = ["xdg-open", "gnome-open", "kde-open", "firefox", "chromium", "google-chrome"];
        for browser in browsers {
            if Command::new("which").arg(browser).output().map(|o| o.status.success()).unwrap_or(false) {
                Command::new(browser).arg(path).spawn()?;
                return Ok(());
            }
        }
        return Err(io::Error::other("未找到可用的浏览器命令"));
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        return Err(io::Error::other("不支持的操作系统"));
    }
}

fn temp_html_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    env::temp_dir().join(format!("gg-render-{}-{nanos}.html", std::process::id()))
}

fn render_markdown_to_terminal(markdown: &str) {
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

