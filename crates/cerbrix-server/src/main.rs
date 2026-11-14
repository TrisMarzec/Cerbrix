use std::path::PathBuf;
use std::process::ExitCode;

use cerbrix_server::config::Config;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "cerbrix-server", version, about = "Cerbrix monitoring server")]
struct Cli {
    /// Path to the TOML configuration file.
    #[arg(long, global = true, env = "CERBRIX_CONFIG", value_name = "PATH")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the server.
    Serve,
    /// Administrative commands.
    Admin,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let _config = match Config::load(cli.config.as_deref()) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("configuration error: {err}");
            return ExitCode::from(2);
        }
    };

    let name = match cli.command {
        Command::Serve => "serve",
        Command::Admin => "admin",
    };
    eprintln!("cerbrix-server {name}: not implemented yet");
    ExitCode::from(1)
}
