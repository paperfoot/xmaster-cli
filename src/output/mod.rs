pub mod csv;
pub mod json;
pub mod table;

use crate::errors::XmasterError;
use serde::Serialize;

pub use csv::CsvRenderable;
// Unused until the lead wires main.rs / agent_info.rs; drop the allow then.
#[allow(unused_imports)]
pub use json::set_pretty;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Json,
    Table,
    Csv,
}

impl OutputFormat {
    /// Detect output format from CLI flags. The single-argument form is kept for
    /// backward compatibility — callers that have a CSV flag should use `detect_full`.
    pub fn detect(json_flag: bool) -> Self {
        Self::detect_full(json_flag, false)
    }

    pub fn detect_full(json_flag: bool, csv_flag: bool) -> Self {
        if csv_flag {
            OutputFormat::Csv
        } else if json_flag || !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
            OutputFormat::Json
        } else {
            OutputFormat::Table
        }
    }
}

/// Render data in the selected format. CSV falls back to JSON for types that
/// only carry the default (single-cell) CsvRenderable implementation.
pub fn render<T: Serialize + Tableable>(
    format: OutputFormat,
    data: &T,
    metadata: Option<serde_json::Value>,
) {
    match format {
        OutputFormat::Json => json::render(data, metadata),
        OutputFormat::Table => table::render(data),
        // Types without a dedicated CsvRenderable impl will hit this branch
        // and get JSON output instead — CSV is opt-in per type.
        OutputFormat::Csv => json::render(data, metadata),
    }
}

/// Render data with full CSV support. Use this in commands whose display types
/// implement CsvRenderable with proper column definitions.
pub fn render_csv<T: Serialize + Tableable + CsvRenderable>(
    format: OutputFormat,
    data: &T,
    metadata: Option<serde_json::Value>,
) {
    match format {
        OutputFormat::Json => json::render(data, metadata),
        OutputFormat::Table => table::render(data),
        OutputFormat::Csv => csv::render(data, metadata),
    }
}

/// Render data plus a one-line hint for the agent or user (e.g. why a list is
/// empty and what to run next). Empty results are success, not errors.
#[allow(dead_code)]
pub fn render_hint<T: Serialize + Tableable>(format: OutputFormat, data: &T, hint: &str) {
    match format {
        OutputFormat::Table => {
            table::render(data);
            eprintln!("{hint}");
        }
        _ => render(format, data, Some(serde_json::json!({ "hint": hint }))),
    }
}

/// Render a failure that has no `XmasterError` (e.g. a clap parse error).
pub fn render_error(format: OutputFormat, code: &str, message: &str, suggestion: &str) {
    emit_error(format, &json::ErrorBody::plain(code, message, suggestion));
}

/// Render an `XmasterError` with its code, suggestion, `retryable` and
/// `retry_after_secs`. Pair with `std::process::exit(err.exit_code())`.
pub fn render_failure(format: OutputFormat, err: &XmasterError) {
    let message = err.to_string();
    let suggestion = err.suggestion();
    let retry_after_secs = err.retry_after_secs();
    emit_error(
        format,
        &json::ErrorBody {
            code: err.error_code(),
            message: &message,
            suggestion: &suggestion,
            retryable: retry_after_secs.is_some(),
            retry_after_secs,
        },
    );
}

fn emit_error(format: OutputFormat, body: &json::ErrorBody) {
    match format {
        OutputFormat::Json => json::render_error(body),
        OutputFormat::Table | OutputFormat::Csv => {
            eprintln!("Error: {}", body.message);
            if !body.suggestion.is_empty() {
                eprintln!("  Suggestion: {}", body.suggestion);
            }
        }
    }
}

pub trait Tableable {
    fn to_table(&self) -> comfy_table::Table;
}
