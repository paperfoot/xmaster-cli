use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::intel::preflight;
use crate::intel::store::IntelStore;
use crate::output::{self, CsvRenderable, OutputFormat, Tableable};
use crate::providers::xapi::XApi;
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
struct ThreadResult {
    #[serde(rename = "ids")]
    tweet_ids: Vec<String>,
    total: usize,
    succeeded: usize,
    failed: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    hook_score: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hook_grade: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

impl Tableable for ThreadResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["#", "Tweet ID", "Status"]);
        for (i, id) in self.tweet_ids.iter().enumerate() {
            table.add_row(vec![
                (i + 1).to_string(),
                id.clone(),
                "Posted".to_string(),
            ]);
        }
        if self.failed > 0 {
            table.add_row(vec![
                "".to_string(),
                format!("{} tweet(s) failed", self.failed),
                "Failed".to_string(),
            ]);
        }
        if let Some(score) = self.hook_score {
            let grade = self.hook_grade.as_deref().unwrap_or("?");
            table.add_row(vec![
                "".to_string(),
                format!("Hook quality: {score}/100 ({grade})"),
                "".to_string(),
            ]);
        }
        table
    }
}

impl CsvRenderable for ThreadResult {
    fn csv_headers() -> Vec<&'static str> {
        vec!["index", "tweet_id", "status"]
    }
    fn csv_rows(&self) -> Vec<Vec<String>> {
        self.tweet_ids
            .iter()
            .enumerate()
            .map(|(i, id)| vec![(i + 1).to_string(), id.clone(), "posted".into()])
            .collect()
    }
}

pub async fn execute(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    texts: &[String],
    media: &[String],
) -> Result<(), XmasterError> {
    if texts.is_empty() {
        return Err(XmasterError::Api {
            provider: "x",
            code: "invalid_input",
            message: "Thread must contain at least one tweet".into(),
        });
    }

    let api = XApi::new(ctx.clone());

    // ── Pre-flight on the hook (first tweet) ──
    let analysis = preflight::analyze(&texts[0], Some("impressions"));
    let mut warnings = Vec::new();

    for issue in &analysis.issues {
        if issue.severity == preflight::Severity::Critical {
            warnings.push(format!("[CRITICAL] {}: {}", issue.code, issue.message));
        } else if issue.severity == preflight::Severity::Warning {
            warnings.push(format!("[WARN] {}", issue.message));
        }
    }

    if format == OutputFormat::Table {
        eprintln!(
            "--- Thread hook pre-flight ({}/100, {}) ---",
            analysis.score, analysis.grade
        );
        for w in &warnings {
            eprintln!("  {w}");
        }
        if !analysis.suggestions.is_empty() {
            eprintln!("  Tip: {}", analysis.suggestions[0]);
        }
        eprintln!("  Posting {} tweets with natural pacing...", texts.len());
        eprintln!("---");
    }

    // Upload media if provided (attach to first tweet only)
    let media_ids = if !media.is_empty() {
        let mut ids = Vec::new();
        for path in media {
            let id = api.upload_media(path).await?;
            ids.push(id);
        }
        Some(ids)
    } else {
        None
    };

    let mut posted_ids: Vec<String> = Vec::new();
    let mut failed = 0usize;

    for (i, text) in texts.iter().enumerate() {
        // ── Natural pacing between thread tweets (1-3 seconds) ──
        if i > 0 {
            let jitter_ms = 1000 + (rand::random::<u64>() % 2000);
            tokio::time::sleep(std::time::Duration::from_millis(jitter_ms)).await;
        }

        let reply_to = if i == 0 {
            None
        } else {
            posted_ids.last().map(|s| s.as_str())
        };
        let tweet_media = if i == 0 { media_ids.as_deref() } else { None };

        match api
            .create_tweet(text, reply_to, None, tweet_media, None, None)
            .await
        {
            Ok(resp) => {
                // Log each tweet to store
                if let Ok(store) = IntelStore::open() {
                    let content_type = if i == 0 { "thread_hook" } else { "thread_reply" };
                    let _ = store.log_post(
                        &resp.id,
                        text,
                        content_type,
                        reply_to,
                        None,
                        if i == 0 { Some(analysis.score as f64) } else { None },
                    );
                }
                posted_ids.push(resp.id);
            }
            Err(e) => {
                failed += 1;
                let remaining = texts.len() - i - 1;
                failed += remaining;
                eprintln!(
                    "Thread broken at tweet {}/{}: {e}. {} tweet(s) not posted.",
                    i + 1,
                    texts.len(),
                    remaining
                );
                break;
            }
        }
    }

    // If ALL tweets failed, return an error instead of a success envelope
    if posted_ids.is_empty() && !texts.is_empty() {
        return Err(XmasterError::Api {
            provider: "x",
            code: "thread_failed",
            message: format!("Thread failed: 0/{} tweets posted", texts.len()),
        });
    }

    let display = ThreadResult {
        total: texts.len(),
        succeeded: posted_ids.len(),
        failed,
        tweet_ids: posted_ids.clone(),
        hook_score: Some(analysis.score),
        hook_grade: Some(analysis.grade.clone()),
        warnings: if format == OutputFormat::Json { warnings } else { vec![] },
    };
    output::render(format, &display, None);

    // Undo hint
    if format == OutputFormat::Table && !posted_ids.is_empty() {
        eprintln!(
            "Delete thread: {}",
            posted_ids
                .iter()
                .map(|id| format!("xmaster delete {id}"))
                .collect::<Vec<_>>()
                .join(" && ")
        );
    }
    Ok(())
}
