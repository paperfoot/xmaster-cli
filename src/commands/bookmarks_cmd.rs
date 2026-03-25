use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::intel::bookmarks::{BookmarkRecord, BookmarkStore};
use crate::output::{self, OutputFormat, Tableable};
use crate::providers::xapi::XApi;
use serde::Serialize;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Display types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct BookmarkList {
    bookmarks: Vec<BookmarkRow>,
    total: usize,
}

#[derive(Serialize)]
struct BookmarkRow {
    id: String,
    author: String,
    text: String,
    likes: i64,
    saved: String,
}

impl Tableable for BookmarkList {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["ID", "Author", "Text", "Likes", "Saved"]);
        for b in &self.bookmarks {
            let truncated = if b.text.len() > 60 {
                format!("{}...", &b.text[..57])
            } else {
                b.text.clone()
            };
            table.add_row(vec![
                &b.id,
                &b.author,
                &truncated,
                &b.likes.to_string(),
                &b.saved,
            ]);
        }
        table
    }
}

#[derive(Serialize)]
struct SyncDisplay {
    new_bookmarks: u32,
    already_stored: u32,
    total_in_db: u32,
    message: String,
}

impl Tableable for SyncDisplay {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Field", "Value"]);
        table.add_row(vec!["New", &self.new_bookmarks.to_string()]);
        table.add_row(vec!["Already stored", &self.already_stored.to_string()]);
        table.add_row(vec!["Total in archive", &self.total_in_db.to_string()]);
        table.add_row(vec!["Status", &self.message]);
        table
    }
}

#[derive(Serialize)]
struct ExportDisplay {
    count: usize,
    output: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
}

impl Tableable for ExportDisplay {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Field", "Value"]);
        table.add_row(vec!["Exported", &self.count.to_string()]);
        table.add_row(vec!["Output", &self.output]);
        table.add_row(vec!["Status", &self.message]);
        table
    }
}

#[derive(Serialize)]
struct DigestDisplay {
    period_days: u32,
    count: u32,
    unique_authors: usize,
    link_count: u32,
    text_count: u32,
    top_authors: Vec<AuthorSummary>,
}

#[derive(Serialize)]
struct AuthorSummary {
    username: String,
    count: u32,
}

impl Tableable for DigestDisplay {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Field", "Value"]);
        table.add_row(vec![
            "Period",
            &format!("Last {} days", self.period_days),
        ]);
        table.add_row(vec!["Bookmarks", &self.count.to_string()]);
        table.add_row(vec!["Authors", &self.unique_authors.to_string()]);
        table.add_row(vec!["With links", &self.link_count.to_string()]);
        table.add_row(vec!["Text only", &self.text_count.to_string()]);
        for a in &self.top_authors {
            table.add_row(vec![
                &format!("@{}", a.username),
                &format!("{} bookmarks", a.count),
            ]);
        }
        table
    }
}

#[derive(Serialize)]
struct StatsDisplay {
    total: u32,
    unread: u32,
    with_links: u32,
    with_media: u32,
    top_authors: Vec<(String, u32)>,
    oldest: Option<String>,
    newest: Option<String>,
}

impl Tableable for StatsDisplay {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Field", "Value"]);
        table.add_row(vec!["Total", &self.total.to_string()]);
        table.add_row(vec!["Unread", &self.unread.to_string()]);
        table.add_row(vec!["With links", &self.with_links.to_string()]);
        table.add_row(vec!["With media", &self.with_media.to_string()]);
        if let Some(ref o) = self.oldest {
            table.add_row(vec!["Oldest", o]);
        }
        if let Some(ref n) = self.newest {
            table.add_row(vec!["Newest", n]);
        }
        for (author, count) in &self.top_authors {
            table.add_row(vec![
                &format!("@{author}"),
                &format!("{count} bookmarks"),
            ]);
        }
        table
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn records_to_list(records: Vec<BookmarkRecord>) -> BookmarkList {
    let total = records.len();
    let bookmarks = records
        .into_iter()
        .map(|r| BookmarkRow {
            id: r.tweet_id,
            author: format!("@{}", r.author_username),
            text: r.text,
            likes: r.likes,
            saved: chrono::DateTime::from_timestamp(r.bookmarked_at, 0)
                .map(|dt| dt.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| r.bookmarked_at.to_string()),
        })
        .collect();
    BookmarkList { bookmarks, total }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

pub async fn list(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    count: usize,
    unread: bool,
) -> Result<(), XmasterError> {
    if unread {
        let store = BookmarkStore::open()?;
        let records = store.list_unread(count)?;
        if records.is_empty() {
            output::render_error(
                format,
                "no_unread_bookmarks",
                "No unread bookmarks found",
                "Sync bookmarks first: xmaster bookmarks sync",
            );
            return Ok(());
        }
        output::render(format, &records_to_list(records), None);
    } else {
        // Live from X API — bookmarks require OAuth 2.0
        let token = crate::providers::oauth2::ensure_oauth2_token(&ctx.config).await?;
        let api = XApi::new(ctx.clone());
        let user_id = api.get_me().await?.id;
        let per_page = count.min(100);
        let url = format!(
            "https://api.x.com/2/users/{}/bookmarks?max_results={}&tweet.fields=created_at,public_metrics,author_id&expansions=author_id&user.fields=username,name",
            user_id, per_page
        );
        let json = crate::providers::oauth2::oauth2_get(&url, &token).await?;
        let tweets = parse_bookmark_response(&json);

        let records: Vec<BookmarkRecord> = tweets
            .into_iter()
            .map(|t| {
                let metrics = t.public_metrics.as_ref();
                BookmarkRecord {
                    tweet_id: t.id,
                    author_username: t
                        .author_username
                        .unwrap_or_else(|| t.author_id.unwrap_or_default()),
                    author_name: None,
                    text: t.text,
                    created_at: t.created_at.clone(),
                    bookmarked_at: chrono::Utc::now().timestamp(),
                    likes: metrics.map(|m| m.like_count as i64).unwrap_or(0),
                    retweets: metrics.map(|m| m.retweet_count as i64).unwrap_or(0),
                    replies: metrics.map(|m| m.reply_count as i64).unwrap_or(0),
                    has_media: false,
                    has_link: false,
                    tags: String::new(),
                    notes: String::new(),
                    read: false,
                }
            })
            .collect();
        output::render(format, &records_to_list(records), None);
    }
    Ok(())
}

pub async fn sync(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    count: usize,
) -> Result<(), XmasterError> {
    // Bookmarks require OAuth 2.0 — get token (auto-refresh if needed)
    let token = crate::providers::oauth2::ensure_oauth2_token(&ctx.config).await?;

    // Fetch bookmarks via OAuth 2.0
    let user_id = {
        let api = XApi::new(ctx.clone());
        let me = api.get_me().await?;
        me.id
    };
    let per_page = count.min(100); // API max is 100 per request
    let base_url = format!(
        "https://api.x.com/2/users/{}/bookmarks?max_results={}&tweet.fields=created_at,public_metrics,author_id&expansions=author_id&user.fields=username,name",
        user_id, per_page
    );

    // Paginate through all bookmarks until count is reached or no more pages
    let mut tweets = Vec::new();
    let mut next_token: Option<String> = None;
    let mut remaining = count;

    loop {
        let url = match &next_token {
            Some(token_val) => format!("{}&pagination_token={}", base_url, token_val),
            None => base_url.clone(),
        };
        let json = crate::providers::oauth2::oauth2_get(&url, &token).await?;

        let page_tweets = parse_bookmark_response(&json);
        remaining = remaining.saturating_sub(page_tweets.len());
        tweets.extend(page_tweets);

        // Check for next page
        next_token = json
            .get("meta")
            .and_then(|m| m.get("next_token"))
            .and_then(|t| t.as_str())
            .map(|s| s.to_string());

        if next_token.is_none() || remaining == 0 {
            break;
        }
    }

    // Trim to requested count
    tweets.truncate(count);
    let store = BookmarkStore::open()?;
    let result = store.sync(tweets)?;

    let display = SyncDisplay {
        new_bookmarks: result.new_bookmarks,
        already_stored: result.already_stored,
        total_in_db: result.total_in_db,
        message: format!(
            "Synced: {} new, {} already stored. Total: {} in local archive",
            result.new_bookmarks, result.already_stored, result.total_in_db
        ),
    };
    output::render(format, &display, None);

    if format == OutputFormat::Table {
        eprintln!(
            "Search: xmaster bookmarks search \"query\"",
        );
        eprintln!(
            "Export: xmaster bookmarks export -o bookmarks.md",
        );
    }
    Ok(())
}

/// Parse X API v2 bookmarks response into TweetData for the store
fn parse_bookmark_response(json: &serde_json::Value) -> Vec<crate::providers::xapi::TweetData> {
    let mut tweets = Vec::new();
    let empty_arr = Vec::new();
    let data = json.get("data").and_then(|d| d.as_array()).unwrap_or(&empty_arr);

    // Build author lookup from includes.users
    let mut author_map = std::collections::HashMap::new();
    if let Some(includes) = json.get("includes") {
        if let Some(users) = includes.get("users").and_then(|u| u.as_array()) {
            for user in users {
                let id = user.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let username = user.get("username").and_then(|v| v.as_str()).unwrap_or("");
                let name = user.get("name").and_then(|v| v.as_str()).unwrap_or("");
                author_map.insert(id.to_string(), (username.to_string(), name.to_string()));
            }
        }
    }

    for tweet in data {
        let id = tweet.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let text = tweet.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let author_id = tweet.get("author_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let created_at = tweet.get("created_at").and_then(|v| v.as_str()).map(|s| s.to_string());
        let (username, _name) = author_map.get(&author_id).cloned().unwrap_or_default();

        let metrics = tweet.get("public_metrics").map(|m| crate::providers::xapi::TweetMetrics {
            like_count: m.get("like_count").and_then(|v| v.as_u64()).unwrap_or(0),
            retweet_count: m.get("retweet_count").and_then(|v| v.as_u64()).unwrap_or(0),
            reply_count: m.get("reply_count").and_then(|v| v.as_u64()).unwrap_or(0),
            impression_count: m.get("impression_count").and_then(|v| v.as_u64()).unwrap_or(0),
            bookmark_count: m.get("bookmark_count").and_then(|v| v.as_u64()).unwrap_or(0),
        });

        tweets.push(crate::providers::xapi::TweetData {
            id,
            text,
            author_id: Some(author_id),
            author_username: Some(username),
            created_at,
            public_metrics: metrics,
            author_followers: None,
            media_urls: vec![],
        });
    }
    tweets
}

pub async fn search(format: OutputFormat, query: &str) -> Result<(), XmasterError> {
    let store = BookmarkStore::open()?;
    let records = store.search(query)?;

    if records.is_empty() {
        output::render_error(
            format,
            "no_results",
            &format!("No bookmarks matching '{query}'"),
            "Try a broader search term or sync more bookmarks: xmaster bookmarks sync",
        );
        return Ok(());
    }

    output::render(format, &records_to_list(records), None);
    Ok(())
}

pub async fn export(
    format: OutputFormat,
    output_path: Option<&str>,
    unread: bool,
) -> Result<(), XmasterError> {
    let store = BookmarkStore::open()?;

    let records = if unread {
        store.list_unread(1000)?
    } else {
        store.search("")? // get all
    };

    if records.is_empty() {
        output::render_error(
            format,
            "no_bookmarks",
            "No bookmarks to export",
            "Sync bookmarks first: xmaster bookmarks sync",
        );
        return Ok(());
    }

    let count = records.len();
    let md = BookmarkStore::export_markdown(&records);

    // Write output first, THEN mark as read (so bookmarks aren't lost if write fails)
    let output_desc = match output_path {
        Some(path) => {
            std::fs::write(path, &md)?;
            path.to_string()
        }
        None => {
            if format == OutputFormat::Json {
                // For JSON output, embed markdown in the JSON envelope instead of
                // printing raw markdown before JSON
                let display = ExportDisplay {
                    count,
                    output: "json".to_string(),
                    message: format!("Exported {count} bookmarks (marked as read)"),
                    content: Some(md.clone()),
                };
                // Mark as read only after successful render
                for r in &records {
                    store.mark_read(&r.tweet_id)?;
                }
                output::render(format, &display, None);
                return Ok(());
            }
            println!("{md}");
            "stdout".to_string()
        }
    };

    // Mark exported bookmarks as read only after successful write
    for r in &records {
        store.mark_read(&r.tweet_id)?;
    }

    if output_path.is_some() {
        let display = ExportDisplay {
            count,
            output: output_desc,
            message: format!("Exported {count} bookmarks (marked as read)"),
            content: None,
        };
        output::render(format, &display, None);
    }
    Ok(())
}

pub async fn digest(format: OutputFormat, days: u32) -> Result<(), XmasterError> {
    let store = BookmarkStore::open()?;
    let digest = store.get_digest(days)?;

    if digest.count == 0 {
        output::render_error(
            format,
            "no_bookmarks_in_period",
            &format!("No bookmarks in the last {days} days"),
            "Sync bookmarks first: xmaster bookmarks sync",
        );
        return Ok(());
    }

    let display = DigestDisplay {
        period_days: digest.period_days,
        count: digest.count,
        unique_authors: digest.by_author.len(),
        link_count: digest.link_count,
        text_count: digest.text_count,
        top_authors: digest
            .by_author
            .iter()
            .take(10)
            .map(|a| AuthorSummary {
                username: a.username.clone(),
                count: a.count,
            })
            .collect(),
    };
    output::render(format, &display, None);
    Ok(())
}

pub async fn stats(format: OutputFormat) -> Result<(), XmasterError> {
    let store = BookmarkStore::open()?;
    let stats = store.get_stats()?;

    if stats.total == 0 {
        output::render_error(
            format,
            "no_bookmarks",
            "No bookmarks in local database",
            "Sync bookmarks first: xmaster bookmarks sync -c 200",
        );
        return Ok(());
    }

    let display = StatsDisplay {
        total: stats.total,
        unread: stats.unread,
        with_links: stats.with_links,
        with_media: stats.with_media,
        top_authors: stats.top_authors,
        oldest: stats.oldest,
        newest: stats.newest,
    };
    output::render(format, &display, None);
    Ok(())
}
