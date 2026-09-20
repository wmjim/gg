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

    match gg::run(cli, lang) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{}: {err:#}", lang.error_prefix());
            // 2 = 命令行用错（含 gg 自己的取值校验），1 = 运行时失败。
            // 判定规则见 gg::error 的模块注释。
            ExitCode::from(gg::error::exit_code_for(&err))
        }
    }
}
