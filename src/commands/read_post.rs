use crate::cli::parse_tweet_id;
use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::intel::store::IntelStore;
use crate::output::{self, OutputFormat, Tableable};
use crate::providers::fxtwitter::{self, Article};
use crate::providers::xapi::XApi;
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
struct PostDisplay {
    id: String,
    author: String,
    text: String,
    likes: u64,
    retweets: u64,
    replies: u64,
    impressions: u64,
    bookmarks: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    date: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    media_urls: Vec<String>,
    /// X Article body, surfaced via FxTwitter when the tweet text is just a
    /// t.co wrapper for an Article. `None` for regular tweets.
    #[serde(skip_serializing_if = "Option::is_none")]
    article: Option<Article>,
}

impl Tableable for PostDisplay {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Field", "Value"]);
        table.add_row(vec!["ID", &self.id]);
        table.add_row(vec!["Author", &self.author]);
        table.add_row(vec!["Text", &self.text]);
        table.add_row(vec!["Likes", &self.likes.to_string()]);
        table.add_row(vec!["Retweets", &self.retweets.to_string()]);
        table.add_row(vec!["Replies", &self.replies.to_string()]);
        table.add_row(vec!["Impressions", &self.impressions.to_string()]);
        table.add_row(vec!["Bookmarks", &self.bookmarks.to_string()]);
        if let Some(ref date) = self.date {
            table.add_row(vec!["Date", date]);
        }
        for url in &self.media_urls {
            table.add_row(vec!["Media", url]);
        }
        if let Some(ref art) = self.article {
            table.add_row(vec!["Article ID", &art.id]);
            table.add_row(vec!["Article Title", &art.title]);
            table.add_row(vec![
                "Article Body".to_string(),
                format!("{} chars\n\n{}", art.body_chars, &art.body),
            ]);
        }
        table
    }
}

pub async fn execute(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    id: &str,
) -> Result<(), XmasterError> {
    let api = XApi::new(ctx.clone());
    let tweet_id = parse_tweet_id(id);
    let tweet = api.get_tweet(&tweet_id).await?;
    if let Ok(store) = IntelStore::open() {
        let _ = store.record_discovered_post("read", &tweet);
    }

    // Article enrichment: if the tweet text looks like a t.co Article wrapper,
    // try FxTwitter (no auth, ~2.5s timeout, graceful degradation). Skip when
    // disabled in config.
    let article = if !ctx.config.settings.disable_fxtwitter
        && fxtwitter::text_looks_like_article_wrapper(&tweet.text)
    {
        fxtwitter::fetch_article(&tweet.id).await.ok().flatten()
    } else {
        None
    };

    let metrics = tweet.public_metrics.as_ref();
    let display = PostDisplay {
        id: tweet.id,
        author: tweet
            .author_username
            .map(|u| format!("@{u}"))
            .unwrap_or_else(|| tweet.author_id.unwrap_or_default()),
        text: tweet.text,
        likes: metrics.map(|m| m.like_count).unwrap_or(0),
        retweets: metrics.map(|m| m.retweet_count).unwrap_or(0),
        replies: metrics.map(|m| m.reply_count).unwrap_or(0),
        impressions: metrics.map(|m| m.impression_count).unwrap_or(0),
        bookmarks: metrics.map(|m| m.bookmark_count).unwrap_or(0),
        date: tweet.created_at,
        media_urls: tweet.media_urls,
        article,
    };
    output::render(format, &display, None);
    Ok(())
}
