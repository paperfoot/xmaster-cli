//! FxTwitter client for Article enrichment.
//!
//! Articles (long-form posts on X) are NOT readable via the public v2 API —
//! the tweet's `text` is just a t.co URL, and the only first-party path to
//! the body is the private GraphQL endpoint with rotating operation IDs.
//!
//! FxTwitter (the community service powering Discord/Telegram link unfurls)
//! exposes the Article body at `https://api.fxtwitter.com/i/status/<id>` with
//! no auth required, returning the body as Draft.js-style content blocks.
//!
//! This module is a soft dependency: if FxTwitter is unreachable, we return
//! `None` and the caller falls back to the v2 API response unchanged.
//! Opt out by setting `keys.disable_fxtwitter = true` in config.
//!
//! Reference: https://github.com/FixTweet/FxTwitter

use serde::{Deserialize, Serialize};
use std::time::Duration;

const FX_TIMEOUT_MS: u64 = 2500;
const FX_BASE: &str = "https://api.fxtwitter.com";

/// Flattened Article that xmaster exposes in `read` / `metrics` / `timeline`
/// outputs. Compact by design — agents don't need the full Draft.js shape,
/// just the title and a flowing markdown-ish body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Article {
    pub id: String,
    pub title: String,
    /// Article body flattened to readable text. Block types map as follows:
    /// `header-two` -> `## `, `header-three` -> `### `,
    /// `unordered-list-item` -> `- `, `ordered-list-item` -> `1. `, others
    /// passed through. Blocks joined with `\n\n`.
    pub body: String,
    pub body_chars: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_image_url: Option<String>,
}

// --- FxTwitter API response shape (only the fields we read) ---

#[derive(Deserialize)]
struct FxResponse {
    tweet: Option<FxTweet>,
}

#[derive(Deserialize)]
struct FxTweet {
    article: Option<FxArticle>,
}

#[derive(Deserialize)]
struct FxArticle {
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    content: Option<FxArticleContent>,
    #[serde(default)]
    cover_media: Option<FxCoverMedia>,
}

#[derive(Deserialize)]
struct FxArticleContent {
    #[serde(default)]
    blocks: Vec<FxBlock>,
}

#[derive(Deserialize)]
struct FxBlock {
    #[serde(default)]
    text: String,
    #[serde(default, rename = "type")]
    block_type: String,
}

#[derive(Deserialize)]
struct FxCoverMedia {
    #[serde(default)]
    media_info: Option<FxMediaInfo>,
}

#[derive(Deserialize)]
struct FxMediaInfo {
    #[serde(default)]
    url: Option<String>,
}

// --- Public API ---

/// Fetch an Article for a tweet ID via FxTwitter. Returns `Ok(None)` when the
/// tweet doesn't have an Article (or when FxTwitter is unreachable / slow —
/// we degrade gracefully rather than fail). Returns `Ok(Some(_))` when an
/// Article body was retrieved.
pub async fn fetch_article(tweet_id: &str) -> Result<Option<Article>, ()> {
    let url = format!("{FX_BASE}/i/status/{tweet_id}");
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_millis(FX_TIMEOUT_MS))
        .user_agent("xmaster-cli/1.7.1 (Article enrichment via FxTwitter)")
        .build()
    {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };
    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };
    if !resp.status().is_success() {
        return Ok(None);
    }
    let body: FxResponse = match resp.json().await {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };
    let article = body
        .tweet
        .and_then(|t| t.article)
        .map(flatten_article);
    Ok(article)
}

fn flatten_article(a: FxArticle) -> Article {
    let title = a.title.unwrap_or_default();
    let body = a
        .content
        .map(|c| flatten_blocks(&c.blocks))
        .unwrap_or_default();
    let body_chars = body.chars().count();
    let cover_image_url = a
        .cover_media
        .and_then(|c| c.media_info)
        .and_then(|m| m.url);
    Article {
        id: a.id,
        title,
        body,
        body_chars,
        cover_image_url,
    }
}

fn flatten_blocks(blocks: &[FxBlock]) -> String {
    let mut out = String::new();
    for (i, b) in blocks.iter().enumerate() {
        if i > 0 {
            out.push_str("\n\n");
        }
        let prefix: &str = match b.block_type.as_str() {
            "header-one" => "# ",
            "header-two" => "## ",
            "header-three" => "### ",
            "header-four" => "#### ",
            "unordered-list-item" => "- ",
            "ordered-list-item" => "1. ",
            "blockquote" => "> ",
            _ => "",
        };
        out.push_str(prefix);
        out.push_str(&b.text);
    }
    out
}

/// Heuristic: a tweet looks like an Article wrapper if its text is just a
/// single t.co URL with nothing else. Articles are posted as a regular tweet
/// whose only content is the auto-generated Article link.
pub fn text_looks_like_article_wrapper(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with("https://t.co/")
        && !trimmed.contains(' ')
        && !trimmed.contains('\n')
        && trimmed.len() < 50
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_handles_headers_and_lists() {
        let blocks = vec![
            FxBlock { text: "Title".into(), block_type: "header-two".into() },
            FxBlock { text: "Body line one.".into(), block_type: "unstyled".into() },
            FxBlock { text: "Bullet one".into(), block_type: "unordered-list-item".into() },
            FxBlock { text: "Bullet two".into(), block_type: "unordered-list-item".into() },
            FxBlock { text: "Quote text".into(), block_type: "blockquote".into() },
        ];
        let out = flatten_blocks(&blocks);
        assert!(out.contains("## Title"));
        assert!(out.contains("- Bullet one"));
        assert!(out.contains("> Quote text"));
        assert!(out.contains("Body line one."));
    }

    #[test]
    fn detects_article_wrapper() {
        assert!(text_looks_like_article_wrapper("https://t.co/abc123XY"));
        assert!(!text_looks_like_article_wrapper("Check this https://t.co/abc out"));
        assert!(!text_looks_like_article_wrapper("just some text"));
        assert!(!text_looks_like_article_wrapper(
            "https://t.co/x https://t.co/y"
        ));
    }
}
