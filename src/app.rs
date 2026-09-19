//! 应用编排层：解析配置 → 分派动作 → 组装依赖。
//!
//! 查询逻辑收敛到 [`QueryService`]，通过 [`QueryDeps`] 注入渲染器、提示器、
//! AI 生成器与编辑器，使「非交互场景不得触发 AI / 落盘」这类规则可被单测覆盖。

use crate::ai::{ClaudeGenerator, NoteGenerator};
use crate::cli::{Action, Cli};
use crate::config::{self, AppConfig};
use crate::editor::{EditorLauncher, SystemEditor};
use crate::i18n::Language;
use crate::notes;
use crate::prompt::{ConsolePrompter, Prompter};
use crate::render::{MarkdownRenderer, OutputTarget, Renderer};
use crate::utils::debug_log;
use crate::utils::layout;
use crate::utils::output;
use anyhow::{Context, Result};
use std::io::{self, IsTerminal};
use std::path::Path;
use std::process::ExitCode;

const SUGGESTION_LIMIT: usize = 5;

/// 拿不到真实终端宽度时的兜底列数。
const DEFAULT_TERMINAL_WIDTH: usize = 80;

/// 未命中笔记时的退出码，便于脚本判断（`gg foo || echo "没笔记"`）。
pub const EXIT_NOTE_NOT_FOUND: u8 = 3;

pub fn run(cli: Cli) -> Result<ExitCode> {
    let parts = cli.into_parts();
    let notes_dir = config::resolve_notes_dir(parts.notes_dir)?;
    let mut config = AppConfig::load()?;

    // --set-editor / --lang 是纯配置动作，先于首次运行引导处理。
    if let Some(editor) = parts.set_editor {
        config.editor = Some(editor);
        config.save()?;
        eprintln!("{}", config.language().saved_editor_config());
        return Ok(ExitCode::SUCCESS);
    }

    if let Some(raw) = parts.lang {
        let Some(lang) = Language::parse(&raw) else {
            anyhow::bail!("{}", config.language().invalid_language(&raw));
        };
        config.language = Some(lang);
        config.save()?;
        eprintln!("{}", lang.saved_language_config());
        return Ok(ExitCode::SUCCESS);
    }

    let prompter = ConsolePrompter::new(config.language());
    if config.is_first_run() && prompter.is_interactive() {
        let lang = ask_language(&prompter)?;
        config.language = Some(lang);
        config.save()?;
        eprintln!("{}", lang.saved_language_config());
    }

    let lang = config.language();

    match parts.action {
        Action::List => {
            let found = notes::scan_commands(&notes_dir)?;
            warn_skipped(&found.skipped, lang);
            write_commands(&found.items, lang)?;
            Ok(ExitCode::SUCCESS)
        }
        Action::Search { keyword, content } => {
            if content {
                let found = notes::search_notes_by_content(&notes_dir, &keyword)?;
                warn_skipped(&found.skipped, lang);
                let lines = found
                    .items
                    .iter()
                    .map(notes::ContentMatch::render)
                    .collect::<Vec<_>>();
                output::write_lines(io::stdout().lock(), lines)?;
            } else {
                let found = notes::search_commands_by_name(&notes_dir, &keyword)?;
                warn_skipped(&found.skipped, lang);
                write_commands(&found.items, lang)?;
            }
            Ok(ExitCode::SUCCESS)
        }
        Action::Query(command) => {
            if config.is_first_run() {
                // 非交互首次运行：默认中文并落盘，避免后续每次都判空。
                config.language = Some(lang);
                config.save().ok();
                eprintln!("{}", lang.first_run_default_note());
            }

            let renderer = MarkdownRenderer::new(lang);
            let editor = SystemEditor::from_config(&config);
            let generator = ClaudeGenerator::new(lang, config.ai_timeout());
            let deps = QueryDeps {
                renderer: &renderer,
                editor: &editor,
                generator: &generator,
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
                config.save().ok();
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
            return Err(err).context("无法输出帮助信息");
        }
    }
    Ok(())
}

/// 输出命令名列表。
///
/// 只在 stdout 是终端时排成多列（对齐 `ls` 的行为），管道与重定向仍为
/// 每行一条，保证 `gg list | grep x` 这类脚本不受影响。
fn write_commands(commands: &[String], _lang: Language) -> Result<()> {
    let stdout = io::stdout();
    if stdout.is_terminal() {
        let rendered = layout::format_columns(commands, terminal_width());
        return output::write_text(stdout.lock(), &rendered);
    }
    output::write_lines(stdout.lock(), commands.to_vec())
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

    Language::parse(&chosen).ok_or_else(|| anyhow::anyhow!("无法识别语言选项: {chosen}"))
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
        notes::validate_command_name(command)?;
        let lang = self.config.language();

        if options.edit {
            let ensured = notes::ensure_note_file(self.notes_dir, command)?;
            if ensured.created {
                eprintln!("{}", lang.note_created(&ensured.path.display().to_string()));
            }
            self.deps.editor.open(&ensured.path)?;
            return Ok(QueryOutcome::EditorOpened);
        }

        if let Some(markdown) = notes::read_note(self.notes_dir, command)? {
            self.deps.renderer.render(&markdown, options.target)?;
            return Ok(QueryOutcome::Rendered);
        }

        self.report_miss(command, lang)?;
        self.ai_fallback(command, options)
    }

    fn report_miss(&self, command: &str, lang: Language) -> Result<()> {
        eprintln!("{}", lang.note_not_found(command));

        let found = notes::scan_commands(self.notes_dir)?;
        let suggestions = notes::suggest_commands(command, &found.items, SUGGESTION_LIMIT);
        if !suggestions.is_empty() {
            eprintln!("{}", lang.did_you_mean(&suggestions.join(", ")));
        }
        Ok(())
    }

    fn ai_fallback(&self, command: &str, options: QueryOptions) -> Result<QueryOutcome> {
        let lang = self.config.language();

        if !self.deps.generator.is_available() {
            eprintln!("{}", lang.claude_missing());
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
        eprintln!("{}", lang.ai_progress(command));
        let generated = self
            .deps
            .generator
            .generate(command, &self.config.ai_note_language)?;
        self.deps.renderer.render(&generated, options.target)?;

        if self.should_save(options)? {
            self.save(command, &generated)?;
        } else {
            eprintln!("{}", lang.save_skipped());
        }
        Ok(QueryOutcome::AiGenerated)
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
        let path = notes::write_note(self.notes_dir, command, generated)?;
        eprintln!(
            "{}",
            self.config
                .language()
                .note_saved(&path.display().to_string())
        );
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
    }

    impl Renderer for FakeRenderer {
        fn render(&self, markdown: &str, _target: OutputTarget) -> Result<()> {
            self.rendered.borrow_mut().push(markdown.to_string());
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
    }

    impl FakePrompter {
        fn interactive(answers: Vec<bool>) -> Self {
            Self {
                interactive: true,
                answers: RefCell::new(answers),
                asked: RefCell::new(Vec::new()),
            }
        }

        fn non_interactive() -> Self {
            Self {
                interactive: false,
                answers: RefCell::new(Vec::new()),
                asked: RefCell::new(Vec::new()),
            }
        }

        fn asked(&self) -> Vec<String> {
            self.asked.borrow().clone()
        }
    }

    impl Prompter for FakePrompter {
        fn is_interactive(&self) -> bool {
            self.interactive
        }

        fn confirm(&self, question: &str, _default_yes: bool) -> Result<bool> {
            self.asked.borrow_mut().push(question.to_string());
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
        assert!(format!("{err:#}").contains("unsupported path characters"));
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
