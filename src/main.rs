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

use clap::error::ErrorKind;
use clap::Parser;
use cli::Cli;
use config::load_config;
use context::AppContext;
use output::OutputFormat;
use std::sync::Arc;

/// Pre-scan argv for --json before clap parses, so --json is honored on
/// help, version and parse errors. Stops at `--`: after it, "--json" is text.
fn has_json_flag() -> bool {
    std::env::args_os().take_while(|a| a != "--").any(|a| a == "--json")
}

/// Help and version exit 0; every other parse error exits 3 with a fix
/// scoped to the command being parsed. Help stays plain text when piped (one
/// readable page, not one escaped JSON string) unless --json asks for JSON.
fn exit_on_parse_error(e: clap::Error, json_flag: bool) -> ! {
    match e.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion if json_flag => {
            let data = if e.kind() == ErrorKind::DisplayVersion {
                serde_json::json!({"name": "xmaster", "version": env!("CARGO_PKG_VERSION")})
            } else {
                serde_json::json!({"usage": e.to_string().trim_end()})
            };
            output::json::render(&data, None);
            std::process::exit(0)
        }
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => e.exit(),
        _ => {
            let argv: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
            let (message, suggestion) = cli::explain_parse_error(&e, &argv);
            output::render_error(OutputFormat::detect(json_flag), "bad_input", &message, &suggestion);
            std::process::exit(3)
        }
    }
}

fn exit_on_failure(format: OutputFormat, e: &errors::XmasterError) -> ! {
    output::render_failure(format, e);
    std::process::exit(e.exit_code())
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
    let cli = Cli::try_parse().unwrap_or_else(|e| exit_on_parse_error(e, json_flag));

    let format = OutputFormat::detect_full(cli.json, cli.csv);
    if cli.pretty {
        output::set_pretty(true);
    }
    output::set_quiet(cli.quiet);

    // Discovery, `config path` and the contract hook need no config or HTTP
    // client: they answer even with a broken config file.
    if let Some(result) = commands::dispatch_local(&cli, format) {
        if let Err(e) = result {
            exit_on_failure(format, &e);
        }
        return;
    }

    let config = load_config().unwrap_or_else(|e| exit_on_failure(format, &e));
    let ctx = AppContext::new(config).map(Arc::new).unwrap_or_else(|e| exit_on_failure(format, &e));

    if let Err(e) = commands::dispatch(ctx, &cli, format).await {
        exit_on_failure(format, &e);
    }

    // One-time star nudge after a successful command
    if !cli.quiet {
        star_nudge::maybe_show(format);
    }
}
