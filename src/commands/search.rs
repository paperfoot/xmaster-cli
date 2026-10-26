use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::intel::store::IntelStore;
use crate::output::{self, CsvRenderable, OutputFormat, Tableable};
use crate::providers::xapi::XApi;
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
struct SearchResults {
    query: String,
    tweets: Vec<TweetRow>,
}

#[derive(Serialize)]
struct TweetRow {
    id: String,
    author: String,
    text: String,
    impressions: u64,
    likes: u64,
    retweets: u64,
    replies: u64,
    date: String,
    age_minutes: i64,
    imps_per_min: f64,
}

/// Views per minute of age, one decimal; age floors at 1 so a brand-new post
/// doesn't divide by zero.
fn imps_per_min(impressions: u64, age_minutes: i64) -> f64 {
    (impressions as f64 / age_minutes.max(1) as f64 * 10.0).round() / 10.0
}

/// Client-side reorder, highest first; "date" is newest first. Stable, so
/// ties keep X's order.
fn sort_rows(rows: &mut [TweetRow], sort: &str) {
    match sort {
        "velocity" => rows.sort_by(|a, b| b.imps_per_min.total_cmp(&a.imps_per_min)),
        "impressions" | "views" => rows.sort_by_key(|t| std::cmp::Reverse(t.impressions)),
        "likes" => rows.sort_by_key(|t| std::cmp::Reverse(t.likes)),
        "reposts" | "retweets" | "rts" => rows.sort_by_key(|t| std::cmp::Reverse(t.retweets)),
        "date" => rows.sort_by_key(|t| t.age_minutes),
        // Other values are rejected by the CLI parser.
        _ => {}
    }
}

/// Compact human age: 45m, 6h, 3.0d. Lets the agent (and humans) see at a
/// glance whether a post is fresh enough to be worth replying to — reply
/// momentum dies after ~30-60 min.
fn human_age(minutes: i64) -> String {
    if minutes < 0 {
        "0m".into()
    } else if minutes < 60 {
        format!("{minutes}m")
    } else if minutes < 1440 {
        format!("{}h", minutes / 60)
    } else {
        format!("{:.1}d", minutes as f64 / 1440.0)
    }
}

impl Tableable for SearchResults {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["ID", "Author", "Age", "Text", "Views", "Views/min", "Likes", "RTs", "Replies", "Date"]);
        for t in &self.tweets {
            let truncated: String = if t.text.chars().count() > 80 {
                t.text.chars().take(77).collect::<String>() + "..."
            } else {
                t.text.clone()
            };
            table.add_row(vec![
                t.id.clone(),
                t.author.clone(),
                human_age(t.age_minutes),
                truncated,
                t.impressions.to_string(),
                format!("{:.1}", t.imps_per_min),
                t.likes.to_string(),
                t.retweets.to_string(),
                t.replies.to_string(),
                t.date.clone(),
            ]);
        }
        table
    }
}

impl CsvRenderable for SearchResults {
    fn csv_headers() -> Vec<&'static str> {
        vec!["id", "author", "age_minutes", "text", "impressions", "imps_per_min", "likes", "retweets", "replies", "date"]
    }

    fn csv_rows(&self) -> Vec<Vec<String>> {
        self.tweets
            .iter()
            .map(|t| {
                vec![
                    t.id.clone(),
                    t.author.clone(),
                    t.age_minutes.to_string(),
                    t.text.clone(),
                    t.impressions.to_string(),
                    format!("{:.1}", t.imps_per_min),
                    t.likes.to_string(),
                    t.retweets.to_string(),
                    t.replies.to_string(),
                    t.date.clone(),
                ]
            })
            .collect()
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn execute(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    query: &str,
    mode: &str,
    count: usize,
    since: Option<&str>,
    before: Option<&str>,
    sort: Option<&str>,
) -> Result<(), XmasterError> {
    let start_time: Option<String> = match since {
        Some(s) => Some(crate::commands::timeline::parse_since(s).map_err(XmasterError::Config)?),
        // No --since: floor at the last 24h. X recent-search spans 7 days, so
        // without a floor a low-volume query silently returns week-old posts that
        // are useless to reply to. Pass --since 7d to widen.
        None => Some(
            (chrono::Utc::now() - chrono::Duration::hours(24))
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        ),
    };
    let end_time = before.map(crate::commands::timeline::parse_since).transpose()
        .map_err(XmasterError::Config)?;
    let api = XApi::new(ctx.clone());
    let tweets = api.search_tweets_paginated(query, mode, count, start_time.as_deref(), end_time.as_deref()).await?;
    if let Ok(store) = IntelStore::open() {
        let _ = store.record_discovered_posts("search", &tweets);
    }
    let mut display = SearchResults {
        query: query.to_string(),
        tweets: tweets.into_iter().map(|t| {
            let metrics = t.public_metrics.as_ref();
            let age_minutes = t.created_at.as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| (chrono::Utc::now() - dt.with_timezone(&chrono::Utc)).num_minutes())
                .unwrap_or(0);
            let impressions = metrics.map(|m| m.impression_count).unwrap_or(0);
            TweetRow {
                id: t.id,
                author: t.author_username
                    .map(|u| format!("@{u}"))
                    .unwrap_or_else(|| t.author_id.unwrap_or_default()),
                text: t.text,
                impressions,
                likes: metrics.map(|m| m.like_count).unwrap_or(0),
                retweets: metrics.map(|m| m.retweet_count).unwrap_or(0),
                replies: metrics.map(|m| m.reply_count).unwrap_or(0),
                date: t.created_at.unwrap_or_default(),
                age_minutes,
                imps_per_min: imps_per_min(impressions, age_minutes),
            }
        }).collect(),
    };
    if let Some(sort) = sort {
        sort_rows(&mut display.tweets, sort);
    }
    output::render_csv(format, &display, None);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, impressions: u64, age_minutes: i64) -> TweetRow {
        TweetRow {
            id: id.into(),
            author: "@a".into(),
            text: String::new(),
            impressions,
            likes: 0,
            retweets: 0,
            replies: 0,
            date: String::new(),
            age_minutes,
            imps_per_min: imps_per_min(impressions, age_minutes),
        }
    }

    #[test]
    fn imps_per_min_is_views_over_age_to_one_decimal() {
        assert_eq!(imps_per_min(1000, 30), 33.3);
        assert_eq!(imps_per_min(90, 60), 1.5);
        assert_eq!(imps_per_min(0, 10), 0.0);
        // A post younger than a minute (or clock skew) counts as one minute.
        assert_eq!(imps_per_min(250, 0), 250.0);
        assert_eq!(imps_per_min(250, -3), 250.0);
    }

    #[test]
    fn velocity_sorts_most_views_per_minute_first() {
        // a: 10k over 5h = 33.3/min; b: 2k over 10m = 200/min; c: 600 over 2m = 300/min.
        let mut rows = vec![row("a", 10_000, 300), row("b", 2_000, 10), row("c", 600, 2)];
        sort_rows(&mut rows, "velocity");
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["c", "b", "a"]);
        sort_rows(&mut rows, "views");
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        sort_rows(&mut rows, "date");
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["c", "b", "a"]);
    }

    #[test]
    fn rate_shows_in_json_table_and_csv() {
        let results = SearchResults { query: "q".into(), tweets: vec![row("1", 1000, 30)] };
        let json = serde_json::to_value(&results).unwrap();
        assert_eq!(json["tweets"][0]["imps_per_min"], 33.3);
        assert!(results.to_table().to_string().contains("Views/min"));
        let col = SearchResults::csv_headers().iter().position(|h| *h == "imps_per_min").unwrap();
        assert_eq!(results.csv_rows()[0][col], "33.3");
    }
}
