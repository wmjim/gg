//! Claude 回退：通过 `claude -p` 生成 Markdown 笔记。

use crate::i18n::Language;
use crate::utils::debug_log;
use crate::utils::process::resolve_program;
use anyhow::{Context, Result, bail};
use std::env;
use std::process::{Command, Stdio};

/// AI 笔记生成端口。
pub trait NoteGenerator {
    /// 生成器是否可用（CLI 是否存在）。
    fn is_available(&self) -> bool;

    fn generate(&self, command: &str, language: &str) -> Result<String>;
}

pub struct ClaudeGenerator {
    lang: Language,
}

impl ClaudeGenerator {
    pub fn new(lang: Language) -> Self {
        Self { lang }
    }

    fn spec() -> String {
        env::var("GG_CLAUDE_BIN")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "claude".to_string())
    }
}

impl NoteGenerator for ClaudeGenerator {
    fn is_available(&self) -> bool {
        let spec = Self::spec();
        let Ok(program) = resolve_program(&spec) else {
            debug_log!("ai::is_available: 未找到 `{spec}`");
            return false;
        };

        Command::new(&program.bin)
            .args(&program.args)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn generate(&self, command: &str, language: &str) -> Result<String> {
        let spec = Self::spec();
        let program =
            resolve_program(&spec).with_context(|| format!("无法定位 AI 命令行工具 `{spec}`"))?;

        debug_log!(
            "ai::generate: 调用 `{}` 生成 `{command}` 的笔记, language={language}",
            program.bin.display()
        );

        let output = Command::new(&program.bin)
            .args(&program.args)
            .arg("-p")
            .arg("--output-format")
            .arg("text")
            .arg(build_prompt(command, language))
            .output()
            .with_context(|| format!("无法调用 `{}`", program.describe()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("{}", self.lang.claude_failed(stderr.trim()));
        }

        let content = String::from_utf8(output.stdout)
            .map_err(|_| anyhow::anyhow!("{}", self.lang.claude_output_not_utf8()))?;
        let trimmed = content.trim();
        if trimmed.is_empty() {
            bail!("{}", self.lang.claude_empty_output());
        }

        Ok(trimmed.to_string())
    }
}

fn build_prompt(command: &str, language: &str) -> String {
    format!(
        "你是 Linux 命令笔记助手。请为命令 `{command}` 生成一份可读性高的 Markdown 速查笔记；\
         输出语言为 {language}；必须包含简介、常用参数、至少5个带说明的示例、注意事项；\
         常用参数可用无序列表或表格；示例命令必须使用带语言标记且闭合的代码块\
         （例如以 ```bash 开头并以 ``` 结束）；输出纯 Markdown，不要附加解释。"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_requires_every_requested_section() {
        let prompt = build_prompt("grep", "zh-CN");
        for expected in ["grep", "zh-CN", "简介", "常用参数", "注意事项", "```bash"] {
            assert!(
                prompt.contains(expected),
                "提示词缺少 `{expected}`: {prompt}"
            );
        }
    }
}
