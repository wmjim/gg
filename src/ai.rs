use anyhow::{Context, Result, bail};
use std::env;
use std::ffi::OsString;
use std::process::{Command, Stdio};

pub fn is_claude_available() -> bool {
    let bin = claude_bin();
    Command::new(bin)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub fn generate_note_with_claude(command: &str, language: &str) -> Result<String> {
    let prompt = build_prompt(command, language);
    let bin = claude_bin();
    let output = Command::new(&bin)
        .arg("-p")
        .arg("--output-format")
        .arg("text")
        .arg(prompt)
        .output()
        .with_context(|| format!("Failed to invoke {:?}", bin))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Claude query failed: {}", stderr.trim());
    }

    let content = String::from_utf8(output.stdout).context("Claude output is not valid UTF-8")?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        bail!("Claude returned empty content");
    }

    Ok(trimmed.to_string())
}

fn claude_bin() -> OsString {
    env::var_os("GG_CLAUDE_BIN").unwrap_or_else(|| OsString::from("claude"))
}

fn build_prompt(command: &str, language: &str) -> String {
    format!(
        "你是 Linux 命令笔记助手。请为命令 `{command}` 生成一份可读性高的 Markdown 速查笔记；输出语言为 {language}；必须包含简介、常用参数、至少5个带说明的示例、注意事项；输出纯 Markdown，不要附加解释。"
    )
}
