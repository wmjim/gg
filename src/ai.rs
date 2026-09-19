//! AI 笔记生成。
//!
//! 后端不再写死 claude：命令行由 [`AiCommand`] 从配置模板解析，
//! 提示词默认追加为最后一个参数，也可用 `{prompt}` 指定插入位置。

use crate::config::AiInvocation;
use crate::i18n::Language;
use crate::utils::debug_log;
use crate::utils::process::{resolve_program, wait_with_timeout};
use anyhow::{Context, Result, anyhow, bail};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::Duration;

/// 提示词占位符，用于把提示词插到参数中间。
const PROMPT_PLACEHOLDER: &str = "{prompt}";

/// AI 笔记生成端口。
pub trait NoteGenerator {
    /// 供 UI 展示的后端描述（例如 `claude -p --output-format text`）。
    fn description(&self) -> String;

    /// 后端是否可用（可执行文件能否定位）。
    fn is_available(&self) -> bool;

    fn generate(&self, command: &str, language: &str) -> Result<String>;
}

/// 配置为 `ai_provider = "none"` 时的空实现。
pub struct DisabledGenerator;

impl NoteGenerator for DisabledGenerator {
    fn description(&self) -> String {
        "none".to_string()
    }

    fn is_available(&self) -> bool {
        false
    }

    fn generate(&self, _command: &str, _language: &str) -> Result<String> {
        bail!("AI 回退已关闭")
    }
}

/// 一条解析完成的 AI 命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiCommand {
    bin: PathBuf,
    /// 提示词之前的参数。
    before: Vec<String>,
    /// 提示词之后的参数。
    after: Vec<String>,
}

impl AiCommand {
    /// 解析命令行模板。
    ///
    /// 含 `{prompt}` 时按该位置插入提示词，否则追加为最后一个参数。
    pub fn parse(spec: &str) -> Result<Self> {
        let spec = spec.trim();
        if spec.is_empty() {
            bail!("AI 命令行不能为空");
        }

        match spec.split_once(PROMPT_PLACEHOLDER) {
            Some((head, tail)) => {
                let program = resolve_program(head.trim()).with_context(|| {
                    format!("无法解析 `{head}`（`{PROMPT_PLACEHOLDER}` 之前的命令）")
                })?;
                let after = shell_words::split(tail).with_context(|| {
                    format!("无法解析 `{tail}`（`{PROMPT_PLACEHOLDER}` 之后的参数）")
                })?;
                Ok(Self {
                    bin: program.bin,
                    before: program.args,
                    after,
                })
            }
            None => {
                let program = resolve_program(spec)?;
                Ok(Self {
                    bin: program.bin,
                    before: program.args,
                    after: Vec::new(),
                })
            }
        }
    }

    /// 生成带提示词参数的 `Command`。
    pub fn command_for(&self, prompt: &str) -> Command {
        let mut command = Command::new(&self.bin);
        command.args(&self.before).arg(prompt).args(&self.after);
        command
    }

    pub fn describe(&self) -> String {
        let mut parts = vec![self.bin.display().to_string()];
        parts.extend(self.before.iter().cloned());
        parts.extend(self.after.iter().cloned());
        parts.join(" ")
    }
}

pub struct AiNoteGenerator {
    lang: Language,
    /// `None` 表示不限制生成耗时。
    timeout: Option<Duration>,
    spec: String,
}

impl AiNoteGenerator {
    pub fn from_invocation(
        lang: Language,
        timeout: Option<Duration>,
        invocation: &AiInvocation,
    ) -> Box<dyn NoteGenerator> {
        match invocation {
            AiInvocation::Disabled => Box::new(DisabledGenerator),
            AiInvocation::Command(spec) => Box::new(Self {
                lang,
                timeout,
                spec: spec.clone(),
            }),
        }
    }

    fn resolve(&self) -> Result<AiCommand> {
        AiCommand::parse(&self.spec).with_context(|| {
            format!(
                "无法定位 AI 命令行工具（ai_command / ai_provider 的实际命令为 `{}`）",
                self.spec
            )
        })
    }
}

impl NoteGenerator for AiNoteGenerator {
    fn description(&self) -> String {
        match AiCommand::parse(&self.spec) {
            Ok(command) => command.describe(),
            Err(_) => self.spec.clone(),
        }
    }

    /// 只做可执行文件定位，不探测 `--version`：既避免版本探测挂住，
    /// 也避免为一次未命中付出进程启动开销。
    fn is_available(&self) -> bool {
        match self.resolve() {
            Ok(_) => true,
            Err(err) => {
                debug_log!("ai::is_available: {err:#}");
                false
            }
        }
    }

    fn generate(&self, command: &str, language: &str) -> Result<String> {
        let ai_command = self.resolve()?;
        debug_log!(
            "ai::generate: 执行 `{}` 生成 `{command}` 的笔记, language={language}, timeout={:?}",
            ai_command.describe(),
            self.timeout
        );

        let prompt = build_prompt(command, language);
        let mut child = ai_command
            .command_for(&prompt)
            // stdout 与 stderr 都用内部管道：子进程会继承这两个句柄，
            // 若直通终端，超时被杀的孙进程会一直占着调用方的管道。
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("无法调用 `{}`", ai_command.describe()))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("无法获取 stdout 管道"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("无法获取 stderr 管道"))?;

        // 必须在等待退出的同时持续读取，否则输出填满管道缓冲区就会死锁。
        let stdout_reader = spawn_reader(stdout);
        let stderr_reader = spawn_reader(stderr);

        let status = match self.timeout {
            Some(timeout) => wait_with_timeout(&mut child, timeout)?,
            None => Some(child.wait().context("无法等待 AI 命令退出")?),
        };

        // 超时时直接返回，不 join 读取线程：被杀的只是直接子进程，留下的
        // 孙进程可能仍持有管道写端，read_to_end 会一直阻塞。
        let Some(status) = status else {
            let seconds = self.timeout.unwrap_or_default().as_secs();
            bail!("{}", self.lang.ai_timeout(seconds));
        };

        let stdout_bytes = join_reader(stdout_reader)?;
        let stderr_bytes = join_reader(stderr_reader)?;
        let stderr_text = String::from_utf8_lossy(&stderr_bytes).trim().to_string();

        if !stderr_text.is_empty() {
            debug_log!("ai::generate: stderr: {stderr_text}");
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
            bail!("{}", self.lang.ai_failed(&detail));
        }

        let content = String::from_utf8(stdout_bytes)
            .map_err(|_| anyhow!("{}", self.lang.ai_output_not_utf8()))?;
        let trimmed = sanitize_generated_note(&content);
        if trimmed.trim().is_empty() {
            bail!("{}", self.lang.ai_empty_output());
        }

        Ok(trimmed)
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
        .map_err(|_| anyhow!("读取 AI 输出时线程异常退出"))?
        .context("无法读取 AI 输出")
}

/// 去掉模型习惯性加上的标题，并清理行尾空格与多余空行。
///
/// 手写笔记（`ls.md` / `pwd.md`）都以定义直接开头，`# xxx 命令速查` 这类
/// 标题只会挤占篇幅。提示词已要求不要写标题，这里再兜一层，保证结果稳定。
pub fn sanitize_generated_note(markdown: &str) -> String {
    let lines: Vec<&str> = markdown.lines().map(str::trim_end).collect();

    let mut start = 0;
    if let Some(index) = lines.iter().position(|line| !line.trim().is_empty()) {
        start = if lines[index].trim_start().starts_with("# ") {
            index + 1
        } else {
            index
        };
    }

    let mut output = String::new();
    for line in &lines[start..] {
        if line.trim().is_empty() {
            // 折叠开头与连续的空行
            if output.is_empty() || output.ends_with("\n\n") {
                continue;
            }
        }
        output.push_str(line);
        output.push('\n');
    }

    while output.ends_with("\n\n") {
        output.pop();
    }

    output
}

fn build_prompt(command: &str, language: &str) -> String {
    format!(
        "你是命令行速查笔记编辑。为命令 `{command}` 输出一份 Markdown 速查笔记。\
         直接输出笔记正文，不要任何前言、说明或收尾语。\n\
         \n\
         必须严格采用以下结构，不要增删章节：\n\
         1. 第一行：`命令名`：一句话说明它做什么，40 字以内。\n\
         2. 空行后，用无序列表只列 2~4 个最高频的选项，每项形如 `-x`：用途。低频选项不要写。\n\
         3. 空行后，给 1~3 个 ```bash 代码块，每个代码块只放一条可直接运行的命令；必要时在其上方加一行以 # 开头的说明。\n\
         4. 只有当输出格式需要解释时（例如 `ls -l` 长格式各字段的含义），才在最后补 3~6 行简短说明。\n\
         \n\
         硬性约束：\n\
         - 全文 10~25 行，宁少勿多。\n\
         - 不要出现 简介/语法/常用参数/示例/注意事项/总结 这类小节标题。\n\
         - 不要用表格，除非多个选项需要横向对照。\n\
         - 不要介绍命令的历史、来源、所属项目；不要用「强大的」「常用的」「非常重要」这类形容。\n\
         - 不要 emoji，不要「总之」「综上」，不要客套或鼓励式语气，不要重复已说过的信息。\n\
         - 命令示例必须真实可运行，不要写行尾空格。\n\
         - 输出语言：{language}。\n\
         \n\
         风格参考（别人手写的 pwd 笔记，只示意结构与信息密度，不要照抄内容）：\n\
         `pwd`：显示**当前工作目录**的路径，即显示所在位置的**绝对路径**。\n\
         \n\
         ```bash\n\
         # 查看当前工作目录路径\n\
         $ pwd\n\
         /home/wm\n\
         ```\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_requires_the_concise_structure() {
        let prompt = build_prompt("grep", "zh-CN");

        for expected in [
            "grep",
            "zh-CN",
            "10~25 行",
            "2~4 个最高频的选项",
            "1~3 个 ```bash 代码块",
            "不要出现 简介/语法/常用参数/示例/注意事项/总结 这类小节标题",
            "不要 emoji",
            "不要介绍命令的历史、来源、所属项目",
        ] {
            assert!(
                prompt.contains(expected),
                "提示词缺少 `{expected}`:\n{prompt}"
            );
        }
    }

    /// 旧提示词要求「简介 + 常用参数 + 至少 5 个示例 + 注意事项」，
    /// 正是内容膨胀的根源，回归时不要把它加回来。
    #[test]
    fn prompt_no_longer_demands_at_least_five_examples() {
        let prompt = build_prompt("ls", "zh-CN");
        assert!(!prompt.contains("至少5个"), "不应再要求至少 5 个示例");
    }

    #[test]
    fn prompt_is_usable_for_any_language() {
        assert!(build_prompt("ls", "en").contains("en"));
    }

    #[test]
    fn ai_command_appends_prompt_by_default() {
        let command = AiCommand::parse("sh -c echo").expect("解析成功");
        assert_eq!(command.before, vec!["-c".to_string(), "echo".to_string()]);
        assert!(command.after.is_empty());
        assert!(command.describe().contains("sh"));
    }

    #[test]
    fn ai_command_honours_prompt_placeholder() {
        let command = AiCommand::parse("sh -c {prompt} --flag").expect("解析成功");
        assert_eq!(command.before, vec!["-c".to_string()]);
        assert_eq!(command.after, vec!["--flag".to_string()]);
    }

    #[test]
    fn ai_command_rejects_empty_spec() {
        assert!(AiCommand::parse("   ").is_err());
    }

    #[test]
    fn ai_command_reports_missing_binary() {
        let err = AiCommand::parse("__gg_no_such_ai__").expect_err("必须失败");
        assert!(format!("{err:#}").contains("__gg_no_such_ai__"), "{err:#}");
    }

    #[test]
    fn sanitize_drops_leading_title_and_trailing_whitespace() {
        let raw = "# grep 命令速查\n\n`grep`：搜索文本。  \n\n\n\n```bash\ngrep x f\n```\n\n";

        let cleaned = sanitize_generated_note(raw);
        assert_eq!(cleaned, "`grep`：搜索文本。\n\n```bash\ngrep x f\n```\n");
    }

    #[test]
    fn sanitize_keeps_content_that_has_no_title() {
        let raw = "`pwd`：显示当前工作目录。\n\n```bash\n$ pwd\n```\n";
        assert_eq!(sanitize_generated_note(raw), raw);
    }

    /// 只有「首个非空行是一级标题」才去掉，正文里的小标题要保留。
    #[test]
    fn sanitize_keeps_inner_headings() {
        let raw = "`ls`：列出文件。\n\n## 输出格式\n\n从左至右依次是权限、所有者。\n";
        let cleaned = sanitize_generated_note(raw);
        assert!(cleaned.contains("## 输出格式"), "{cleaned}");
    }

    #[test]
    fn sanitize_handles_empty_input() {
        assert_eq!(sanitize_generated_note(""), "");
        assert_eq!(sanitize_generated_note("\n\n  \n"), "");
        assert_eq!(sanitize_generated_note("# 只有标题\n"), "");
    }
}
