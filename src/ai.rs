//! Claude 回退：通过 `claude -p` 生成 Markdown 笔记。

use crate::i18n::Language;
use crate::utils::debug_log;
use crate::utils::process::{Program, resolve_program, wait_with_timeout};
use anyhow::{Context, Result, anyhow, bail};
use std::env;
use std::io::Read;
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::Duration;

/// `claude --version` 探测的超时。
///
/// 探测是「未命中笔记」路径上的固定开销，必须快速返回：首次运行时的
/// 交互式登录提示或网络阻塞都可能让它长时间不返回。
const AVAILABILITY_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// AI 笔记生成端口。
pub trait NoteGenerator {
    /// 生成器是否可用（CLI 是否存在）。
    fn is_available(&self) -> bool;

    fn generate(&self, command: &str, language: &str) -> Result<String>;
}

pub struct ClaudeGenerator {
    lang: Language,
    /// `None` 表示不限制生成耗时。
    timeout: Option<Duration>,
}

impl ClaudeGenerator {
    pub fn new(lang: Language, timeout: Option<Duration>) -> Self {
        Self { lang, timeout }
    }

    fn spec() -> String {
        env::var("GG_CLAUDE_BIN")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "claude".to_string())
    }

    fn resolve(&self) -> Result<Program> {
        let spec = Self::spec();
        resolve_program(&spec).with_context(|| format!("无法定位 AI 命令行工具 `{spec}`"))
    }
}

impl NoteGenerator for ClaudeGenerator {
    fn is_available(&self) -> bool {
        let spec = Self::spec();
        let Ok(program) = resolve_program(&spec) else {
            debug_log!("ai::is_available: 未找到 `{spec}`");
            return false;
        };

        let Ok(mut child) = Command::new(&program.bin)
            .args(&program.args)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            debug_log!("ai::is_available: `{spec} --version` 启动失败");
            return false;
        };

        match wait_with_timeout(&mut child, AVAILABILITY_PROBE_TIMEOUT) {
            Ok(Some(status)) => status.success(),
            Ok(None) => {
                debug_log!(
                    "ai::is_available: `{spec} --version` 超过 {}s 未返回，已终止并视为不可用",
                    AVAILABILITY_PROBE_TIMEOUT.as_secs()
                );
                false
            }
            Err(err) => {
                debug_log!("ai::is_available: 等待 `{spec} --version` 出错: {err}");
                false
            }
        }
    }

    fn generate(&self, command: &str, language: &str) -> Result<String> {
        let program = self.resolve()?;
        debug_log!(
            "ai::generate: 调用 `{}` 生成 `{command}` 的笔记, language={language}, timeout={:?}",
            program.bin.display(),
            self.timeout
        );

        let mut child = Command::new(&program.bin)
            .args(&program.args)
            .arg("-p")
            .arg("--output-format")
            .arg("text")
            .arg(build_prompt(command, language))
            // stdout 与 stderr 都用内部管道：claude 的子进程会继承这两个句柄，
            // 若直通终端，超时被杀的孙进程会一直占着终端管道，导致调用方
            // 在读取输出时被拖住。内部管道则随 gg 退出而关闭。
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("无法调用 `{}`", program.describe()))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("无法获取 claude 的 stdout 管道"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("无法获取 claude 的 stderr 管道"))?;

        // 必须在等待退出的同时持续读取，否则输出填满管道缓冲区就会死锁。
        let stdout_reader = spawn_reader(stdout);
        let stderr_reader = spawn_reader(stderr);

        let status = match self.timeout {
            Some(timeout) => wait_with_timeout(&mut child, timeout)?,
            None => Some(child.wait().context("无法等待 claude 退出")?),
        };

        // 超时时直接返回，不 join 读取线程：被杀的只是直接子进程，留下的
        // 孙进程可能仍持有管道写端，read_to_end 会一直阻塞，令超时形同虚设。
        // 不引入 unsafe 就无法建立进程组一并杀死，这些进程与读取线程随
        // gg 退出一起消亡。
        let Some(status) = status else {
            let seconds = self.timeout.unwrap_or_default().as_secs();
            bail!("{}", self.lang.claude_timeout(seconds));
        };

        let stdout_bytes = join_reader(stdout_reader)?;
        let stderr_bytes = join_reader(stderr_reader)?;
        let stderr_text = String::from_utf8_lossy(&stderr_bytes).trim().to_string();

        if !stderr_text.is_empty() {
            debug_log!("ai::generate: claude stderr: {stderr_text}");
        }

        if !status.success() {
            let code = status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "signal".to_string());
            let detail = if stderr_text.is_empty() {
                format!("退出码 {code}")
            } else {
                format!("退出码 {code}: {stderr_text}")
            };
            bail!("{}", self.lang.claude_failed(&detail));
        }

        let content = String::from_utf8(stdout_bytes)
            .map_err(|_| anyhow!("{}", self.lang.claude_output_not_utf8()))?;
        let trimmed = content.trim();
        if trimmed.is_empty() {
            bail!("{}", self.lang.claude_empty_output());
        }

        Ok(trimmed.to_string())
    }
}

fn spawn_reader<R: Read + Send + 'static>(mut source: R) -> JoinHandle<std::io::Result<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        source.read_to_end(&mut buffer)?;
        Ok(buffer)
    })
}

fn join_reader(handle: JoinHandle<std::io::Result<Vec<u8>>>) -> Result<Vec<u8>> {
    handle
        .join()
        .map_err(|_| anyhow!("读取 claude 输出时线程异常退出"))?
        .context("无法读取 claude 输出")
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
