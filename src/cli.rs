use crate::i18n::Language;
use clap::{Command, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "gg", version, about = "Query your own command notes like man")]
pub struct Cli {
    #[arg(long, global = true, value_name = "DIR")]
    pub notes_dir: Option<PathBuf>,

    #[arg(short, long, global = true)]
    pub browser: bool,

    #[arg(short = 'e', long, global = true)]
    pub edit: bool,

    #[arg(long, global = true, value_name = "EDITOR")]
    pub set_editor: Option<String>,

    #[arg(long, global = true, value_name = "LANG")]
    pub lang: Option<String>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    List,
    Search {
        #[arg(value_name = "KEYWORD")]
        keyword: String,
    },
    #[command(external_subcommand)]
    Query(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Query(String),
    List,
    Search(String),
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliParts {
    pub notes_dir: Option<PathBuf>,
    pub browser: bool,
    pub edit: bool,
    pub set_editor: Option<String>,
    pub lang: Option<String>,
    pub action: Action,
}

impl Cli {
    pub fn into_parts(self) -> CliParts {
        let action = match self.command {
            Some(Commands::List) => Action::List,
            Some(Commands::Search { keyword }) => Action::Search(keyword),
            Some(Commands::Query(args)) => Action::Query(args.join(" ")),
            None => Action::None,
        };

        CliParts {
            notes_dir: self.notes_dir,
            browser: self.browser,
            edit: self.edit,
            set_editor: self.set_editor,
            lang: self.lang,
            action,
        }
    }
}

/// 按语言构造本地化的 clap 命令。
///
/// 改造前 `app.rs` 里有一份手写的 `print_help_zh()`，与 clap 生成的英文帮助
/// 各自维护，已经漂移（手写版漏掉了 `-h`）。这里统一由 clap 生成，只有一份定义。
pub fn command(lang: Language) -> Command {
    use clap::CommandFactory;

    let options_heading = leak(lang.cli_heading_options());
    let commands_heading = leak(lang.cli_heading_commands());
    let template = format!(
        "{{about-with-newline}}\n{}: {{usage}}\n\n{{all-args}}{{after-help}}\n",
        lang.cli_usage_heading()
    );

    let mut cmd = Cli::command()
        .about(lang.cli_about())
        .help_template(template)
        .subcommand_help_heading(commands_heading)
        .mut_arg("notes_dir", |arg| {
            arg.help(lang.cli_arg_notes_dir())
                .help_heading(options_heading)
        })
        .mut_arg("browser", |arg| {
            arg.help(lang.cli_arg_browser())
                .help_heading(options_heading)
        })
        .mut_arg("edit", |arg| {
            arg.help(lang.cli_arg_edit()).help_heading(options_heading)
        })
        .mut_arg("set_editor", |arg| {
            arg.help(lang.cli_arg_set_editor())
                .help_heading(options_heading)
        })
        .mut_arg("lang", |arg| {
            arg.help(lang.cli_arg_lang()).help_heading(options_heading)
        })
        .mut_subcommand("list", |sub| sub.about(lang.cli_cmd_list()))
        .mut_subcommand("search", |sub| {
            sub.about(lang.cli_cmd_search())
                .mut_arg("keyword", |arg| arg.help(lang.cli_cmd_search_keyword()))
        });

    // `-h/--help`、`-V/--version` 与 `help` 子命令由 clap 惰性注入，
    // 必须先 build 才能改它们的文案，否则 `mut_arg` 会直接 panic。
    cmd.build();
    cmd.mut_arg("help", |arg| {
        arg.help(lang.cli_arg_help()).help_heading(options_heading)
    })
    .mut_arg("version", |arg| {
        arg.help(lang.cli_arg_version())
            .help_heading(options_heading)
    })
    .mut_subcommand("help", |sub| sub.about(lang.cli_cmd_help()))
}

/// clap 的分组标题要求 `&'static str`，而文案在运行时才确定语言。
/// 每个进程最多泄漏几个短字符串，代价可忽略。
fn leak(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn render_help(lang: Language) -> String {
        command(lang).render_long_help().to_string()
    }

    #[test]
    fn help_is_localized() {
        let zh = render_help(Language::Zh);
        assert!(zh.contains("像 man 一样查询你自己的命令笔记"), "{zh}");
        assert!(zh.contains("用法:"), "{zh}");
        assert!(zh.contains("命令:"), "{zh}");
        assert!(zh.contains("选项:"), "{zh}");
        assert!(zh.contains("指定笔记目录"), "{zh}");

        let en = render_help(Language::En);
        assert!(en.contains("Query your own command notes like man"), "{en}");
        assert!(en.contains("Usage:"), "{en}");
        assert!(en.contains("Commands:"), "{en}");
        assert!(en.contains("Options:"), "{en}");
    }

    /// 手写中文帮助曾漏掉 `-h`，本地化后必须与 clap 完全一致。
    #[test]
    fn both_languages_expose_the_same_flags() {
        let zh = render_help(Language::Zh);
        for flag in [
            "-h, --help",
            "-V, --version",
            "-b, --browser",
            "-e, --edit",
            "--notes-dir",
        ] {
            assert!(zh.contains(flag), "中文帮助缺少 `{flag}`:\n{zh}");
        }

        let en = render_help(Language::En);
        for flag in [
            "-h, --help",
            "-V, --version",
            "-b, --browser",
            "-e, --edit",
            "--notes-dir",
        ] {
            assert!(en.contains(flag), "英文帮助缺少 `{flag}`:\n{en}");
        }
    }

    #[test]
    fn subcommands_are_localized() {
        let zh = render_help(Language::Zh);
        assert!(zh.contains("列出所有笔记命令"), "{zh}");
        assert!(zh.contains("按文件名搜索笔记命令"), "{zh}");
    }

    #[test]
    fn listing_languages_stays_in_sync() {
        assert_eq!(Language::ALL.len(), 2);
    }

    #[test]
    fn derive_and_localized_command_share_the_same_arg_ids() {
        // `mut_arg` 在 id 不存在时会 panic，这里确保所有 id 都与派生定义一致。
        let derived = Cli::command();
        for id in ["notes_dir", "browser", "edit", "set_editor", "lang"] {
            assert!(
                derived.get_arguments().any(|arg| arg.get_id() == id),
                "缺少参数 {id}"
            );
        }
    }
}
