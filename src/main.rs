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
mod star_nudge;
pub mod utils;

use clap::Parser;
use cli::Cli;
use config::load_config;
use context::AppContext;
use output::OutputFormat;
use std::sync::Arc;

/// Pre-scan argv for --json before clap parses. This ensures --json is
/// honored on help, version, and parse-error paths (agent-cli-framework).
fn has_json_flag() -> bool {
    std::env::args_os().any(|a| a == "--json")
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // Activate tracing — honours RUST_LOG (e.g. RUST_LOG=xmaster=debug)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let json_flag = has_json_flag();

    // try_parse so we own the exit code, not clap.
    // --help and --version exit 0 (not 3). Parse errors exit 3.
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            if matches!(
                e.kind(),
                clap::error::ErrorKind::DisplayHelp
                    | clap::error::ErrorKind::DisplayVersion
            ) {
                // Help/version are informational — always exit 0.
                e.exit();
            }
            // Parse errors — we own the exit code. Exit 3 (bad input).
            let format = OutputFormat::detect(json_flag);
            output::render_error(format, "bad_input", &e.to_string(), "Check arguments with: xmaster --help");
            std::process::exit(3);
        }
    };

    let format = OutputFormat::detect_full(cli.json, cli.csv);
    if cli.pretty {
        output::set_pretty(true);
    }

    let config = match load_config() {
        Ok(c) => c,
        Err(e) => {
            output::render_failure(format, &e);
            std::process::exit(e.exit_code());
        }
    };

    let ctx = match AppContext::new(config) {
        Ok(c) => Arc::new(c),
        Err(e) => {
            output::render_failure(format, &e);
            std::process::exit(e.exit_code());
        }
    };

    let result = commands::dispatch(ctx.clone(), &cli, format).await;

    if let Err(e) = result {
        output::render_failure(format, &e);
        std::process::exit(e.exit_code());
    }

    // One-time star nudge after a successful command
    star_nudge::maybe_show(format);
}
