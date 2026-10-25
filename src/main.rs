mod auth;
mod cache;
mod check;
mod config;
mod http;
mod integrations;
mod metrics;
mod notify;
mod quiet;
mod roles;
mod secret;
mod server;
mod status;
#[cfg(test)]
mod testkit;
mod tls;
mod upstream;

use std::{path::PathBuf, process::ExitCode};

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T, E = Error> = std::result::Result<T, E>;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "usage: homie [serve|check|notify-test|hash-password|version] [--config <path>]

  serve          run the server (default)
  check          validate the config and test every integration
  notify-test    send a test message to every [notify] target
  hash-password  read a password from stdin, print its hash for local_login.password_hash
  version        print the version

Config: --config, then $HOMIE_CONFIG, then ~/.config/homie/config.toml,
then /etc/homie/config.toml. HOMIE__<SECTION>__<KEY> variables override it.";

/// Logging comes after the config: `log_format` is in it (RFC 0024).
fn load(path: Option<PathBuf>) -> Result<config::Config> {
    let cfg = config::load(path)?;
    init_logs(cfg.log_format);
    config::log_source(&cfg);
    Ok(cfg)
}

fn init_logs(format: config::LogFormat) {
    let logs = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_target(false);
    match format {
        config::LogFormat::Json => logs.json().flatten_event(true).init(),
        config::LogFormat::Text => logs
            .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
            .init(),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    http::init_tls();

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
        "hash-password" => {
            init_logs(config::LogFormat::Text);
            auth::local::hash_password_cli()
        }
        "serve" => match load(config_path) {
            Ok(cfg) => server::run(cfg).await,
            Err(e) => Err(e),
        },
        "check" => match load(config_path) {
            Ok(cfg) => check::run(cfg).await,
            Err(e) => Err(e),
        },
        "notify-test" => match load(config_path) {
            Ok(cfg) => notify::test(&cfg).await,
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
