//! 应用编排层：解析配置 → 分派动作 → 组装依赖。
//!
//! 查询逻辑收敛到 [`QueryService`]，通过 [`QueryDeps`] 注入渲染器、提示器、
//! AI 生成器与编辑器，使「非交互场景不得触发 AI / 落盘」这类规则可被单测覆盖。

use crate::ai::{AiNoteGenerator, NoteGenerator};
use crate::cli::{Action, Cli};
use crate::config::{self, AiInvocation, AppConfig};
use crate::editor::{self, EditorLauncher, SystemEditor};
use crate::error;
use crate::i18n::Language;
use crate::notes;
use crate::prompt::{ConsolePrompter, Prompter};
use crate::render::{MarkdownRenderer, OutputTarget, Renderer};
use crate::utils::debug_log;
use crate::utils::output;
use crate::utils::{layout, spinner};
use anyhow::{Context, Result};
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const SUGGESTION_LIMIT: usize = 5;

/// 未命中提示与相近建议的文案。
///
/// 两条消息拼在同一行（`未找到命令 \`lz\`，推荐:`），只把建议列表放到下一行；
/// 没有相近命令时用完整句子收尾，不留悬空的分隔符。
fn miss_message(command: &str, suggestions: &[String], lang: Language) -> String {
    if suggestions.is_empty() {
        return lang.note_not_found_alone(command);
    }
    format!(
        "{}{}",
        lang.note_not_found(command),
        lang.did_you_mean(&suggestions.join(", "))
    )
}

/// 拿不到真实终端宽度时的兜底列数。
const DEFAULT_TERMINAL_WIDTH: usize = 80;

/// 「没有产生任何结果」的退出码，便于脚本判断（`gg foo || echo "没笔记"`）。
///
/// 覆盖两种情况：查询未命中笔记，以及操作因缺少用户确认而未执行。
/// 「命令行用法错误」（2）与「运行时错误」（1）的判定见 [`crate::error`]。
pub const EXIT_NOTE_NOT_FOUND: u8 = 3;

pub fn run(cli: Cli, lang: Language) -> Result<ExitCode> {
    let parts = cli.into_parts();
    let notes_dir = config::resolve_notes_dir(parts.notes_dir, lang)?;
    let mut config = AppConfig::load(lang)?;

    // --set-editor / --lang 只写配置，本身不构成一次请求：写完后继续执行本次
    // 动作（`gg --lang en list` 必须既保存语言又列出笔记）。早期实现在这里
    // 直接 return，子命令会被静默吞掉。
    let config_only = matches!(parts.action, Action::None)
        && (parts.set_editor.is_some() || parts.lang.is_some());

    // 先应用 `--lang` 再应用 `--set-editor`：这样两条确认信息都用用户本次要求的
    // 语言输出（否则 `gg --lang en --set-editor X` 会打印中文的编辑器确认），
    // 且 `--lang` 非法时直接退出，不会留下「编辑器已写入、语言却报错」的半成品。
    if let Some(raw) = parts.lang {
        let Some(chosen) = Language::parse(&raw) else {
            return Err(error::usage(config.language().invalid_language(&raw)));
        };
        config.language = Some(chosen);
        config.save(chosen)?;
        eprintln!("{}", chosen.saved_language_config());
    }

    if let Some(editor) = parts.set_editor {
        // 取值定位不到可执行文件时提前告知：拼写错误应在这里暴露，而不是等
        // 下次 `gg -e`。但不阻断写入 —— 运行时本就会回退到环境变量/系统默认。
        let problem = editor::spec_problem(&editor, lang);
        config.editor = Some(editor);
        config.save(lang)?;
        eprintln!("{}", config.language().saved_editor_config());
        if let Some(reason) = problem {
            eprintln!("{}", config.language().set_editor_unusable(&reason));
        }
    }

    // 没有别的动作时，写配置就是本次的目的：不再走首次运行引导，也不打印帮助。
    if config_only {
        return Ok(ExitCode::SUCCESS);
    }

    let prompter = ConsolePrompter::new(config.language());
    if config.is_first_run() && prompter.is_interactive() {
        let chosen = ask_language(&prompter)?;
        config.language = Some(chosen);
        config.save(chosen)?;
        eprintln!("{}", chosen.saved_language_config());
    }

    let lang = config.language();

    match parts.action {
        Action::List => {
            let found = notes::scan_commands(&notes_dir, lang)?;
            warn_skipped(&found.skipped, lang);
            write_commands(&found.items, lang)?;
            Ok(ExitCode::SUCCESS)
        }
        Action::Remove(commands) => Ok(remove_notes(
            &notes_dir, &commands, &prompter, parts.yes, lang,
        )?
        .exit_code()
        .map_or(ExitCode::SUCCESS, ExitCode::from)),
        Action::Search { keyword, content } => {
            // 空关键词几乎总是脚本失误（`gg search $var` 而变量为空）。必须明确
            // 报错：不拦的话，文件名搜索会因 `contains("")` 恒真而列出全部，
            // 正文搜索却返回零条——同一个输入得到相反的语义。
            if keyword.trim().is_empty() {
                return Err(error::usage(lang.search_keyword_empty()));
            }

            let matched = if content {
                let found = notes::search_notes_by_content(&notes_dir, &keyword, lang)?;
                warn_skipped(&found.skipped, lang);
                let lines = found
                    .items
                    .iter()
                    .map(notes::ContentMatch::render)
                    .collect::<Vec<_>>();
                output::write_lines(io::stdout().lock(), lines, lang)?;
                !found.items.is_empty()
            } else {
                let found = notes::search_commands_by_name(&notes_dir, &keyword, lang)?;
                warn_skipped(&found.skipped, lang);
                write_commands(&found.items, lang)?;
                !found.items.is_empty()
            };

            // 与 `grep` 一致：搜索是一次带条件的查询，无命中即「没有产生任何
            // 结果」，返回 3，便于 `gg search foo || echo 没找到`。`list` 是
            // 枚举而非查询，空目录仍按成功处理。
            //
            // 原因写 stderr：stdout 必须保持干净，否则 `gg search x > out.txt`
            // 会把提示写进文件。两种模式的建议不同——只有名称搜索才该提示
            // 「加 `-c` 搜正文」，在正文模式下再提一次就是错误建议。
            Ok(if matched {
                ExitCode::SUCCESS
            } else {
                let message = if content {
                    lang.search_no_match_content(&keyword)
                } else {
                    lang.search_no_match_name(&keyword)
                };
                eprintln!("{message}");
                ExitCode::from(EXIT_NOTE_NOT_FOUND)
            })
        }
        Action::Query(command) => {
            if config.is_first_run() {
                // 非交互首次运行：默认中文并落盘，避免后续每次都判空。
                config.language = Some(lang);
                config.save(lang).ok();
                eprintln!("{}", lang.first_run_default_note());
            }

            let renderer = MarkdownRenderer::new(lang);
            let editor = SystemEditor::from_config(&config);
            let generator = AiNoteGenerator::from_invocation(
                lang,
                config.ai_timeout(),
                &config.ai_invocation(),
            );
            let deps = QueryDeps {
                renderer: &renderer,
                editor: &editor,
                generator: generator.as_ref(),
                prompter: &prompter,
            };
            let service = QueryService::new(&notes_dir, &config, &deps);
            let options = QueryOptions {
                target: if parts.browser {
                    OutputTarget::Browser
                } else {
                    OutputTarget::Terminal
                },
                edit: parts.edit,
                assume_yes: parts.yes,
            };

            Ok(service
                .query(&command, options)?
                .exit_code()
                .map_or(ExitCode::SUCCESS, ExitCode::from))
        }
        Action::None => {
            if config.is_first_run() {
                config.language = Some(lang);
                config.save(lang).ok();
                eprintln!("{}", lang.first_run_default_note());
            }
            print_help(lang)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// clap 的 `print_help` 同样会在下游关闭管道时报错，这里一并吸收。
fn print_help(lang: Language) -> Result<()> {
    if let Err(err) = crate::cli::command(lang).print_help() {
        if !crate::utils::output::is_broken_pipe(&err) {
            return Err(err).context(lang.help_print_failed());
        }
    }
    Ok(())
}

/// 输出命令名列表。
///
/// 只在 stdout 是终端时排成多列（对齐 `ls` 的行为），管道与重定向仍为
/// 每行一条，保证 `gg list | grep x` 这类脚本不受影响。
fn write_commands(commands: &[String], lang: Language) -> Result<()> {
    let stdout = io::stdout();
    if stdout.is_terminal() {
        let rendered = layout::format_columns(commands, terminal_width());
        return output::write_text(stdout.lock(), &rendered, lang);
    }
    output::write_lines(stdout.lock(), commands.to_vec(), lang)
}

/// 终端宽度：没有可靠的跨平台 std API，只能用 `COLUMNS`，否则按 80 列。
fn terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|width| *width > 0)
        .unwrap_or(DEFAULT_TERMINAL_WIDTH)
}

/// 扫描时被跳过的条目必须让用户看到，否则会误以为列表是完整的。
fn warn_skipped(skipped: &[notes::Skipped], lang: Language) {
    if skipped.is_empty() {
        return;
    }

    eprintln!("{}", lang.notes_skipped(&skipped.len().to_string()));
    for entry in skipped {
        debug_log!(
            "app::warn_skipped: 跳过 {} —— {}",
            entry.path.display(),
            entry.reason
        );
    }
}

/// 删除动作的结果，决定退出码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoveOutcome {
    /// 目标已全部删除。
    Removed,
    /// 目标不存在，什么都没做。
    NotFound,
    /// 未取得确认（非交互且未加 --yes，或用户拒绝）。
    Declined,
}

impl RemoveOutcome {
    fn exit_code(self) -> Option<u8> {
        match self {
            RemoveOutcome::Removed => None,
            RemoveOutcome::NotFound | RemoveOutcome::Declined => Some(EXIT_NOTE_NOT_FOUND),
        }
    }
}

/// 删除笔记。
///
/// 删除不可逆，因此非交互终端下必须显式 `--yes` 才会执行 —— 与 AI 回退同一
/// 原则：没有拿到用户明确同意，就不做有副作用的操作。
fn remove_notes(
    notes_dir: &Path,
    commands: &[String],
    prompter: &dyn Prompter,
    assume_yes: bool,
    lang: Language,
) -> Result<RemoveOutcome> {
    // 空目标集是静默空操作，宁可明确报错。
    // 实际经由 clap 的 `rm` 时不可达（`num_args = 1..` 已保证非空），
    // 这里作为内部不变量护栏保留。
    if commands.is_empty() {
        return Err(error::usage(lang.remove_needs_a_target()));
    }

    // 先校验并收集目标，避免「删了几个才发现有笔误」的半成品状态
    let mut targets: Vec<(String, PathBuf)> = Vec::new();
    let mut missing: Vec<String> = Vec::new();

    for command in commands {
        notes::validate_command_name(command, lang)?;
        let path = notes::note_path(notes_dir, command);
        if path.is_file() {
            targets.push((command.clone(), path));
        } else {
            missing.push(command.clone());
        }
    }

    for command in &missing {
        eprintln!("{}", lang.note_not_found_alone(command));
    }
    if targets.is_empty() {
        return Ok(RemoveOutcome::NotFound);
    }

    if !assume_yes {
        if !prompter.is_interactive() {
            eprintln!("{}", lang.remove_needs_confirmation());
            return Ok(RemoveOutcome::Declined);
        }
        let names = targets
            .iter()
            .map(|(command, _)| command.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        // 默认否定：删除不可逆，直接回车不该删掉东西
        if !prompter.confirm(&lang.ask_remove_note(&names), false)? {
            eprintln!("{}", lang.remove_cancelled());
            return Ok(RemoveOutcome::Declined);
        }
    }

    for (command, path) in &targets {
        notes::remove_note(notes_dir, command, lang)?;
        eprintln!("{}", lang.note_removed(&path.display().to_string()));
    }

    if let Some(repo) = enclosing_git_repository(notes_dir) {
        eprintln!("{}", lang.remove_hint_git(&repo.display().to_string()));
    }

    Ok(RemoveOutcome::Removed)
}

/// 笔记目录位于 git 仓库内时返回仓库根，用于给出找回提示。
///
/// 删除不可逆，而把笔记纳入版本控制是常见做法；只向上找有限层，避免在
/// 无关的顶层目录里翻出 `.git` 给出误导性提示。
fn enclosing_git_repository(notes_dir: &Path) -> Option<PathBuf> {
    // 先转绝对路径：相对路径的 `ancestors()` 末尾会产出空路径，
    // 而空路径会命中当前工作目录的 `.git`，提示里的仓库路径就变成空白。
    let start = notes_dir
        .canonicalize()
        .unwrap_or_else(|_| notes_dir.to_path_buf());

    start
        .ancestors()
        .filter(|dir| !dir.as_os_str().is_empty())
        .take(4)
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

fn ask_language(prompter: &dyn Prompter) -> Result<Language> {
    let bootstrap = Language::default();
    eprintln!("{}", bootstrap.first_run_header());
    eprintln!("{}", bootstrap.first_run_option_zh());
    eprintln!("{}", bootstrap.first_run_option_en());

    let chosen = prompter.choose(
        &bootstrap.first_run_prompt(),
        &["zh", "en"],
        &bootstrap.invalid_input(),
    )?;

    Language::parse(&chosen)
        .ok_or_else(|| anyhow::anyhow!("{}", bootstrap.unknown_language_choice(&chosen)))
}

/// 查询动作的最终状态，决定退出码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryOutcome {
    /// 命中本地笔记并已渲染。
    Rendered,
    /// `--edit` 已在编辑器中打开。
    EditorOpened,
    /// 检测到 claude 但生成成功。
    AiGenerated,
    /// 用户拒绝调用 AI。
    AiDeclined,
    /// 非交互终端，按策略跳过 AI。
    AiSkippedNonInteractive,
    /// claude 不可用。
    AiUnavailable,
}

impl QueryOutcome {
    /// 没有可用的本地笔记内容时返回非 0 退出码，便于脚本判断
    /// （`gg foo || echo "没笔记"`）。
    fn exit_code(self) -> Option<u8> {
        match self {
            QueryOutcome::AiDeclined
            | QueryOutcome::AiSkippedNonInteractive
            | QueryOutcome::AiUnavailable => Some(EXIT_NOTE_NOT_FOUND),
            QueryOutcome::Rendered | QueryOutcome::EditorOpened | QueryOutcome::AiGenerated => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct QueryOptions {
    pub target: OutputTarget,
    pub edit: bool,
    /// `--yes`：对所有询问自动回答「是」。
    pub assume_yes: bool,
}

pub struct QueryDeps<'a> {
    pub renderer: &'a dyn Renderer,
    pub editor: &'a dyn EditorLauncher,
    pub generator: &'a dyn NoteGenerator,
    pub prompter: &'a dyn Prompter,
}

pub struct QueryService<'a> {
    notes_dir: &'a Path,
    config: &'a AppConfig,
    deps: &'a QueryDeps<'a>,
}

impl<'a> QueryService<'a> {
    pub fn new(notes_dir: &'a Path, config: &'a AppConfig, deps: &'a QueryDeps<'a>) -> Self {
        Self {
            notes_dir,
            config,
            deps,
        }
    }

    pub fn query(&self, command: &str, options: QueryOptions) -> Result<QueryOutcome> {
        let lang = self.config.language();
        notes::validate_command_name(command, lang)?;

        if options.edit {
            let ensured = notes::ensure_note_file(self.notes_dir, command, lang)?;
            if ensured.created {
                eprintln!("{}", lang.note_created(&ensured.path.display().to_string()));
            }
            self.deps.editor.open(&ensured.path)?;
            return Ok(QueryOutcome::EditorOpened);
        }

        if let Some(markdown) = notes::read_note(self.notes_dir, command, lang)? {
            self.deps
                .renderer
                .render(&markdown, options.target, command)?;
            return Ok(QueryOutcome::Rendered);
        }

        self.report_miss(command, lang)?;
        self.ai_fallback(command, options)
    }

    fn report_miss(&self, command: &str, lang: Language) -> Result<()> {
        let found = notes::scan_commands(self.notes_dir, lang)?;
        let suggestions = notes::suggest_commands(command, &found.items, SUGGESTION_LIMIT);
        eprintln!("{}", miss_message(command, &suggestions, lang));
        Ok(())
    }

    fn ai_fallback(&self, command: &str, options: QueryOptions) -> Result<QueryOutcome> {
        let lang = self.config.language();

        if matches!(self.config.ai_invocation(), AiInvocation::Disabled) {
            eprintln!("{}", lang.ai_disabled());
            return Ok(QueryOutcome::AiUnavailable);
        }

        if !self.deps.generator.is_available() {
            eprintln!(
                "{}",
                lang.ai_tool_missing(&self.deps.generator.description())
            );
            return Ok(QueryOutcome::AiUnavailable);
        }

        // 关键策略：`ask_before_ai = true`（默认）表示需要用户确认，而确认必须
        // 依赖交互终端。`gg foo | less`、`gg foo > out.md` 这类场景 stdout 不是
        // 终端，若直接放行会在用户毫不知情的情况下调用 AI 并写入笔记目录。
        // 两种方式可以显式授权：把 `ask_before_ai` 设为 false，或本次加 `--yes`。
        if self.config.ask_before_ai && !options.assume_yes {
            if !self.deps.prompter.is_interactive() {
                eprintln!("{}", lang.ai_skipped_non_interactive());
                return Ok(QueryOutcome::AiSkippedNonInteractive);
            }
            if !self
                .deps
                .prompter
                .confirm(&lang.ask_generate_note(), true)?
            {
                eprintln!("{}", lang.ai_cancelled());
                return Ok(QueryOutcome::AiDeclined);
            }
        }

        debug_log!("app::ai_fallback: 为 `{command}` 生成笔记");
        let generated = self.generate_with_progress(command, lang)?;

        if self.should_save(options)? {
            // 笔记已落到本地目录，直接把整篇打出来只会刷屏：
            // 告知保存位置与查看命令即可。
            self.save(command, &generated)?;
            eprintln!("{}", lang.note_ready_hint(command));
        } else {
            // 没落盘就只能当场输出，否则刚生成的内容直接丢失。
            eprintln!("{}", lang.save_skipped());
            self.deps
                .renderer
                .render(&generated, options.target, command)?;
        }
        Ok(QueryOutcome::AiGenerated)
    }

    /// 调用 AI，期间在终端显示转圈动画。
    ///
    /// 动画只在 stderr 是终端时启用；否则退化为一行静态提示，
    /// 避免日志里塞满控制字符。
    fn generate_with_progress(&self, command: &str, lang: Language) -> Result<String> {
        let label = lang.ai_progress(command);
        let mut spinner =
            spinner::Spinner::start(label.clone(), spinner::stderr_supports_animation());
        if !spinner.is_active() {
            eprintln!("{label}");
        }

        let result = self
            .deps
            .generator
            .generate(command, &self.config.ai_note_language);

        // 必须在打印结果/报错前擦除动画行，否则会互相覆盖。
        spinner.stop();
        result
    }

    /// 落盘决策：`--yes` 视为对所有询问回答「是」；未启用询问则按 `auto_save_ai`。
    fn should_save(&self, options: QueryOptions) -> Result<bool> {
        if !self.config.ask_before_save {
            return Ok(self.config.auto_save_ai);
        }
        if options.assume_yes {
            return Ok(true);
        }
        if !self.deps.prompter.is_interactive() {
            return Ok(self.config.auto_save_ai);
        }
        self.deps.prompter.confirm(
            &self.config.language().ask_save_note(),
            self.config.auto_save_ai,
        )
    }

    fn save(&self, command: &str, generated: &str) -> Result<()> {
        let lang = self.config.language();
        let path = notes::write_note(self.notes_dir, command, generated, lang)?;
        eprintln!("{}", lang.note_saved(&path.display().to_string()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use anyhow::Result;
    use std::cell::RefCell;
    use std::fs;
    use std::path::{Path, PathBuf};

    #[derive(Default)]
    struct FakeRenderer {
        rendered: RefCell<Vec<String>>,
        titles: RefCell<Vec<String>>,
    }

    impl Renderer for FakeRenderer {
        fn render(&self, markdown: &str, _target: OutputTarget, command: &str) -> Result<()> {
            self.rendered.borrow_mut().push(markdown.to_string());
            self.titles.borrow_mut().push(command.to_string());
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeEditor {
        opened: RefCell<Vec<PathBuf>>,
    }

    impl EditorLauncher for FakeEditor {
        fn open(&self, path: &Path) -> Result<()> {
            self.opened.borrow_mut().push(path.to_path_buf());
            Ok(())
        }
    }

    struct FakeGenerator {
        available: bool,
        calls: RefCell<usize>,
    }

    impl FakeGenerator {
        fn available() -> Self {
            Self {
                available: true,
                calls: RefCell::new(0),
            }
        }

        fn missing() -> Self {
            Self {
                available: false,
                calls: RefCell::new(0),
            }
        }

        fn call_count(&self) -> usize {
            *self.calls.borrow()
        }
    }

    impl NoteGenerator for FakeGenerator {
        fn description(&self) -> String {
            "fake-ai".to_string()
        }

        fn is_available(&self) -> bool {
            self.available
        }

        fn generate(&self, command: &str, _language: &str) -> Result<String> {
            *self.calls.borrow_mut() += 1;
            Ok(format!("# {command}\nAI 生成内容\n"))
        }
    }

    struct FakePrompter {
        interactive: bool,
        answers: RefCell<Vec<bool>>,
        asked: RefCell<Vec<String>>,
        defaults: RefCell<Vec<bool>>,
    }

    impl FakePrompter {
        fn interactive(answers: Vec<bool>) -> Self {
            Self {
                interactive: true,
                answers: RefCell::new(answers),
                asked: RefCell::new(Vec::new()),
                defaults: RefCell::new(Vec::new()),
            }
        }

        fn non_interactive() -> Self {
            Self {
                interactive: false,
                answers: RefCell::new(Vec::new()),
                asked: RefCell::new(Vec::new()),
                defaults: RefCell::new(Vec::new()),
            }
        }

        fn asked(&self) -> Vec<String> {
            self.asked.borrow().clone()
        }

        /// 每次询问传入的默认答案，用于断言危险操作默认否定。
        fn defaults(&self) -> Vec<bool> {
            self.defaults.borrow().clone()
        }
    }

    impl Prompter for FakePrompter {
        fn is_interactive(&self) -> bool {
            self.interactive
        }

        fn confirm(&self, question: &str, default_yes: bool) -> Result<bool> {
            self.asked.borrow_mut().push(question.to_string());
            self.defaults.borrow_mut().push(default_yes);
            let mut answers = self.answers.borrow_mut();
            if answers.is_empty() {
                anyhow::bail!("测试用例没有准备足够的回答");
            }
            Ok(answers.remove(0))
        }

        fn choose(&self, _prompt: &str, options: &[&str], _retry: &str) -> Result<String> {
            Ok(options[0].to_string())
        }
    }

    fn options() -> QueryOptions {
        QueryOptions {
            target: OutputTarget::Terminal,
            edit: false,
            assume_yes: false,
        }
    }

    fn yes_options() -> QueryOptions {
        QueryOptions {
            assume_yes: true,
            ..options()
        }
    }

    fn notes_dir_with_ls() -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("ls.md"), "# ls\n列出目录内容\n").expect("写笔记");
        temp
    }

    /// 两条提示必须落在同一行，只有建议列表换行。
    #[test]
    fn miss_message_puts_both_parts_on_the_same_line() {
        let suggestions = vec!["ls".to_string(), "less".to_string()];
        let message = miss_message("lz", &suggestions, Language::Zh);

        let lines: Vec<&str> = message.lines().collect();
        assert_eq!(
            lines,
            vec!["未找到命令 `lz`，推荐:", "ls, less"],
            "实际:
{message}"
        );
        assert_eq!(
            message.lines().next(),
            miss_message("lz", &suggestions, Language::Zh)
                .lines()
                .next(),
        );
    }

    #[test]
    fn miss_message_without_suggestions_ends_the_sentence() {
        let message = miss_message("zzzz", &[], Language::Zh);
        assert_eq!(message, "未找到命令 `zzzz`。");
        assert!(
            !message.ends_with('，'),
            "没有建议时不应留下悬空的逗号: {message}"
        );
    }

    #[test]
    fn miss_message_is_localized() {
        let suggestions = vec!["ls".to_string()];
        let message = miss_message("lz", &suggestions, Language::En);
        assert_eq!(
            message.lines().next(),
            Some("No notes for `lz`, recommend:")
        );
    }

    fn note_files(temp: &tempfile::TempDir) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(temp.path())
            .expect("读目录")
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                path.file_stem()?.to_str().map(str::to_string)
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn remove_refuses_in_a_non_interactive_terminal_without_yes() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("ls.md"), "# ls\n").expect("写笔记");
        let prompter = FakePrompter::non_interactive();

        let outcome = remove_notes(
            temp.path(),
            &["ls".to_string()],
            &prompter,
            false,
            Language::Zh,
        )
        .expect("执行成功");

        assert_eq!(outcome, RemoveOutcome::Declined);
        assert_eq!(outcome.exit_code(), Some(EXIT_NOTE_NOT_FOUND));
        assert_eq!(note_files(&temp), vec!["ls"], "非交互下不得删除文件");
        assert!(prompter.asked().is_empty(), "非交互下不应尝试询问");
    }

    #[test]
    fn remove_deletes_after_interactive_confirmation() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("ls.md"), "# ls\n").expect("写笔记");
        fs::write(temp.path().join("grep.md"), "# grep\n").expect("写笔记");
        let prompter = FakePrompter::interactive(vec![true]);

        let outcome = remove_notes(
            temp.path(),
            &["ls".to_string()],
            &prompter,
            false,
            Language::Zh,
        )
        .expect("执行成功");

        assert_eq!(outcome, RemoveOutcome::Removed);
        assert_eq!(outcome.exit_code(), None);
        assert_eq!(note_files(&temp), vec!["grep"], "只应删掉指定的那一份");
    }

    /// 删除不可逆，回车必须等于「不删」。
    #[test]
    fn remove_asks_with_a_negative_default() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("ls.md"), "# ls\n").expect("写笔记");
        let prompter = FakePrompter::interactive(vec![true]);

        remove_notes(
            temp.path(),
            &["ls".to_string()],
            &prompter,
            false,
            Language::Zh,
        )
        .expect("执行成功");

        assert_eq!(prompter.defaults(), vec![false], "危险操作必须默认否定");
    }

    #[test]
    fn remove_keeps_the_file_when_the_user_declines() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("ls.md"), "# ls\n").expect("写笔记");
        let prompter = FakePrompter::interactive(vec![false]);

        let outcome = remove_notes(
            temp.path(),
            &["ls".to_string()],
            &prompter,
            false,
            Language::Zh,
        )
        .expect("执行成功");

        assert_eq!(outcome, RemoveOutcome::Declined);
        assert_eq!(note_files(&temp), vec!["ls"]);
    }

    #[test]
    fn yes_authorizes_removal_without_a_terminal() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("ls.md"), "# ls\n").expect("写笔记");
        fs::write(temp.path().join("grep.md"), "# grep\n").expect("写笔记");
        let prompter = FakePrompter::non_interactive();

        let outcome = remove_notes(
            temp.path(),
            &["ls".to_string(), "grep".to_string()],
            &prompter,
            true,
            Language::Zh,
        )
        .expect("执行成功");

        assert_eq!(outcome, RemoveOutcome::Removed);
        assert!(note_files(&temp).is_empty(), "两个目标都应删除");
    }

    #[test]
    fn remove_deletes_only_the_notes_that_exist() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("ls.md"), "# ls\n").expect("写笔记");
        let prompter = FakePrompter::non_interactive();

        let outcome = remove_notes(
            temp.path(),
            &["ls".to_string(), "nope".to_string()],
            &prompter,
            true,
            Language::Zh,
        )
        .expect("执行成功");

        assert_eq!(outcome, RemoveOutcome::Removed, "存在的那份仍应删掉");
        assert!(note_files(&temp).is_empty());
    }

    #[test]
    fn remove_reports_not_found_when_nothing_matches() {
        let temp = tempfile::tempdir().expect("tempdir");
        let prompter = FakePrompter::non_interactive();

        let outcome = remove_notes(
            temp.path(),
            &["nope".to_string()],
            &prompter,
            true,
            Language::Zh,
        )
        .expect("执行成功");

        assert_eq!(outcome, RemoveOutcome::NotFound);
        assert_eq!(outcome.exit_code(), Some(EXIT_NOTE_NOT_FOUND));
    }

    /// 目录与笔记同名时不能被删掉。
    #[test]
    fn remove_ignores_a_directory_with_the_same_name() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::create_dir(temp.path().join("somedir.md")).expect("建同名目录");
        let prompter = FakePrompter::non_interactive();

        let outcome = remove_notes(
            temp.path(),
            &["somedir".to_string()],
            &prompter,
            true,
            Language::Zh,
        )
        .expect("执行成功");

        assert_eq!(outcome, RemoveOutcome::NotFound);
        assert!(temp.path().join("somedir.md").is_dir(), "目录必须还在");
    }

    #[test]
    fn git_recovery_hint_uses_a_non_empty_repository_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let notes_dir = temp.path().join("notes");
        fs::create_dir_all(&notes_dir).expect("建笔记目录");
        fs::create_dir_all(temp.path().join(".git")).expect("造假 git 仓库");

        let repo = enclosing_git_repository(&notes_dir).expect("应找到仓库");
        assert!(!repo.as_os_str().is_empty(), "仓库路径不能是空的");
        assert_eq!(
            repo.canonicalize().expect("规范化"),
            temp.path().canonicalize().expect("规范化"),
        );
    }

    /// 空目标集不得退化为「静默成功」。
    #[test]
    fn remove_rejects_an_empty_target_list() {
        let temp = tempfile::tempdir().expect("tempdir");
        let prompter = FakePrompter::non_interactive();

        let err = remove_notes(temp.path(), &[], &prompter, true, Language::Zh)
            .expect_err("空目标集应当报错");
        assert!(format!("{err:#}").contains("至少一个"), "{err:#}");
    }

    #[test]
    fn remove_rejects_path_traversal_before_touching_disk() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("ls.md"), "# ls\n").expect("写笔记");
        let prompter = FakePrompter::non_interactive();

        let err = remove_notes(
            temp.path(),
            &["../etc/passwd".to_string()],
            &prompter,
            true,
            Language::Zh,
        )
        .expect_err("路径穿越必须被拒绝");

        assert!(format!("{err:#}").contains("路径字符"));
        assert_eq!(note_files(&temp), vec!["ls"], "拒绝后不应有副作用");
    }

    #[test]
    fn hit_renders_without_touching_ai() {
        let temp = notes_dir_with_ls();
        let config = AppConfig::default();
        let renderer = FakeRenderer::default();
        let editor = FakeEditor::default();
        let generator = FakeGenerator::available();
        let prompter = FakePrompter::non_interactive();
        let deps = QueryDeps {
            renderer: &renderer,
            editor: &editor,
            generator: &generator,
            prompter: &prompter,
        };

        let outcome = QueryService::new(temp.path(), &config, &deps)
            .query("ls", options())
            .expect("查询成功");

        assert_eq!(outcome, QueryOutcome::Rendered);
        assert_eq!(renderer.rendered.borrow().len(), 1);
        assert_eq!(generator.call_count(), 0);
        assert!(prompter.asked().is_empty());
    }

    /// P0 回归：管道场景（stdout 非终端）绝不能调用 AI，也不能落盘。
    #[test]
    fn non_interactive_miss_never_calls_ai_nor_writes_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            auto_save_ai: true,
            ask_before_ai: true,
            ..AppConfig::default()
        };
        let renderer = FakeRenderer::default();
        let editor = FakeEditor::default();
        let generator = FakeGenerator::available();
        let prompter = FakePrompter::non_interactive();
        let deps = QueryDeps {
            renderer: &renderer,
            editor: &editor,
            generator: &generator,
            prompter: &prompter,
        };

        let outcome = QueryService::new(temp.path(), &config, &deps)
            .query("lz", options())
            .expect("查询成功");

        assert_eq!(outcome, QueryOutcome::AiSkippedNonInteractive);
        assert_eq!(generator.call_count(), 0, "非交互时不得调用 AI");
        assert!(!temp.path().join("lz.md").exists(), "非交互时不得写入笔记");
    }

    #[test]
    fn declining_ai_confirmation_skips_generation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            ask_before_ai: true,
            ..AppConfig::default()
        };
        let renderer = FakeRenderer::default();
        let editor = FakeEditor::default();
        let generator = FakeGenerator::available();
        let prompter = FakePrompter::interactive(vec![false]);
        let deps = QueryDeps {
            renderer: &renderer,
            editor: &editor,
            generator: &generator,
            prompter: &prompter,
        };

        let outcome = QueryService::new(temp.path(), &config, &deps)
            .query("lz", options())
            .expect("查询成功");

        assert_eq!(outcome, QueryOutcome::AiDeclined);
        assert_eq!(generator.call_count(), 0);
        assert!(!temp.path().join("lz.md").exists());
    }

    /// 显式把 `ask_before_ai` 设为 false 即视为放弃确认，允许非交互生成。
    #[test]
    fn ask_before_ai_false_allows_non_interactive_generation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            ask_before_ai: false,
            auto_save_ai: true,
            ask_before_save: false,
            ..AppConfig::default()
        };
        let renderer = FakeRenderer::default();
        let editor = FakeEditor::default();
        let generator = FakeGenerator::available();
        let prompter = FakePrompter::non_interactive();
        let deps = QueryDeps {
            renderer: &renderer,
            editor: &editor,
            generator: &generator,
            prompter: &prompter,
        };

        let outcome = QueryService::new(temp.path(), &config, &deps)
            .query("lz", options())
            .expect("查询成功");

        assert_eq!(outcome, QueryOutcome::AiGenerated);
        assert_eq!(generator.call_count(), 1);
        assert!(temp.path().join("lz.md").exists());
    }

    /// `--yes` 应当能授权非交互场景生成，且不产生任何询问。
    #[test]
    fn assume_yes_generates_non_interactively_without_prompting() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            ask_before_ai: true,
            auto_save_ai: true,
            ask_before_save: true,
            ..AppConfig::default()
        };
        let renderer = FakeRenderer::default();
        let editor = FakeEditor::default();
        let generator = FakeGenerator::available();
        let prompter = FakePrompter::non_interactive();
        let deps = QueryDeps {
            renderer: &renderer,
            editor: &editor,
            generator: &generator,
            prompter: &prompter,
        };

        let outcome = QueryService::new(temp.path(), &config, &deps)
            .query("lz", yes_options())
            .expect("查询成功");

        assert_eq!(outcome, QueryOutcome::AiGenerated);
        assert_eq!(generator.call_count(), 1);
        assert!(
            prompter.asked().is_empty(),
            "--yes 不应再问任何问题，实际问了: {:?}",
            prompter.asked()
        );
        assert!(temp.path().join("lz.md").exists());
    }

    /// `--yes` 是对询问回答「是」，因此即使 auto_save_ai=false 也应落盘。
    #[test]
    fn assume_yes_overrides_auto_save_veto() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            ask_before_ai: true,
            ask_before_save: true,
            auto_save_ai: false,
            ..AppConfig::default()
        };
        let renderer = FakeRenderer::default();
        let editor = FakeEditor::default();
        let generator = FakeGenerator::available();
        let prompter = FakePrompter::non_interactive();
        let deps = QueryDeps {
            renderer: &renderer,
            editor: &editor,
            generator: &generator,
            prompter: &prompter,
        };

        let outcome = QueryService::new(temp.path(), &config, &deps)
            .query("lz", yes_options())
            .expect("查询成功");

        assert_eq!(outcome, QueryOutcome::AiGenerated);
        assert!(
            temp.path().join("lz.md").exists(),
            "--yes 应视为对保存询问回答「是」"
        );
    }

    /// `--yes` 是对询问的预设回答，不能绕过 claude 是否存在的判断。
    #[test]
    fn assume_yes_does_not_bypass_missing_claude() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig::default();
        let renderer = FakeRenderer::default();
        let editor = FakeEditor::default();
        let generator = FakeGenerator::missing();
        let prompter = FakePrompter::non_interactive();
        let deps = QueryDeps {
            renderer: &renderer,
            editor: &editor,
            generator: &generator,
            prompter: &prompter,
        };

        let outcome = QueryService::new(temp.path(), &config, &deps)
            .query("lz", yes_options())
            .expect("查询成功");

        assert_eq!(outcome, QueryOutcome::AiUnavailable);
        assert_eq!(generator.call_count(), 0);
    }

    /// 未启用询问时（ask_before_save=false），`--yes` 不应改变 auto_save_ai 的语义。
    #[test]
    fn assume_yes_keeps_auto_save_ai_when_no_prompt_is_configured() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            ask_before_ai: false,
            ask_before_save: false,
            auto_save_ai: false,
            ..AppConfig::default()
        };
        let renderer = FakeRenderer::default();
        let editor = FakeEditor::default();
        let generator = FakeGenerator::available();
        let prompter = FakePrompter::non_interactive();
        let deps = QueryDeps {
            renderer: &renderer,
            editor: &editor,
            generator: &generator,
            prompter: &prompter,
        };

        let outcome = QueryService::new(temp.path(), &config, &deps)
            .query("lz", yes_options())
            .expect("查询成功");

        assert_eq!(outcome, QueryOutcome::AiGenerated);
        assert!(
            !temp.path().join("lz.md").exists(),
            "没配置询问时，--yes 不应把 auto_save_ai=false 变成保存"
        );
    }

    #[test]
    fn interactive_accept_saves_generated_note() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            ask_before_ai: true,
            auto_save_ai: true,
            ask_before_save: false,
            ..AppConfig::default()
        };
        let renderer = FakeRenderer::default();
        let editor = FakeEditor::default();
        let generator = FakeGenerator::available();
        let prompter = FakePrompter::interactive(vec![true]);
        let deps = QueryDeps {
            renderer: &renderer,
            editor: &editor,
            generator: &generator,
            prompter: &prompter,
        };

        let outcome = QueryService::new(temp.path(), &config, &deps)
            .query("lz", options())
            .expect("查询成功");

        assert_eq!(outcome, QueryOutcome::AiGenerated);
        assert_eq!(generator.call_count(), 1);
        let saved = fs::read_to_string(temp.path().join("lz.md")).expect("笔记应已保存");
        assert!(saved.contains("AI 生成内容"));
    }

    #[test]
    fn ask_before_save_can_veto_writing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            ask_before_ai: true,
            ask_before_save: true,
            auto_save_ai: false,
            ..AppConfig::default()
        };
        let renderer = FakeRenderer::default();
        let editor = FakeEditor::default();
        let generator = FakeGenerator::available();
        let prompter = FakePrompter::interactive(vec![true, false]);
        let deps = QueryDeps {
            renderer: &renderer,
            editor: &editor,
            generator: &generator,
            prompter: &prompter,
        };

        let outcome = QueryService::new(temp.path(), &config, &deps)
            .query("lz", options())
            .expect("查询成功");

        assert_eq!(outcome, QueryOutcome::AiGenerated);
        assert_eq!(generator.call_count(), 1);
        assert!(!temp.path().join("lz.md").exists(), "用户拒绝了保存");
    }

    #[test]
    fn missing_claude_reports_unavailable_without_prompting() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig::default();
        let renderer = FakeRenderer::default();
        let editor = FakeEditor::default();
        let generator = FakeGenerator::missing();
        let prompter = FakePrompter::interactive(vec![true]);
        let deps = QueryDeps {
            renderer: &renderer,
            editor: &editor,
            generator: &generator,
            prompter: &prompter,
        };

        let outcome = QueryService::new(temp.path(), &config, &deps)
            .query("lz", options())
            .expect("查询成功");

        assert_eq!(outcome, QueryOutcome::AiUnavailable);
        assert!(prompter.asked().is_empty(), "claude 缺失时不应询问");
    }

    #[test]
    fn edit_creates_note_and_opens_editor_without_rendering() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig::default();
        let renderer = FakeRenderer::default();
        let editor = FakeEditor::default();
        let generator = FakeGenerator::missing();
        let prompter = FakePrompter::non_interactive();
        let deps = QueryDeps {
            renderer: &renderer,
            editor: &editor,
            generator: &generator,
            prompter: &prompter,
        };

        let outcome = QueryService::new(temp.path(), &config, &deps)
            .query(
                "newcmd",
                QueryOptions {
                    target: OutputTarget::Terminal,
                    edit: true,
                    assume_yes: false,
                },
            )
            .expect("查询成功");

        assert_eq!(outcome, QueryOutcome::EditorOpened);
        assert_eq!(editor.opened.borrow().len(), 1);
        assert!(temp.path().join("newcmd.md").exists());
        assert!(renderer.rendered.borrow().is_empty());
    }

    #[test]
    fn path_traversal_is_rejected_before_touching_disk() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig::default();
        let renderer = FakeRenderer::default();
        let editor = FakeEditor::default();
        let generator = FakeGenerator::missing();
        let prompter = FakePrompter::non_interactive();
        let deps = QueryDeps {
            renderer: &renderer,
            editor: &editor,
            generator: &generator,
            prompter: &prompter,
        };

        let err = QueryService::new(temp.path(), &config, &deps)
            .query("../etc/passwd", options())
            .expect_err("路径穿越必须被拒绝");
        assert!(format!("{err:#}").contains("路径字符"));
    }

    #[test]
    fn miss_sets_not_found_exit_code() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig::default();
        let renderer = FakeRenderer::default();
        let editor = FakeEditor::default();
        let generator = FakeGenerator::missing();
        let prompter = FakePrompter::non_interactive();
        let deps = QueryDeps {
            renderer: &renderer,
            editor: &editor,
            generator: &generator,
            prompter: &prompter,
        };

        let outcome = QueryService::new(temp.path(), &config, &deps)
            .query("lz", options())
            .expect("查询成功");

        assert_eq!(outcome, QueryOutcome::AiUnavailable);
        assert_eq!(
            outcome.exit_code(),
            Some(EXIT_NOTE_NOT_FOUND),
            "未命中必须返回非 0 退出码"
        );
    }

    /// 生成成功即视为成功：用户已经拿到内容。
    #[test]
    fn generated_note_exits_zero() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = AppConfig {
            ask_before_ai: true,
            ..AppConfig::default()
        };
        let renderer = FakeRenderer::default();
        let editor = FakeEditor::default();
        let generator = FakeGenerator::available();
        let prompter = FakePrompter::interactive(vec![true]);
        let deps = QueryDeps {
            renderer: &renderer,
            editor: &editor,
            generator: &generator,
            prompter: &prompter,
        };

        let outcome = QueryService::new(temp.path(), &config, &deps)
            .query("lz", options())
            .expect("查询成功");

        assert_eq!(outcome.exit_code(), None);
    }
}
