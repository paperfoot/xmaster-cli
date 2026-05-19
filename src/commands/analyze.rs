use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::intel::preflight::{self, AnalyzeContext, PostMode, PreflightResult, Severity};
use crate::output::{self, OutputFormat, Tableable};
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
struct AnalyzeDisplay {
    #[serde(flatten)]
    result: PreflightResult,
    #[serde(skip)]
    premium: bool,
}

impl Tableable for AnalyzeDisplay {
    fn to_table(&self) -> comfy_table::Table {
        use comfy_table::{Attribute, Cell, Color};

        let mut table = comfy_table::Table::new();

        // Header section: Score + Grade
        table.set_header(vec!["Field", "Value"]);

        let grade_color = match self.result.grade.as_str() {
            "A" => Color::Green,
            "B" => Color::Cyan,
            "C" => Color::Yellow,
            "D" => Color::Red,
            _ => Color::DarkRed,
        };

        table.add_row(vec![
            Cell::new("Score"),
            Cell::new(format!("{}/100", self.result.score)).fg(grade_color),
        ]);
        table.add_row(vec![
            Cell::new("Grade"),
            Cell::new(&self.result.grade)
                .fg(grade_color)
                .add_attribute(Attribute::Bold),
        ]);
        table.add_row(vec![
            Cell::new("Type"),
            Cell::new(&self.result.features.content_type_guess),
        ]);
        table.add_row(vec![
            Cell::new("Characters"),
            Cell::new(format!("{}/{}", self.result.features.char_count, if self.premium { "25000" } else { "280" })),
        ]);
        table.add_row(vec![
            Cell::new("Hook Strength"),
            Cell::new(format!("{}/100", self.result.features.hook_strength)),
        ]);

        // Proxy signals section
        let ps = &self.result.proxy_scores;
        table.add_row(vec![
            Cell::new("").add_attribute(Attribute::Dim),
            Cell::new("--- Proxy Signals ---").add_attribute(Attribute::Dim),
        ]);
        let proxy_pairs: [(&str, f32); 9] = [
            ("P(reply)", ps.reply),
            ("P(quote)", ps.quote),
            ("P(profile_click)", ps.profile_click),
            ("P(follow)", ps.follow_author),
            ("P(DM share)", ps.share_via_dm),
            ("P(link share)", ps.share_via_copy_link),
            ("P(dwell)", ps.dwell),
            ("P(media_expand)", ps.media_expand),
            ("P(negative)", ps.negative_risk),
        ];
        for (label, val) in &proxy_pairs {
            if *val < 0.01 && *label != "P(negative)" {
                continue;
            }
            let color = if *label == "P(negative)" {
                if *val >= 0.30 {
                    Color::Red
                } else {
                    Color::DarkGrey
                }
            } else if *val >= 0.40 {
                Color::Green
            } else if *val >= 0.20 {
                Color::Yellow
            } else {
                Color::DarkGrey
            };
            table.add_row(vec![
                Cell::new(label),
                Cell::new(format!("{:.0}%", val * 100.0)).fg(color),
            ]);
        }

        // Goal scores section
        let gs = &self.result.goal_scores;
        table.add_row(vec![
            Cell::new("").add_attribute(Attribute::Dim),
            Cell::new("--- Goal Scores ---").add_attribute(Attribute::Dim),
        ]);
        let goal_pairs: [(&str, u32); 5] = [
            ("Replies", gs.replies),
            ("Quotes", gs.quotes),
            ("Shares", gs.shares),
            ("Follows", gs.follows),
            ("Impressions", gs.impressions),
        ];
        for (label, val) in &goal_pairs {
            let color = if *val >= 40 {
                Color::Green
            } else if *val >= 20 {
                Color::Yellow
            } else {
                Color::DarkGrey
            };
            table.add_row(vec![
                Cell::new(label),
                Cell::new(format!("{}/100", val)).fg(color),
            ]);
        }

        // Issues section
        if !self.result.issues.is_empty() {
            table.add_row(vec![
                Cell::new("").add_attribute(Attribute::Dim),
                Cell::new("--- Issues ---").add_attribute(Attribute::Dim),
            ]);
            for issue in &self.result.issues {
                let severity_color = match issue.severity {
                    Severity::Critical => Color::Red,
                    Severity::Warning => Color::Yellow,
                    Severity::Info => Color::Cyan,
                };
                table.add_row(vec![
                    Cell::new(format!("[{}]", issue.severity)).fg(severity_color),
                    Cell::new(&issue.message),
                ]);
            }
        }

        // Suggestions section
        if !self.result.suggestions.is_empty() {
            table.add_row(vec![
                Cell::new("").add_attribute(Attribute::Dim),
                Cell::new("--- Suggestions ---").add_attribute(Attribute::Dim),
            ]);
            for (i, suggestion) in self.result.suggestions.iter().enumerate() {
                table.add_row(vec![
                    Cell::new(format!("{}.", i + 1)),
                    Cell::new(suggestion),
                ]);
            }
        }

        // Next command
        if !self.result.suggested_next_commands.is_empty() {
            table.add_row(vec![
                Cell::new("").add_attribute(Attribute::Dim),
                Cell::new("--- Next ---").add_attribute(Attribute::Dim),
            ]);
            for cmd in &self.result.suggested_next_commands {
                table.add_row(vec![
                    Cell::new("Run"),
                    Cell::new(cmd).fg(Color::Green),
                ]);
            }
        }

        table
    }
}

pub async fn execute(
    app: Arc<AppContext>,
    format: OutputFormat,
    text: &str,
    goal: Option<&str>,
    reply_to: Option<&str>,
) -> Result<(), XmasterError> {
    // Auto-detect: if the input is a tweet ID or X URL, fetch the actual
    // content. For Article wrappers (text = single t.co), enrich via
    // FxTwitter and analyze the Article body instead of the t.co link.
    // Friction-free: agents don't need a separate command to score a post.
    let resolved_text = if let Some(id) = parse_tweet_reference(text) {
        match resolve_post_content(&app, &id).await {
            Ok(content) => content,
            Err(_) => text.to_string(), // fall back to raw text on failure
        }
    } else {
        text.to_string()
    };

    let premium = app.config.account.premium;
    let voice = if app.config.style.voice.is_empty() { None } else { Some(app.config.style.voice.clone()) };
    let mode = reply_to.map(|_| PostMode::Reply);
    let ctx = AnalyzeContext {
        goal: goal.map(|g| g.to_string()),
        mode,
        premium,
        author_voice: voice,
        ..Default::default()
    };
    let result = preflight::analyze(&resolved_text, &ctx);
    let display = AnalyzeDisplay { result, premium };
    output::render(format, &display, None);
    Ok(())
}

/// Detects whether the input refers to a tweet (numeric ID or X URL).
/// Returns the bare tweet ID if so. None for free-form text.
fn parse_tweet_reference(input: &str) -> Option<String> {
    let trimmed = input.trim();
    // URL form: https://x.com/<user>/status/<id> or twitter.com variant
    if let Some(idx) = trimmed.find("/status/") {
        let tail = &trimmed[idx + "/status/".len()..];
        let id: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
        if id.len() >= 15 {
            return Some(id);
        }
    }
    // Numeric ID form (X tweet IDs are 18-19 digits today; allow 15+)
    if trimmed.len() >= 15
        && trimmed.len() <= 25
        && trimmed.chars().all(|c| c.is_ascii_digit())
    {
        return Some(trimmed.to_string());
    }
    None
}

/// Fetch a post by ID. If the body is an Article wrapper, enrich with
/// FxTwitter so preflight scores the actual Article content, not the t.co.
async fn resolve_post_content(
    app: &Arc<AppContext>,
    tweet_id: &str,
) -> Result<String, XmasterError> {
    let api = crate::providers::xapi::XApi::new(app.clone());
    let tweet = api.get_tweet(tweet_id).await?;
    if !app.config.settings.disable_fxtwitter
        && crate::providers::fxtwitter::text_looks_like_article_wrapper(&tweet.text)
    {
        if let Ok(Some(article)) = crate::providers::fxtwitter::fetch_article(&tweet.id).await {
            // Title + body — gives preflight everything it needs to score the
            // long-form correctly (hook strength, payoff density, POV, etc.).
            let mut combined = String::new();
            if !article.title.is_empty() {
                combined.push_str(&article.title);
                combined.push_str("\n\n");
            }
            combined.push_str(&article.body);
            return Ok(combined);
        }
    }
    Ok(tweet.text)
}
