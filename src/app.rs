use crate::ai;
use crate::cli::{Action, Cli};
use crate::config::{self, AppConfig};
use crate::editor;
use crate::notes;
use crate::render;
use anyhow::Result;
use clap::CommandFactory;
use std::io::{self, IsTerminal, Write};
use std::path::Path;

const SUGGESTION_LIMIT: usize = 5;

pub fn run(cli: Cli) -> Result<()> {
    let parts = cli.into_parts();
    let notes_dir = config::resolve_notes_dir(parts.notes_dir)?;
    let mut config = AppConfig::load()?;

    // Handle --set-editor
    if let Some(editor) = parts.set_editor {
        config.editor = Some(editor);
        config.save()?;
        eprintln!("已保存编辑器配置。");
        return Ok(());
    }

    // Handle --lang
    if let Some(lang) = parts.lang {
        let lang = lang.to_lowercase();
        if lang != "zh" && lang != "en" {
            anyhow::bail!("无效的语言选项: {}，请使用 zh 或 en", lang);
        }
        config.language = Some(lang);
        config.save()?;
        eprintln!("已保存语言配置。");
        return Ok(());
    }

    // First run: ask for language preference (only in interactive mode)
    if config.is_first_run() && is_interactive_terminal() {
        eprintln!("首次使用 gg，请选择显示语言 (Choose display language):");
        eprintln!("  1) 中文 (zh)");
        eprintln!("  2) English (en)");
        let lang = ask_choice(&["zh", "en"], "请输入选项数字 (Enter option number) [1]: ")?;
        config.language = Some(lang);
        config.save()?;
        eprintln!("已保存语言配置。");
    }

    // Determine current language (default to zh if not set)
    let lang = config.language.as_deref().unwrap_or("zh");

    match parts.action {
        Action::List => list_commands(&notes_dir, lang),
        Action::Search(keyword) => search_commands(&notes_dir, &keyword, lang),
        Action::Query(command) => query_command(&notes_dir, &config, &command, parts.browser, parts.edit, lang),
        Action::None => {
            // No subcommand and no config action - this is first run without interaction
            // or user just wants help. Show help.
            let is_first = config.is_first_run();
            if is_first {
                // First run but non-interactive: default to zh
                config.language = Some("zh".to_string());
                config.save().ok();
                eprintln!("(已默认设置语言为中文，如需更改请使用 --lang 选项)");
            }

            // Determine lang after potential first-run save
            let current_lang = config.language.as_deref().unwrap_or("zh");

            if current_lang == "zh" {
                print_help_zh();
            } else {
                Cli::command().print_help().map_err(|e| anyhow::anyhow!("Failed to print help: {}", e))?;
                println!();
            }
            Ok(())
        }
    }
}

fn ask_choice(options: &[&str], prompt: &str) -> Result<String> {
    let mut stderr = io::stderr();
    loop {
        write!(stderr, "{}", prompt)?;
        stderr.flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let answer = input.trim();

        if answer.is_empty() {
            return Ok(options[0].to_string());
        }

        if let Ok(num) = answer.parse::<usize>() {
            if num >= 1 && num <= options.len() {
                return Ok(options[num - 1].to_string());
            }
        }

        // Check if input directly matches an option
        for opt in options {
            if answer.eq_ignore_ascii_case(opt) {
                return Ok(opt.to_string());
            }
        }

        writeln!(stderr, "无效输入，请重新输入。")?;
    }
}

fn list_commands(notes_dir: &Path, _lang: &str) -> Result<()> {
    let commands = notes::list_commands(notes_dir)?;
    for command in commands {
        println!("{command}");
    }
    Ok(())
}

fn search_commands(notes_dir: &Path, keyword: &str, _lang: &str) -> Result<()> {
    let commands = notes::search_commands_by_name(notes_dir, keyword)?;
    for command in commands {
        println!("{command}");
    }
    Ok(())
}

fn query_command(notes_dir: &Path, config: &AppConfig, command: &str, browser: bool, edit: bool, lang: &str) -> Result<()> {
    notes::validate_command_name(command)?;

    if edit {
        let path = notes::ensure_note_file(notes_dir, command)?;
        editor::open_in_editor(&path, config)?;
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

    let msg_not_found = if lang == "zh" {
        format!("未找到命令 `{command}` 的笔记。")
    } else {
        format!("No notes found for command `{command}`.")
    };
    eprintln!("{msg_not_found}");

    let all_commands = notes::list_commands(notes_dir)?;
    let suggestions = notes::suggest_commands(command, &all_commands, SUGGESTION_LIMIT);
    if !suggestions.is_empty() {
        let msg = if lang == "zh" {
            format!("你可能想查: {}", suggestions.join(", "))
        } else {
            format!("Did you mean: {}", suggestions.join(", "))
        };
        eprintln!("{msg}");
    }

    if !config.ai_provider.eq_ignore_ascii_case("claude") {
        let msg = if lang == "zh" {
            format!("当前 ai_provider={}，v1 仅支持 claude，已跳过 AI 回退。", config.ai_provider)
        } else {
            format!("ai_provider={} not supported in v1, skipped AI fallback.", config.ai_provider)
        };
        eprintln!("{msg}");
        return Ok(());
    }

    if !ai::is_claude_available() {
        let msg = if lang == "zh" {
            "未检测到 claude CLI，已跳过 AI 回退。"
        } else {
            "claude CLI not found, skipped AI fallback."
        };
        eprintln!("{msg}");
        return Ok(());
    }

    let interactive = is_interactive_terminal();
    let (question, default_yes) = if lang == "zh" {
        ("检测到 claude，可尝试生成该笔记。是否继续查询？", true)
    } else {
        ("claude detected. Generate notes? ", true)
    };
    let should_query = if interactive && config.ask_before_ai {
        ask_yes_no(question, default_yes)?
    } else {
        true
    };

    if !should_query {
        let msg = if lang == "zh" { "已取消 AI 查询。" } else { "AI query cancelled." };
        eprintln!("{msg}");
        return Ok(());
    }

    let generated = ai::generate_note_with_claude(command, &config.ai_note_language)?;
    if browser {
        render::render_markdown_in_browser(&generated)?;
    } else {
        render::render_markdown(&generated);
    }

    let (save_question, default_save) = if lang == "zh" {
        ("是否保存这份 AI 生成笔记到本地？", config.auto_save_ai)
    } else {
        ("Save AI-generated notes locally?", config.auto_save_ai)
    };
    let should_save = if interactive && config.ask_before_save {
        ask_yes_no(save_question, default_save)?
    } else {
        config.auto_save_ai
    };

    if should_save {
        let path = notes::write_note(notes_dir, command, &generated)?;
        let msg = if lang == "zh" {
            format!("已保存笔记: {}", path.display())
        } else {
            format!("Notes saved: {}", path.display())
        };
        eprintln!("{msg}");
    } else {
        let msg = if lang == "zh" { "已跳过保存。" } else { "Save skipped." };
        eprintln!("{msg}");
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

fn print_help_zh() {
    println!("gg - 像 man 一样查询你自己的命令笔记");
    println!();
    println!("用法: gg [选项] [命令]");
    println!();
    println!("命令:");
    println!("  list              列出所有笔记命令");
    println!("  search <关键词>   按文件名搜索笔记命令");
    println!("  help              打印此帮助信息或指定子命令的帮助");
    println!();
    println!("选项:");
    println!("    --notes-dir <目录>      指定笔记目录");
    println!("  -b, --browser             在浏览器中打开 markdown 而非终端");
    println!("  -e, --edit                在默认编辑器中打开 markdown 进行编辑");
    println!("    --set-editor <编辑器>   设置默认编辑器并保存到配置");
    println!("    --lang <语言>           设置显示语言 (zh/en) 并保存到配置");
    println!("  --help                    打印帮助");
    println!("  --version                 打印版本");
}








