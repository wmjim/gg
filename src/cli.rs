use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "gg", version, about = "Query your own command notes like man")]
pub struct Cli {
    #[arg(long, global = true, value_name = "DIR")]
    pub notes_dir: Option<PathBuf>,

    /// Open markdown in default browser instead of terminal
    #[arg(short, long, global = true)]
    pub browser: bool,

    /// Open markdown in default editor for editing
    #[arg(short = 'e', long, global = true)]
    pub edit: bool,

    /// Set the default editor and save to config
    #[arg(long, global = true, value_name = "EDITOR")]
    pub set_editor: Option<String>,

    /// Set the display language (zh/en) and save to config
    #[arg(long, global = true, value_name = "LANG")]
    pub lang: Option<String>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// List all note commands
    List,
    /// Search note commands by file name
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
            Some(Commands::Query(args)) => {
                let command = args.join(" ");
                Action::Query(command)
            }
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









