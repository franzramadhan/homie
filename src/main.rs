mod auth;
mod cache;
mod check;
mod config;
mod http;
mod integrations;
mod quiet;
mod secret;
mod server;

use std::{path::PathBuf, process::ExitCode};

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T, E = Error> = std::result::Result<T, E>;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "usage: homie [serve|check|hash-password|version] [--config <path>]

  serve          run the server (default)
  check          validate the config and test every integration
  hash-password  read a password from stdin, print its hash for local_login.password_hash
  version        print the version

Config: --config, then $HOMIE_CONFIG, then ~/.config/homie/config.toml,
then /etc/homie/config.toml. HOMIE__<SECTION>__<KEY> variables override it.";

#[tokio::main]
async fn main() -> ExitCode {
    http::init_tls();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_target(false)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .init();

    let mut args = std::env::args().skip(1);
    let mut cmd = None;
    let mut config_path = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" | "-c" => config_path = args.next().map(PathBuf::from),
            "-h" | "--help" | "help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "-V" | "--version" => cmd = Some("version".to_string()),
            _ if cmd.is_none() => cmd = Some(arg),
            _ => {
                eprintln!("unexpected argument {arg:?}\n\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }

    let result = match cmd.as_deref().unwrap_or("serve") {
        "version" => {
            println!("homie {VERSION}");
            return ExitCode::SUCCESS;
        }
        "hash-password" => auth::local::hash_password_cli(),
        "serve" => match config::load(config_path) {
            Ok(cfg) => server::run(cfg).await,
            Err(e) => Err(e),
        },
        "check" => match config::load(config_path) {
            Ok(cfg) => check::run(cfg).await,
            Err(e) => Err(e),
        },
        other => {
            eprintln!("unknown command {other:?}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
