use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "gg", version, about = "Query your own command notes like man")]
#[command(subcommand_required = true, arg_required_else_help = true)]
pub struct Cli {
    #[arg(long, global = true, value_name = "DIR")]
    pub notes_dir: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Commands,
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
}

impl Cli {
    pub fn into_parts(self) -> (Option<PathBuf>, Action) {
        let action = match self.command {
            Commands::List => Action::List,
            Commands::Search { keyword } => Action::Search(keyword),
            Commands::Query(args) => {
                let command = args.join(" ");
                Action::Query(command)
            }
        };

        (self.notes_dir, action)
    }
}
