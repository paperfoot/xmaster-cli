use crate::cli::parse_tweet_id;
use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::output::{self, OutputFormat, Tableable};
use crate::providers::xapi::{TweetData, TweetLookup, XApi};
use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, Serialize)]
struct EngageInboxResult {
    source_tweet_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    root_metrics: Option<RootMetrics>,
    totals: InboxTotals,
    heuristic: EngagementHeuristic,
    recommendations: Vec<InboxRecommendation>,
    quote_threads: Vec<QuoteThread>,
    suggested_next_commands: Vec<String>,
    notes: Vec<String>,
}

#[derive(Debug, Serialize)]
struct RootMetrics {
    impressions: u64,
    likes: u64,
    retweets: u64,
    replies: u64,
    quotes: u64,
    bookmarks: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_clicks: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url_clicks: Option<u64>,
}

#[derive(Debug, Serialize)]
struct InboxTotals {
    direct_replies_checked: usize,
    quote_tweets_checked: usize,
    quote_replies_checked: usize,
    recommended_actions: usize,
}

#[derive(Debug, Serialize)]
struct EngagementHeuristic {
    name: &'static str,
    rationale: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct InboxRecommendation {
    priority: u8,
    surface: String,
    action: String,
    target_id: String,
    target_url: String,
    author: String,
    likes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_at: Option<String>,
    reason: String,
    text: String,
}

#[derive(Debug, Serialize)]
struct QuoteThread {
    quote_id: String,
    quote_url: String,
    author: String,
    likes: u64,
    impressions: u64,
    text: String,
    replies_checked: usize,
    replies: Vec<QuoteReply>,
}

#[derive(Debug, Serialize)]
struct QuoteReply {
    id: String,
    author: String,
    likes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_at: Option<String>,
    text: String,
}

impl Tableable for EngageInboxResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec![
            "Priority", "Surface", "Action", "Target", "Author", "Why", "Text",
        ]);

        if self.recommendations.is_empty() {
            table.add_row(vec![
                "-".into(),
                "none".into(),
                "watch".into(),
                self.source_tweet_id.clone(),
                "-".into(),
                "No direct reply, quote, or quote-comment action found".into(),
                "-".into(),
            ]);
            return table;
        }

        for rec in &self.recommendations {
            table.add_row(vec![
                rec.priority.to_string(),
                rec.surface.clone(),
                rec.action.clone(),
                rec.target_id.clone(),
                rec.author.clone(),
                rec.reason.clone(),
                crate::utils::safe_truncate(&rec.text, 120).to_string(),
            ]);
        }
        table
    }
}

pub async fn execute(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    id: &str,
    count: usize,
    quote_reply_count: usize,
) -> Result<(), XmasterError> {
    let source_tweet_id = parse_tweet_id(id);
    let api = XApi::new(ctx);
    let mut notes = vec![
        "Native reposts do not have their own comments; this checks quote tweets/reposts-with-comment and replies under those quote tweets.".into(),
        "Recommendation priorities are read-only heuristics. The command never posts or drafts replies.".into(),
    ];

    let root_metrics = fetch_root_metrics(&api, &source_tweet_id).await?;
    let self_username = api
        .get_me()
        .await
        .ok()
        .map(|user| user.username.to_lowercase());
    if self_username.is_some() {
        notes.push(
            "Self-authored replies and quote tweets are excluded from recommendations.".into(),
        );
    }

    let direct_replies = match api.get_replies(&source_tweet_id, count).await {
        Ok(replies) => replies,
        Err(err) => {
            notes.push(format!(
                "Could not fetch direct replies for {source_tweet_id}: {err}"
            ));
            Vec::new()
        }
    };

    let quotes = match api.get_tweet_quotes(&source_tweet_id, count).await {
        Ok(quotes) => quotes,
        Err(err) => {
            notes.push(format!(
                "Could not fetch quote tweets for {source_tweet_id}: {err}"
            ));
            Vec::new()
        }
    };

    let mut recommendations = Vec::new();
    for reply in &direct_replies {
        if !is_self_authored(reply, self_username.as_deref()) {
            recommendations.push(recommend_direct_reply(reply));
        }
    }

    let mut quote_threads = Vec::new();
    let mut quote_replies_checked = 0usize;
    for quote in quotes {
        if !is_self_authored(&quote, self_username.as_deref()) {
            recommendations.push(recommend_quote(&quote));
        }

        let quote_replies = if quote_reply_count == 0 {
            Vec::new()
        } else {
            match api.get_replies(&quote.id, quote_reply_count).await {
                Ok(replies) => replies,
                Err(err) => {
                    notes.push(format!(
                        "Could not fetch replies under quote {}: {err}",
                        quote.id
                    ));
                    Vec::new()
                }
            }
        };

        quote_replies_checked += quote_replies.len();
        for reply in &quote_replies {
            if !is_self_authored(reply, self_username.as_deref()) {
                recommendations.push(recommend_quote_reply(reply, &quote));
            }
        }

        quote_threads.push(build_quote_thread(quote, quote_replies));
    }

    recommendations.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| b.likes.cmp(&a.likes))
            .then_with(|| b.created_at.cmp(&a.created_at))
    });

    let totals = InboxTotals {
        direct_replies_checked: direct_replies.len(),
        quote_tweets_checked: quote_threads.len(),
        quote_replies_checked,
        recommended_actions: recommendations.len(),
    };

    let suggested_next_commands = vec![
        format!("xmaster metrics {source_tweet_id}"),
        format!("xmaster replies {source_tweet_id} --count {count}"),
        format!("xmaster quotes {source_tweet_id} --count {count}"),
    ];

    let result = EngageInboxResult {
        source_tweet_id,
        root_metrics,
        totals,
        heuristic: author_reply_back_heuristic(),
        recommendations,
        quote_threads,
        suggested_next_commands,
        notes,
    };

    output::render(format, &result, None);
    Ok(())
}

async fn fetch_root_metrics(
    api: &XApi,
    source_tweet_id: &str,
) -> Result<Option<RootMetrics>, XmasterError> {
    let ids = vec![source_tweet_id.to_string()];
    let root = api.get_posts_by_ids(&ids).await?;
    Ok(root.first().map(root_metrics_from_lookup))
}

fn root_metrics_from_lookup(tweet: &TweetLookup) -> RootMetrics {
    let public = tweet.public_metrics.clone().unwrap_or_default();
    let non_public = tweet.non_public_metrics.clone();
    RootMetrics {
        impressions: public.impression_count,
        likes: public.like_count,
        retweets: public.retweet_count,
        replies: public.reply_count,
        quotes: public.quote_count,
        bookmarks: public.bookmark_count,
        profile_clicks: non_public.as_ref().map(|m| m.user_profile_clicks),
        url_clicks: non_public.as_ref().map(|m| m.url_link_clicks),
    }
}

fn recommend_direct_reply(tweet: &TweetData) -> InboxRecommendation {
    let question = is_question(&tweet.text);
    InboxRecommendation {
        priority: if question { 100 } else { 92 },
        surface: "direct_reply".into(),
        action: if question {
            "answer_reply".into()
        } else {
            "reply_back".into()
        },
        target_id: tweet.id.clone(),
        target_url: status_url(&tweet.id),
        author: author(tweet),
        likes: likes(tweet),
        created_at: tweet.created_at.clone(),
        reason: if question {
            "Question on your post — answer quickly to build the reply-back loop".into()
        } else {
            "Direct comment on your post — author reply-back is a strong reciprocity signal".into()
        },
        text: tweet.text.clone(),
    }
}

fn recommend_quote(tweet: &TweetData) -> InboxRecommendation {
    InboxRecommendation {
        priority: if is_question(&tweet.text) { 88 } else { 78 },
        surface: "quote_tweet".into(),
        action: "thank_or_extend_quote".into(),
        target_id: tweet.id.clone(),
        target_url: status_url(&tweet.id),
        author: author(tweet),
        likes: likes(tweet),
        created_at: tweet.created_at.clone(),
        reason: "Quote tweet amplified the post; reply lightly or ask one real question".into(),
        text: tweet.text.clone(),
    }
}

fn recommend_quote_reply(reply: &TweetData, quote: &TweetData) -> InboxRecommendation {
    let question = is_question(&reply.text);
    InboxRecommendation {
        priority: if question { 96 } else { 84 },
        surface: "quote_reply".into(),
        action: if question {
            "answer_quote_comment".into()
        } else {
            "join_quote_thread".into()
        },
        target_id: reply.id.clone(),
        target_url: status_url(&reply.id),
        author: author(reply),
        likes: likes(reply),
        created_at: reply.created_at.clone(),
        reason: format!(
            "Comment under @{}'s quote; second-order thread that manual metrics checks often miss",
            quote.author_username.as_deref().unwrap_or("unknown")
        ),
        text: reply.text.clone(),
    }
}

fn build_quote_thread(quote: TweetData, replies: Vec<TweetData>) -> QuoteThread {
    let quote_metrics = quote.public_metrics.clone();
    let quote_id = quote.id;
    let quote_url = status_url(&quote_id);
    QuoteThread {
        quote_id,
        quote_url,
        author: quote.author_username.unwrap_or_else(|| "unknown".into()),
        likes: quote_metrics.as_ref().map(|m| m.like_count).unwrap_or(0),
        impressions: quote_metrics
            .as_ref()
            .map(|m| m.impression_count)
            .unwrap_or(0),
        text: quote.text,
        replies_checked: replies.len(),
        replies: replies
            .into_iter()
            .map(|reply| QuoteReply {
                id: reply.id,
                author: reply.author_username.unwrap_or_else(|| "unknown".into()),
                likes: reply
                    .public_metrics
                    .as_ref()
                    .map(|m| m.like_count)
                    .unwrap_or(0),
                created_at: reply.created_at,
                text: reply.text,
            })
            .collect(),
    }
}

fn likes(tweet: &TweetData) -> u64 {
    tweet
        .public_metrics
        .as_ref()
        .map(|metrics| metrics.like_count)
        .unwrap_or(0)
}

fn author(tweet: &TweetData) -> String {
    tweet
        .author_username
        .as_ref()
        .map(|username| format!("@{username}"))
        .unwrap_or_else(|| "@unknown".into())
}

fn is_question(text: &str) -> bool {
    text.contains('?')
}

fn is_self_authored(tweet: &TweetData, self_username: Option<&str>) -> bool {
    match (&tweet.author_username, self_username) {
        (Some(author), Some(me)) => author.eq_ignore_ascii_case(me),
        _ => false,
    }
}

fn status_url(id: &str) -> String {
    format!("https://x.com/i/status/{id}")
}

fn author_reply_back_heuristic() -> EngagementHeuristic {
    EngagementHeuristic {
        name: "author_reply_back",
        rationale: "Replying back to people who replied to you builds a reciprocal engagement loop and accumulates dwell + follow_author signals. Priority ordering, not a fixed weight.",
    }
}
