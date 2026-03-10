use crate::ai;
use crate::cli::{Action, Cli};
use crate::config::{self, AppConfig};
use crate::editor;
use crate::notes;
use crate::render;
use anyhow::Result;
use std::io::{self, IsTerminal, Write};
use std::path::Path;

const SUGGESTION_LIMIT: usize = 5;

pub fn run(cli: Cli) -> Result<()> {
    let (notes_override, browser, edit, action) = cli.into_parts();
    let notes_dir = config::resolve_notes_dir(notes_override)?;
    let config = AppConfig::load()?;

    match action {
        Action::List => list_commands(&notes_dir),
        Action::Search(keyword) => search_commands(&notes_dir, &keyword),
        Action::Query(command) => query_command(&notes_dir, &config, &command, browser, edit),
    }
}

fn list_commands(notes_dir: &Path) -> Result<()> {
    let commands = notes::list_commands(notes_dir)?;
    for command in commands {
        println!("{command}");
    }
    Ok(())
}

fn search_commands(notes_dir: &Path, keyword: &str) -> Result<()> {
    let commands = notes::search_commands_by_name(notes_dir, keyword)?;
    for command in commands {
        println!("{command}");
    }
    Ok(())
}

fn query_command(notes_dir: &Path, config: &AppConfig, command: &str, browser: bool, edit: bool) -> Result<()> {
    notes::validate_command_name(command)?;

    if edit {
        let path = notes::ensure_note_file(notes_dir, command)?;
        editor::open_in_editor(&path)?;
        return Ok(());
    }
    if let Some(markdown) = notes::read_note(notes_dir, command)? {
        if browser {
            render::render_markdown_in_browser(&markdown)?;
        } else {
            render::render_markdown(&markdown);
        }
        return Ok(());
    }

    eprintln!("未找到命令 `{command}` 的笔记。");
    let all_commands = notes::list_commands(notes_dir)?;
    let suggestions = notes::suggest_commands(command, &all_commands, SUGGESTION_LIMIT);
    if !suggestions.is_empty() {
        eprintln!("你可能想查: {}", suggestions.join(", "));
    }

    if !config.ai_provider.eq_ignore_ascii_case("claude") {
        eprintln!(
            "当前 ai_provider={}，v1 仅支持 claude，已跳过 AI 回退。",
            config.ai_provider
        );
        return Ok(());
    }

    if !ai::is_claude_available() {
        eprintln!("未检测到 claude CLI，已跳过 AI 回退。");
        return Ok(());
    }

    let interactive = is_interactive_terminal();
    let should_query = if interactive && config.ask_before_ai {
        ask_yes_no("检测到 claude，可尝试生成该笔记。是否继续查询？", true)?
    } else {
        true
    };

    if !should_query {
        eprintln!("已取消 AI 查询。");
        return Ok(());
    }

    let generated = ai::generate_note_with_claude(command, &config.ai_note_language)?;
    if browser {
        render::render_markdown_in_browser(&generated)?;
    } else {
        render::render_markdown(&generated);
    }

    let should_save = if interactive && config.ask_before_save {
        ask_yes_no("是否保存这份 AI 生成笔记到本地？", config.auto_save_ai)?
    } else {
        config.auto_save_ai
    };

    if should_save {
        let path = notes::write_note(notes_dir, command, &generated)?;
        eprintln!("已保存笔记: {}", path.display());
    } else {
        eprintln!("已跳过保存。");
    }

    Ok(())
}

fn is_interactive_terminal() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

fn ask_yes_no(question: &str, default_yes: bool) -> Result<bool> {
    let mut stderr = io::stderr();
    let default_hint = if default_yes { "Y/n" } else { "y/N" };

    loop {
        write!(stderr, "{question} [{default_hint}]: ")?;
        stderr.flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let answer = input.trim().to_ascii_lowercase();

        if answer.is_empty() {
            return Ok(default_yes);
        }
        if answer == "y" || answer == "yes" {
            return Ok(true);
        }
        if answer == "n" || answer == "no" {
            return Ok(false);
        }

        writeln!(stderr, "请输入 y 或 n。")?;
    }
}








