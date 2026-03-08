use clap::Parser;
use gg::cli::Cli;

fn main() {
    let cli = Cli::parse();
    if let Err(err) = gg::run(cli) {
        eprintln!("Error: {err:#}");
        std::process::exit(1);
    }
}
