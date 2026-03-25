mod browser_cookies;
mod cli;
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
    let cli = Cli::parse();
    let format = OutputFormat::detect(cli.json);

    let config = match load_config() {
        Ok(c) => c,
        Err(e) => {
            output::render_error(format, e.error_code(), &e.to_string(), &e.suggestion());
            std::process::exit(e.exit_code());
        }
    };

    let ctx = Arc::new(AppContext::new(config));

    let result = commands::dispatch(ctx.clone(), &cli, format).await;

    if let Err(e) = result {
        output::render_error(format, e.error_code(), &e.to_string(), &e.suggestion());
        std::process::exit(e.exit_code());
    }
}
