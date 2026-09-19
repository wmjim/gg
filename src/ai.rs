//! AI 笔记生成。
//!
//! 后端不再写死 claude：命令行由 [`AiCommand`] 从配置模板解析，
//! 提示词默认追加为最后一个参数，也可用 `{prompt}` 指定插入位置。

use crate::config::AiInvocation;
use crate::i18n::Language;
use crate::utils::debug_log;
use crate::utils::process::{Program, resolve_program, split_command_line, wait_with_timeout};
use anyhow::{Context, Result, anyhow, bail};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::Duration;

/// 提示词占位符，用于把提示词插到参数中间。
const PROMPT_PLACEHOLDER: &str = "{prompt}";

/// 内置提示词。用户可以在配置目录放一份 `AGENTS.md` 覆盖它。
const DEFAULT_PROMPT: &str = include_str!("assets/default_prompt.md");

/// AI 笔记生成端口。
pub trait NoteGenerator {
    /// 供 UI 展示的后端描述（例如 `claude -p --output-format text`）。
    fn description(&self) -> String;

    /// 后端是否可用（可执行文件能否定位）。
    fn is_available(&self) -> bool;

    fn generate(&self, command: &str, language: &str) -> Result<String>;
}

/// 配置为 `ai_provider = "none"` 时的空实现。
pub struct DisabledGenerator {
    lang: Language,
}

impl DisabledGenerator {
    pub fn new(lang: Language) -> Self {
        Self { lang }
    }
}

impl NoteGenerator for DisabledGenerator {
    fn description(&self) -> String {
        "none".to_string()
    }

    fn is_available(&self) -> bool {
        false
    }

    fn generate(&self, _command: &str, _language: &str) -> Result<String> {
        bail!("{}", self.lang.ai_disabled_generation())
    }
}

/// 一条解析完成的 AI 命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiCommand {
    /// 可执行文件 + 提示词之前的参数。
    program: Program,
    /// 提示词之后的参数。
    after: Vec<String>,
}

impl AiCommand {
    /// 解析命令行模板。
    ///
    /// 含 `{prompt}` 时按该位置插入提示词，否则追加为最后一个参数。
    pub fn parse(spec: &str, lang: Language) -> Result<Self> {
        let spec = spec.trim();
        if spec.is_empty() {
            bail!("{}", lang.ai_command_empty());
        }

        match spec.split_once(PROMPT_PLACEHOLDER) {
            Some((head, tail)) => {
                let program = resolve_program(head.trim(), lang)
                    .with_context(|| lang.ai_command_head_parse_failed(head, PROMPT_PLACEHOLDER))?;
                let after = split_command_line(tail, lang)
                    .with_context(|| lang.ai_command_tail_parse_failed(tail, PROMPT_PLACEHOLDER))?;
                Ok(Self { program, after })
            }
            None => Ok(Self {
                program: resolve_program(spec, lang)?,
                after: Vec::new(),
            }),
        }
    }

    /// 生成带提示词参数的 `Command`。
    /// 生成带提示词参数的 `Command`。
    ///
    /// 走 `Program::command()` 而不是自己 `Command::new(bin)`：Windows 上
    /// `.cmd` 后端必须经 `cmd.exe` 转发，这里不能绕过那层处理。
    pub fn command_for(&self, prompt: &str) -> Command {
        let mut command = self.program.command();
        command.arg(prompt).args(&self.after);
        command
    }

    pub fn describe(&self) -> String {
        let mut parts = vec![self.program.describe()];
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
            AiInvocation::Disabled => Box::new(DisabledGenerator::new(lang)),
            AiInvocation::Command(spec) => Box::new(Self {
                lang,
                timeout,
                spec: spec.clone(),
            }),
        }
    }

    fn resolve(&self) -> Result<AiCommand> {
        AiCommand::parse(&self.spec, self.lang)
            .with_context(|| self.lang.ai_resolve_failed(&self.spec))
    }

    /// 本次生成要用的提示词模板：配置目录下的 `AGENTS.md`，
    /// 不存在或为空时用内置默认。
    fn prompt_template(&self) -> Result<String> {
        match crate::config::prompt_path(self.lang) {
            Ok(path) => load_prompt_template(&path, self.lang),
            // 配置目录定位不到，也就没有用户模板可用。
            Err(_) => Ok(DEFAULT_PROMPT.to_string()),
        }
    }
}

impl NoteGenerator for AiNoteGenerator {
    fn description(&self) -> String {
        match AiCommand::parse(&self.spec, self.lang) {
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

        let prompt = build_prompt(command, language, &self.prompt_template()?);
        let mut child = ai_command
            .command_for(&prompt)
            // stdout 与 stderr 都用内部管道：子进程会继承这两个句柄，
            // 若直通终端，超时被杀的孙进程会一直占着调用方的管道。
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| self.lang.program_invoke_failed(&ai_command.describe()))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("{}", self.lang.ai_pipe_missing("stdout")))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("{}", self.lang.ai_pipe_missing("stderr")))?;

        // 必须在等待退出的同时持续读取，否则输出填满管道缓冲区就会死锁。
        let stdout_reader = spawn_reader(stdout);
        let stderr_reader = spawn_reader(stderr);

        let status = match self.timeout {
            Some(timeout) => wait_with_timeout(&mut child, timeout)?,
            None => Some(child.wait().context(self.lang.ai_wait_failed())?),
        };

        // 超时时直接返回，不 join 读取线程：被杀的只是直接子进程，留下的
        // 孙进程可能仍持有管道写端，read_to_end 会一直阻塞。
        let Some(status) = status else {
            let seconds = self.timeout.unwrap_or_default().as_secs();
            bail!("{}", self.lang.ai_timeout(seconds));
        };

        let stdout_bytes = join_reader(stdout_reader, self.lang)?;
        let stderr_bytes = join_reader(stderr_reader, self.lang)?;
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
                self.lang.ai_exit_code(&code)
            } else {
                self.lang.ai_exit_code_with_stderr(&code, &stderr_text)
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

fn join_reader(handle: JoinHandle<std::io::Result<Vec<u8>>>, lang: Language) -> Result<Vec<u8>> {
    handle
        .join()
        .map_err(|_| anyhow!("{}", lang.ai_reader_thread_failed()))?
        .context(lang.ai_output_read_failed())
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

/// 把命令与输出语言填进提示词模板。
///
/// `{{language}}` 先替换、`{{command}}` 最后替换：命令名允许包含除空白与
/// 路径分隔符之外的任意字符，若先填命令名，名字里的 `{{language}}` 会被二次
/// 展开（`note_template.html` 的 `{{body}}` 出于同样原因放在最后替换）。
fn build_prompt(command: &str, language: &str, template: &str) -> String {
    template
        .replace("{{language}}", language)
        .replace("{{command}}", command)
}

/// 读取用户自定义的提示词模板。
///
/// 路径不存在、或内容只有空白时回落到内置默认 —— 空模板等于让模型收到一条
/// 没有任何指令的请求，几乎一定是误操作。其余读取错误（权限等）直接上报：
/// 用户已经显式定制了它，静默忽略比报错更糟。
fn load_prompt_template(path: &Path, lang: Language) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(raw) if !raw.trim().is_empty() => Ok(raw),
        Ok(_) => {
            debug_log!(
                "ai::load_prompt_template: {} 为空，改用内置提示词",
                path.display()
            );
            Ok(DEFAULT_PROMPT.to_string())
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(DEFAULT_PROMPT.to_string()),
        Err(err) => Err(err).with_context(|| lang.prompt_read_failed(&path.display().to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_requires_the_concise_structure() {
        let prompt = build_prompt("grep", "zh-CN", DEFAULT_PROMPT);

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

    /// 内置模板必须带齐两个占位符，否则命中注释里的“删掉占位符”就要重建默认。
    #[test]
    fn default_prompt_carries_both_placeholders() {
        assert!(
            DEFAULT_PROMPT.contains("{{command}}"),
            "缺少 {{{{command}}}}"
        );
        assert!(
            DEFAULT_PROMPT.contains("{{language}}"),
            "缺少 {{{{language}}}}"
        );
    }

    /// 旧提示词要求「简介 + 常用参数 + 至少 5 个示例 + 注意事项」，
    /// 正是内容膨胀的根源，回归时不要把它加回来。
    #[test]
    fn prompt_no_longer_demands_at_least_five_examples() {
        let prompt = build_prompt("ls", "zh-CN", DEFAULT_PROMPT);
        assert!(!prompt.contains("至少5个"), "不应再要求至少 5 个示例");
    }

    #[test]
    fn prompt_is_usable_for_any_language() {
        assert!(build_prompt("ls", "en", DEFAULT_PROMPT).contains("en"));
    }

    /// 自定义模板完全替换内置内容，占位符按位置展开。
    #[test]
    fn custom_template_replaces_the_builtin_one() {
        let template = "请为 `{{command}}` 写笔记，输出语言 {{language}}。";
        let prompt = build_prompt("grep", "en", template);

        assert_eq!(prompt, "请为 `grep` 写笔记，输出语言 en。");
        assert!(!prompt.contains("10~25 行"), "不应混入内置模板内容");
    }

    /// 命令名可以合法地包含 `{{language}}` 这种字面量，不能因此被二次展开。
    #[test]
    fn command_name_is_substituted_last() {
        let template = "命令={{command}} 语言={{language}}";
        let prompt = build_prompt("{{language}}", "en", template);

        assert_eq!(prompt, "命令={{language}} 语言=en");
    }

    #[test]
    fn missing_prompt_file_falls_back_to_the_builtin_default() {
        let temp = tempfile::tempdir().expect("tempdir");
        let missing = temp.path().join("AGENTS.md");

        let template = load_prompt_template(&missing, Language::Zh).expect("缺失不算错误");
        assert_eq!(template, DEFAULT_PROMPT, "缺失时应回落到内置默认");
    }

    #[test]
    fn prompt_file_overrides_the_builtin_default() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("AGENTS.md");
        fs::write(&path, "自定义提示词 {{command}}\n").expect("写提示词");

        let template = load_prompt_template(&path, Language::Zh).expect("读取成功");
        assert_eq!(template, "自定义提示词 {{command}}\n");
    }

    /// 空文件几乎一定是误操作，应回落到内置默认，而不是发一条没有任何指令的请求。
    #[test]
    fn blank_prompt_file_falls_back_to_the_builtin_default() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("AGENTS.md");
        fs::write(&path, "   \n\n").expect("写空提示词");

        let template = load_prompt_template(&path, Language::Zh).expect("空文件不算错误");
        assert_eq!(template, DEFAULT_PROMPT);
    }

    /// 用临时假可执行文件而不是 `sh`：`sh` 在 Windows 上并非必然存在，
    /// 依赖它会让这些纯解析用例随平台飘。
    fn fake_bin_spec(temp: &tempfile::TempDir, tail: &str) -> String {
        let bin = crate::utils::process::test_support::write_fake_bin(temp.path(), "ai");
        format!("{} {tail}", bin.display())
    }

    #[test]
    fn ai_command_appends_prompt_by_default() {
        let temp = tempfile::tempdir().expect("tempdir");
        let command =
            AiCommand::parse(&fake_bin_spec(&temp, "-c echo"), Language::Zh).expect("解析成功");

        assert_eq!(
            command.program.args,
            vec!["-c".to_string(), "echo".to_string()]
        );
        assert!(command.after.is_empty(), "无占位符时提示词追加在末尾");
        assert!(command.describe().contains("ai"), "{}", command.describe());
    }

    #[test]
    fn ai_command_honours_prompt_placeholder() {
        let temp = tempfile::tempdir().expect("tempdir");
        let command = AiCommand::parse(&fake_bin_spec(&temp, "-c {prompt} --flag"), Language::Zh)
            .expect("解析成功");

        assert_eq!(command.program.args, vec!["-c".to_string()]);
        assert_eq!(command.after, vec!["--flag".to_string()]);
    }

    #[test]
    fn ai_command_rejects_empty_spec() {
        assert!(AiCommand::parse("   ", Language::Zh).is_err());
    }

    #[test]
    fn ai_command_reports_missing_binary() {
        let err = AiCommand::parse("__gg_no_such_ai__", Language::Zh).expect_err("必须失败");
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
