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

    /// Answer yes to every prompt
    #[arg(short = 'y', long, global = true)]
    pub yes: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    List,
    Search {
        #[arg(value_name = "KEYWORD")]
        keyword: String,

        /// Search note bodies instead of file names
        #[arg(short = 'c', long)]
        content: bool,
    },
    #[command(external_subcommand)]
    Query(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Query(String),
    List,
    Search { keyword: String, content: bool },
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliParts {
    pub notes_dir: Option<PathBuf>,
    pub browser: bool,
    pub edit: bool,
    pub set_editor: Option<String>,
    pub lang: Option<String>,
    pub yes: bool,
    pub action: Action,
}

impl Cli {
    pub fn into_parts(self) -> CliParts {
        let action = match self.command {
            Some(Commands::List) => Action::List,
            Some(Commands::Search { keyword, content }) => Action::Search { keyword, content },
            Some(Commands::Query(args)) => Action::Query(args.join(" ")),
            None => Action::None,
        };

        CliParts {
            notes_dir: self.notes_dir,
            browser: self.browser,
            edit: self.edit,
            set_editor: self.set_editor,
            lang: self.lang,
            yes: self.yes,
            action,
        }
    }
}

/// 按语言构造本地化的 clap 命令。
///
/// 改造前 `app.rs` 里有一份手写的 `print_help_zh()`，与 clap 生成的英文帮助
/// 各自维护，已经漂移（手写版漏掉了 `-h`）。这里统一由 clap 生成，只有一份定义。
/// 本地化的分组标题。clap 要求 `&'static str`，故一次性泄漏几个短字符串。
struct Headings {
    usage: &'static str,
    commands: &'static str,
    arguments: &'static str,
    options: &'static str,
}

impl Headings {
    fn new(lang: Language) -> Self {
        Self {
            usage: leak(lang.cli_usage_heading()),
            commands: leak(lang.cli_heading_commands()),
            arguments: leak(lang.cli_heading_arguments()),
            options: leak(lang.cli_heading_options()),
        }
    }

    fn template(&self) -> String {
        format!(
            "{{about-with-newline}}\n{}: {{usage}}\n\n{{all-args}}{{after-help}}\n",
            self.usage
        )
    }
}

pub fn command(lang: Language) -> Command {
    use clap::CommandFactory;

    let headings = Headings::new(lang);

    // 子命令必须在 build 之前定制：`mut_subcommand` 会克隆子命令，
    // 对子命令单独调用 build() 会让位置参数解析失效
    // （`gg search gr` 会报 required arguments were not provided: <KEYWORD>）。
    let mut cmd = Cli::command()
        .about(lang.cli_about())
        .help_template(headings.template())
        .subcommand_help_heading(headings.commands)
        .mut_arg("notes_dir", |arg| {
            arg.help(lang.cli_arg_notes_dir())
                .help_heading(headings.options)
        })
        .mut_arg("browser", |arg| {
            arg.help(lang.cli_arg_browser())
                .help_heading(headings.options)
        })
        .mut_arg("edit", |arg| {
            arg.help(lang.cli_arg_edit()).help_heading(headings.options)
        })
        .mut_arg("set_editor", |arg| {
            arg.help(lang.cli_arg_set_editor())
                .help_heading(headings.options)
        })
        .mut_arg("lang", |arg| {
            arg.help(lang.cli_arg_lang()).help_heading(headings.options)
        })
        .mut_arg("yes", |arg| {
            arg.help(lang.cli_arg_yes()).help_heading(headings.options)
        })
        .mut_subcommand("list", |sub| {
            sub.about(lang.cli_cmd_list())
                .help_template(headings.template())
        })
        .mut_subcommand("search", |sub| {
            sub.about(lang.cli_cmd_search())
                .help_template(headings.template())
                .mut_arg("keyword", |arg| {
                    arg.help(lang.cli_cmd_search_keyword())
                        .help_heading(headings.arguments)
                })
                .mut_arg("content", |arg| {
                    arg.help(lang.cli_arg_search_content())
                        .help_heading(headings.options)
                })
        });

    // `-h/--help`、`-V/--version` 与 `help` 子命令由 clap 在 build 阶段注入，
    // 只能在这之后改它们的文案，否则 `mut_arg` 会直接 panic。
    cmd.build();
    cmd = cmd
        .mut_arg("help", |arg| {
            arg.help(lang.cli_arg_help()).help_heading(headings.options)
        })
        .mut_arg("version", |arg| {
            arg.help(lang.cli_arg_version())
                .help_heading(headings.options)
        })
        .mut_subcommand("help", |sub| sub.about(lang.cli_cmd_help()));

    for name in ["list", "search"] {
        cmd = cmd.mut_subcommand(name, |sub| {
            sub.mut_arg("help", |arg| {
                arg.help(lang.cli_arg_help()).help_heading(headings.options)
            })
        });
    }

    cmd
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
    fn search_subcommand_exposes_content_flag() {
        for lang in Language::ALL {
            let search_help = subcommand_help(lang, "search");
            assert!(
                search_help.contains("-c, --content"),
                "{lang:?} 的 search 帮助缺少 -c: {search_help}"
            );
        }
    }

    fn subcommand_help(lang: Language, name: &str) -> String {
        command(lang)
            .find_subcommand_mut(name)
            .unwrap_or_else(|| panic!("{name} 子命令存在"))
            .render_long_help()
            .to_string()
    }

    /// 子命令帮助必须完全本地化，不能中文标题里混着 `Options:`。
    #[test]
    fn subcommand_help_is_fully_localized() {
        let zh_search = subcommand_help(Language::Zh, "search");
        for expected in ["用法:", "参数:", "选项:", "搜索笔记正文而非文件名"] {
            assert!(
                zh_search.contains(expected),
                "缺少 `{expected}`:\n{zh_search}"
            );
        }
        for unexpected in ["Usage:", "Arguments:", "Options:", "Print help"] {
            assert!(
                !zh_search.contains(unexpected),
                "不应残留 `{unexpected}`:\n{zh_search}"
            );
        }

        let zh_list = subcommand_help(Language::Zh, "list");
        assert!(zh_list.contains("用法:"), "{zh_list}");
        assert!(!zh_list.contains("Options:"), "{zh_list}");

        let en_search = subcommand_help(Language::En, "search");
        for expected in ["Usage:", "Arguments:", "Options:", "Print help"] {
            assert!(
                en_search.contains(expected),
                "缺少 `{expected}`:\n{en_search}"
            );
        }
    }

    /// 回归：对子命令单独 build 过（或改动位置参数归属）会破坏位置参数解析，
    /// 表现为 `gg search gr` 报 required arguments were not provided。
    #[test]
    fn subcommand_positional_still_parses() {
        for lang in Language::ALL {
            let matches = command(lang)
                .try_get_matches_from(["gg", "--notes-dir", "/tmp/notes", "search", "gr"])
                .unwrap_or_else(|err| panic!("{lang:?} 解析失败: {err}"));

            let (name, sub) = matches.subcommand().expect("应有子命令");
            assert_eq!(name, "search");
            assert_eq!(
                sub.get_one::<String>("keyword").map(String::as_str),
                Some("gr")
            );
            assert!(!sub.get_flag("content"), "默认不应开启 --content");
        }
    }

    #[test]
    fn content_flag_parses_for_both_spellings() {
        for args in [
            vec!["gg", "search", "-c", "gr"],
            vec!["gg", "search", "--content", "gr"],
        ] {
            let matches = command(Language::Zh)
                .try_get_matches_from(args.clone())
                .unwrap_or_else(|err| panic!("{args:?} 解析失败: {err}"));
            let (_, sub) = matches.subcommand().expect("应有子命令");
            assert!(sub.get_flag("content"), "{args:?} 应开启 --content");
        }
    }

    #[test]
    fn listing_languages_stays_in_sync() {
        assert_eq!(Language::ALL.len(), 2);
    }

    #[test]
    fn derive_and_localized_command_share_the_same_arg_ids() {
        // `mut_arg` 在 id 不存在时会 panic，这里确保所有 id 都与派生定义一致。
        let derived = Cli::command();
        for id in ["notes_dir", "browser", "edit", "set_editor", "lang", "yes"] {
            assert!(
                derived.get_arguments().any(|arg| arg.get_id() == id),
                "缺少参数 {id}"
            );
        }
    }
}
