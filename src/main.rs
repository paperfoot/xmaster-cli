mod browser_cookies;
mod cli;
pub mod transaction_id;
mod commands;
mod config;
mod context;
mod errors;
pub mod intel;
mod output;
mod providers;

use clap::Parser;
use cli::Cli;
use config::load_config;
use context::AppContext;
use output::OutputFormat;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    // Activate tracing — honours RUST_LOG (e.g. RUST_LOG=xmaster=debug)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let format = OutputFormat::detect(cli.json);

    let config = match load_config() {
        Ok(c) => c,
        Err(e) => {
            output::render_error(format, e.error_code(), &e.to_string(), &e.suggestion());
            std::process::exit(e.exit_code());
        }
    };

    let ctx = match AppContext::new(config) {
        Ok(c) => Arc::new(c),
        Err(e) => {
            output::render_error(format, e.error_code(), &e.to_string(), &e.suggestion());
            std::process::exit(e.exit_code());
        }
    };

    let result = commands::dispatch(ctx.clone(), &cli, format).await;

    if let Err(e) = result {
        output::render_error(format, e.error_code(), &e.to_string(), &e.suggestion());
        std::process::exit(e.exit_code());
    }
}
