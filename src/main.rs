use clap::FromArgMatches;
use gg::cli::Cli;
use std::process::ExitCode;

fn main() -> ExitCode {
    // 先在 clap 解析前探测语言，让 `--help` / `--version` 也能本地化。
    // 这里不报配置错误：真正的错误由 `gg::run` 统一输出。
    let lang = gg::config::peek_language();

    let matches = gg::cli::command(lang).get_matches();
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(err) => err.exit(),
    };

    match gg::run(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{}: {err:#}", lang.error_prefix());
            ExitCode::FAILURE
        }
    }
}
