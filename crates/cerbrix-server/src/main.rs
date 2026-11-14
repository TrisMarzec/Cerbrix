use std::path::PathBuf;
use std::process::ExitCode;

use cerbrix_server::admin::{self, AdminCommand};
use cerbrix_server::config::Config;
use cerbrix_server::state::StateStore;
use clap::{Parser, Subcommand};
use time::OffsetDateTime;

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
    #[command(subcommand)]
    Admin(AdminCommand),
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    let config = match Config::load(cli.config.as_deref()) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("configuration error: {err}");
            return ExitCode::from(2);
        }
    };

    match cli.command {
        Command::Serve => {
            eprintln!("cerbrix-server serve: not implemented yet");
            ExitCode::from(1)
        }
        Command::Admin(command) => {
            let result = async {
                let store = StateStore::connect(&config.database.url).await?;
                admin::run(&store, command, OffsetDateTime::now_utc()).await
            }
            .await;
            match result {
                Ok(text) => {
                    println!("{text}");
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("error: {err:#}");
                    ExitCode::from(1)
                }
            }
        }
    }
}
