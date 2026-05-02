use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::intel::store::IntelStore;
use crate::output::{self, OutputFormat, Tableable};
use crate::providers::xai::XaiSearch;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct RecommendCandidate {
    pub rank: usize,
    pub username: String,
    pub followers: u64,
    pub reply_rate: f64,
    pub score: f64,
    pub source: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
}

/// Adaptive follower band based on user's own follower count.
/// Targets accounts 2x-20x your size for optimal reply ROI.
pub fn default_target_band(my_followers: u64) -> (u64, u64) {
    let min = (my_followers * 2).clamp(500, 5_000);
    let max = (my_followers * 20).clamp(5_000, 100_000);
    (min, max)
}

fn compute_size_fit(target_followers: u64, my_followers: u64) -> f64 {
    let (min, max) = default_target_band(my_followers);
    if target_followers >= min && target_followers <= max {
        1.0
    } else if target_followers < min {
        (target_followers as f64 / min as f64).max(0.2)
    } else {
        (max as f64 / target_followers as f64).max(0.1)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RecommendResult {
    pub candidates: Vec<RecommendCandidate>,
    pub suggested_next_commands: Vec<String>,
}

impl Tableable for RecommendResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Rank", "@Username", "Followers", "Reply Rate", "Score", "Source"]);
        for c in &self.candidates {
            table.add_row(vec![
                c.rank.to_string(),
                format!("@{}", c.username),
                format_followers(c.followers),
                if c.reply_rate > 0.0 {
                    format!("{:.0}%", c.reply_rate * 100.0)
                } else {
                    "—".into()
                },
                format!("{:.2}", c.score),
                c.source.clone(),
            ]);
        }
        table
    }
}

fn format_followers(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

// ---------------------------------------------------------------------------
// Candidate collection (internal)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct RawCandidate {
    username: String,
    followers: u64,
    reply_rate: f64,
    source: String,
    relevance: f64,
}

// ---------------------------------------------------------------------------
// Command handler
// ---------------------------------------------------------------------------

pub async fn recommend(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    topic: Option<&str>,
    min_followers: u32,
    count: usize,
) -> Result<(), XmasterError> {
    let mut candidates: HashMap<String, RawCandidate> = HashMap::new();

    // Phase 1a: Local history — proven reciprocators
    if let Ok(store) = IntelStore::open() {
        if let Ok(reciprocators) = store.get_top_reciprocators(min_followers as i64, 20) {
            for r in reciprocators {
                let username = r.username.to_lowercase();
                candidates.entry(username.clone()).or_insert(RawCandidate {
                    username: r.username,
                    followers: r.avg_followers as u64,
                    reply_rate: r.reply_rate,
                    source: "history".into(),
                    relevance: 0.3,
                });
            }
        }
    }

    // Phase 1b: Live mentions — people already talking to you
    let xapi = crate::providers::xapi::XApi::new(ctx.clone());
    if let Ok(user_id) = xapi.get_authenticated_user_id().await {
        if let Ok(mentions) = xapi.get_user_mentions(&user_id, 20).await {
            if let Ok(store) = IntelStore::open() {
                let _ = store.record_discovered_posts("recommend_mentions", &mentions);
            }
            for tweet in &mentions {
                if let Some(username) = &tweet.author_username {
                    let key = username.to_lowercase();
                    if candidates.contains_key(&key) {
                        continue;
                    }
                    let followers = tweet.author_followers.unwrap_or(0);
                    candidates.entry(key).or_insert(RawCandidate {
                        username: username.clone(),
                        followers,
                        reply_rate: 0.0,
                        source: "mentions".into(),
                        relevance: 0.7,
                    });
                }
            }
        }
    }

    // Phase 1c: Topic discovery via X user search (structured, with follower data)
    // then xAI text search as fallback for additional candidates.
    if let Some(topic_str) = topic {
        // Step 1: structured user search — returns verified accounts with real metrics
        let xapi = crate::providers::xapi::XApi::new(ctx.clone());
        if let Ok(users) = xapi.search_users(topic_str, 20).await {
            for user in users {
                let key = user.username.to_lowercase();
                if candidates.contains_key(&key) {
                    continue;
                }
                let followers = user
                    .public_metrics
                    .as_ref()
                    .map(|m| m.followers_count)
                    .unwrap_or(0);
                candidates.entry(key).or_insert(RawCandidate {
                    username: user.username,
                    followers,
                    reply_rate: 0.0,
                    source: "user_search".into(),
                    relevance: 1.0,
                });
            }
        }

        // Step 2: xAI text search as fallback for additional candidates
        let xai = XaiSearch::new(ctx.clone());
        if let Ok(result) = xai.search_posts(topic_str, 20, None, None, None).await {
            let usernames = extract_usernames_from_text(&result.text);
            for username in usernames {
                let key = username.to_lowercase();
                if candidates.contains_key(&key) {
                    continue;
                }
                candidates.entry(key).or_insert(RawCandidate {
                    username,
                    followers: 0,
                    reply_rate: 0.0,
                    source: "topic_xai".into(),
                    relevance: 0.8,
                });
            }
        }
    }

    // Phase 1d: Enrich with reciprocity data from store
    if let Ok(store) = IntelStore::open() {
        for (_, cand) in candidates.iter_mut() {
            if cand.reply_rate == 0.0 {
                if let Ok(Some(info)) = store.get_engagement_reciprocity(&cand.username) {
                    cand.reply_rate = info.reply_rate;
                }
            }
        }
    }

    // Filter by min_followers (skip candidates with 0 followers unless from topic/mentions)
    let filtered: Vec<RawCandidate> = candidates
        .into_values()
        .filter(|c| c.followers >= min_followers as u64 || c.source != "history")
        .collect();

    if filtered.is_empty() {
        return Err(XmasterError::NotFound(
            "No recommendation candidates found. Try: `xmaster engage recommend --topic \"your niche\"` or engage with more accounts first".into(),
        ));
    }

    // Phase 2: Score with opportunity model
    // Try to get user's follower count for adaptive sizing
    let my_followers = {
        let api = crate::providers::xapi::XApi::new(ctx.clone());
        api.get_me().await.ok().and_then(|u| u.public_metrics.as_ref().map(|m| m.followers_count)).unwrap_or(100) as u64
    };

    let mut scored: Vec<RecommendCandidate> = filtered
        .into_iter()
        .map(|c| {
            let reciprocity = c.reply_rate;
            let reach = if c.followers > 0 {
                ((c.followers as f64).log2() / 20.0).min(1.0)
            } else {
                0.0
            };
            let size_fit = compute_size_fit(c.followers, my_followers);
            let relevance = c.relevance;

            // Opportunity scoring: reply_roi proxy + size_fit + reciprocity + reach + relevance
            let score = 0.25 * reciprocity + 0.25 * size_fit + 0.20 * reach + 0.20 * relevance + 0.10 * 1.0;

            let mut reasons = Vec::new();
            if reciprocity > 0.3 { reasons.push(format!("replied back {:.0}% of the time", reciprocity * 100.0)); }
            if size_fit > 0.8 { reasons.push("in your ideal follower band".into()); }
            if reach > 0.6 { reasons.push("large audience amplifies your reply".into()); }
            if relevance > 0.5 { reasons.push("topically relevant".into()); }

            RecommendCandidate {
                rank: 0,
                username: c.username,
                followers: c.followers,
                reply_rate: c.reply_rate,
                score,
                source: c.source,
                reasons,
            }
        })
        .collect();

    // Phase 3: Rank
    scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(count);
    for (i, c) in scored.iter_mut().enumerate() {
        c.rank = i + 1;
    }

    let suggested_next_commands: Vec<String> = scored
        .iter()
        .map(|c| format!("xmaster search \"from:{}\" -c 5", c.username))
        .collect();

    let result = RecommendResult {
        candidates: scored,
        suggested_next_commands,
    };

    let metadata = serde_json::json!({
        "suggested_next_commands": result.suggested_next_commands,
    });

    output::render(format, &result, Some(metadata));
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Watchlist CRUD
// ---------------------------------------------------------------------------

pub async fn watchlist_add(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    username: &str,
    topic: Option<&str>,
) -> Result<(), XmasterError> {
    let store = IntelStore::open().map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;
    let api = crate::providers::xapi::XApi::new(ctx.clone());

    // Fetch user info to get ID and follower count
    let user = api.get_user_by_username(username).await?;
    let followers = user.public_metrics.as_ref().map(|m| m.followers_count as i64).unwrap_or(0);

    store.add_watchlist(username, Some(&user.id), topic, followers)
        .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;

    #[derive(Serialize)]
    struct WatchlistAddResult { username: String, user_id: String, followers: i64, topic: Option<String>, status: String }
    impl Tableable for WatchlistAddResult {
        fn to_table(&self) -> comfy_table::Table {
            let mut t = comfy_table::Table::new();
            t.set_header(vec!["Field", "Value"]);
            t.add_row(vec!["Username", &format!("@{}", self.username)]);
            t.add_row(vec!["Followers", &format_followers(self.followers as u64)]);
            t.add_row(vec!["Status", &self.status]);
            t
        }
    }
    let display = WatchlistAddResult {
        username: username.to_string(), user_id: user.id, followers, topic: topic.map(String::from), status: "added".into(),
    };
    output::render(format, &display, None);
    Ok(())
}

pub async fn watchlist_list(format: OutputFormat) -> Result<(), XmasterError> {
    let store = IntelStore::open().map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;
    let entries = store.list_watchlist().map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;

    if entries.is_empty() {
        return Err(XmasterError::NotFound("Watchlist is empty. Add accounts with: xmaster engage watchlist add <username>".into()));
    }

    #[derive(Serialize)]
    struct WatchlistDisplay { accounts: Vec<crate::intel::store::WatchlistEntry> }
    impl Tableable for WatchlistDisplay {
        fn to_table(&self) -> comfy_table::Table {
            let mut t = comfy_table::Table::new();
            t.set_header(vec!["Username", "Followers", "Topic"]);
            for a in &self.accounts {
                t.add_row(vec![
                    format!("@{}", a.username),
                    format_followers(a.followers as u64),
                    a.topic.clone().unwrap_or_default(),
                ]);
            }
            t
        }
    }

    output::render(format, &WatchlistDisplay { accounts: entries }, None);
    Ok(())
}

pub async fn watchlist_remove(format: OutputFormat, username: &str) -> Result<(), XmasterError> {
    let store = IntelStore::open().map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;
    let removed = store.remove_watchlist(username).map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;

    if !removed {
        return Err(XmasterError::NotFound(format!("@{username} not in watchlist")));
    }

    #[derive(Serialize)]
    struct RemoveResult { username: String, status: String }
    impl Tableable for RemoveResult {
        fn to_table(&self) -> comfy_table::Table {
            let mut t = comfy_table::Table::new();
            t.add_row(vec![&format!("@{} removed from watchlist", self.username)]);
            t
        }
    }
    output::render(format, &RemoveResult { username: username.to_string(), status: "removed".into() }, None);
    Ok(())
}

// ---------------------------------------------------------------------------
// Hot targets — rank accounts by downstream reply performance
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct HotTargetRow {
    rank: usize,
    username: String,
    sample_count: i64,
    avg_impressions: f64,
    avg_profile_clicks: f64,
    reply_back_rate: f64,
    score: f64,
    last_reply_at: String,
}

#[derive(Serialize)]
struct HotTargetsResult {
    period_days: i64,
    sort: String,
    targets: Vec<HotTargetRow>,
    suggested_next_commands: Vec<String>,
}

impl Tableable for HotTargetsResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut t = comfy_table::Table::new();
        t.set_header(vec!["Rank", "@Username", "Replies", "Avg Imps", "Avg PClicks", "Reply-Back %", "Score"]);
        for row in &self.targets {
            t.add_row(vec![
                row.rank.to_string(),
                format!("@{}", row.username),
                row.sample_count.to_string(),
                format!("{:.0}", row.avg_impressions),
                format!("{:.1}", row.avg_profile_clicks),
                format!("{:.0}%", row.reply_back_rate * 100.0),
                format!("{:.2}", row.score),
            ]);
        }
        t
    }
}

pub async fn hot_targets(
    format: OutputFormat,
    days: i64,
    min_imps: i64,
    min_profile_clicks: i64,
    min_samples: i64,
    count: usize,
    sort: &str,
) -> Result<(), XmasterError> {
    let store = IntelStore::open().map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;
    let mut rows = store
        .rank_hot_reply_targets(days, min_samples, min_imps as f64, min_profile_clicks as f64)
        .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;

    // Optional caller-requested re-sort — default `score` is already applied by the store.
    match sort {
        "avg-impressions" => rows.sort_by(|a, b| b.avg_impressions.partial_cmp(&a.avg_impressions).unwrap_or(std::cmp::Ordering::Equal)),
        "avg-profile-clicks" => rows.sort_by(|a, b| b.avg_profile_clicks.partial_cmp(&a.avg_profile_clicks).unwrap_or(std::cmp::Ordering::Equal)),
        "reply-back-rate" => rows.sort_by(|a, b| b.reply_back_rate.partial_cmp(&a.reply_back_rate).unwrap_or(std::cmp::Ordering::Equal)),
        "score" => { /* already sorted by store */ }
        _ => { /* unknown sort: keep store score order */ }
    }

    rows.truncate(count);

    if rows.is_empty() {
        return Err(XmasterError::NotFound(format!(
            "No hot reply targets in the last {days} days (min_samples={min_samples}, min_imps={min_imps}). \
             Reply to more posts and run `xmaster track run` to capture metrics, then try again."
        )));
    }

    let targets: Vec<HotTargetRow> = rows
        .into_iter()
        .enumerate()
        .map(|(i, r)| HotTargetRow {
            rank: i + 1,
            username: r.username,
            sample_count: r.sample_count,
            avg_impressions: r.avg_impressions,
            avg_profile_clicks: r.avg_profile_clicks,
            reply_back_rate: r.reply_back_rate,
            score: r.score,
            last_reply_at: chrono::DateTime::<chrono::Utc>::from_timestamp(r.last_reply_at, 0)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
        })
        .collect();

    let next_commands = targets
        .iter()
        .take(3)
        .map(|t| format!("xmaster engage recommend --topic '@{}'  # re-engage top target", t.username))
        .collect();

    let result = HotTargetsResult {
        period_days: days,
        sort: sort.to_string(),
        targets,
        suggested_next_commands: next_commands,
    };
    output::render(format, &result, None);
    Ok(())
}

// ---------------------------------------------------------------------------
// engage feed — find fresh posts from big accounts to reply to NOW
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct FeedPost {
    pub id: String,
    pub author: String,
    #[serde(skip_serializing)]
    pub watchlist_username: Option<String>,
    #[serde(skip_serializing)]
    pub author_user_id: Option<String>,
    pub author_followers: u64,
    pub text: String,
    pub age_minutes: i64,
    pub likes: u64,
    pub replies: u64,
    pub reply_command: String,
    #[serde(skip_serializing_if = "is_zero_f32")]
    pub opportunity_score: f32,
}

fn is_zero_f32(v: &f32) -> bool { *v == 0.0 }

#[derive(Debug, Clone, Serialize)]
pub struct FeedResult {
    /// All topics that were searched in this call, in the order they were
    /// resolved (CLI positional args first, then config.niche.topics fallback).
    /// An empty vec means the call ran in watchlist-only mode (no keyword search).
    pub topics: Vec<String>,
    pub posts: Vec<FeedPost>,
    pub total_found: usize,
    pub filtered_by_followers: usize,
}

impl Tableable for FeedResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Age", "Author", "Followers", "Text", "Likes", "Reply cmd"]);
        for p in &self.posts {
            let text_preview: String = p.text.chars().take(60).collect::<String>()
                + if p.text.chars().count() > 60 { "..." } else { "" };
            table.add_row(vec![
                format!("{}m", p.age_minutes),
                format!("@{}", p.author),
                format_followers(p.author_followers),
                text_preview,
                p.likes.to_string(),
                p.reply_command.clone(),
            ]);
        }
        table
    }
}

/// Resolve the list of topics to scan. Accepts multi positional args AND
/// comma-separated values in any positional arg (split + merged). If the
/// caller passes no topics at all, falls back to `niche.topics` from config.
/// Returns an empty Vec only if both sources are empty.
fn resolve_topics(cli_topics: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();

    // Helper: split one raw entry on commas + trim + dedupe into `out`.
    let push_entry = |raw: &str, out: &mut Vec<String>, seen: &mut std::collections::HashSet<String>| {
        for piece in raw.split(',') {
            let trimmed = piece.trim();
            if trimmed.is_empty() {
                continue;
            }
            let key = trimmed.to_lowercase();
            if seen.insert(key) {
                out.push(trimmed.to_string());
            }
        }
    };

    for raw in cli_topics {
        push_entry(raw, &mut out, &mut seen);
    }

    // Fallback to config.niche.topics only when the CLI passed nothing at all.
    if out.is_empty() {
        if let Ok(cfg) = crate::config::load_config() {
            for t in cfg.niche.topic_list() {
                push_entry(&t, &mut out, &mut seen);
            }
        }
    }

    out
}

pub async fn feed(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    cli_topics: &[String],
    min_followers: u64,
    max_age_mins: u64,
    count: usize,
) -> Result<(), XmasterError> {
    let api = crate::providers::xapi::XApi::new(ctx.clone());

    // Resolve topics: CLI args → split commas → fallback to niche.topics.
    // Empty result is not fatal — watchlist-only mode still returns posts
    // from watched accounts, which is often the most useful path anyway.
    let topics = resolve_topics(cli_topics);

    // Phase 1: Check watchlist accounts first (saves API search calls).
    // This runs regardless of whether we have topics — watchlist accounts
    // are intrinsically interesting.
    //
    // Batch-hydrate missing user_ids in a single GET /2/users/by call
    // instead of looping per-user (was O(n) calls, now O(ceil(n/100))).
    let mut watchlist_tweets = Vec::new();
    if let Ok(store) = IntelStore::open() {
        if let Ok(mut watchlist) = store.list_watchlist() {
            // Identify entries missing user_id and batch-resolve them.
            let missing_uids: Vec<String> = watchlist
                .iter()
                .filter(|e| e.user_id.is_none())
                .map(|e| e.username.clone())
                .collect();
            if !missing_uids.is_empty() {
                if let Ok(users) = api.get_users_by_usernames(&missing_uids).await {
                    for user in &users {
                        let followers = user
                            .public_metrics
                            .as_ref()
                            .map(|m| m.followers_count as i64)
                            .unwrap_or(0);
                        // Find the matching entry and backfill
                        if let Some(entry) = watchlist.iter_mut().find(|e| {
                            e.username.to_lowercase() == user.username.to_lowercase()
                        }) {
                            let _ = store.add_watchlist(
                                &entry.username,
                                Some(&user.id),
                                entry.topic.as_deref(),
                                followers,
                            );
                            entry.user_id = Some(user.id.clone());
                            entry.followers = followers;
                        }
                    }
                }
            }

            for entry in &watchlist {
                if let Some(ref uid) = entry.user_id {
                    let start_time = {
                        let since = chrono::Utc::now() - chrono::Duration::minutes(max_age_mins as i64);
                        since.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                    };
                    if let Ok(tweets) = api.get_user_tweets_paginated(uid, 5, Some(&start_time), None).await {
                        for mut t in tweets {
                            // Inject known follower count from watchlist (avoids missing data)
                            if t.author_followers.is_none() {
                                t.author_followers = Some(entry.followers as u64);
                            }
                            if t.author_username.is_none() {
                                t.author_username = Some(entry.username.clone());
                            }
                            watchlist_tweets.push(t);
                        }
                    }
                }
            }
        }
    }

    // Phase 2: Cold search across ALL resolved topics in parallel.
    // Each topic becomes one search call; results are unioned + deduped.
    // This replaces the previous single-topic path — the agent never has to
    // loop per-topic anymore.
    let start_time = {
        let now = chrono::Utc::now();
        let since = now - chrono::Duration::minutes(max_age_mins as i64);
        since.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    };

    let search_tweets = if watchlist_tweets.len() < count && !topics.is_empty() {
        // Budget per topic: split `count*5` evenly, min 10 per topic so even
        // a 5-topic fanout with count=10 still pulls 10/topic not 10/total.
        let per_topic_cap = (count * 5 / topics.len()).clamp(10, 100);
        let mut collected = Vec::new();
        for topic in &topics {
            let tweets = api
                .search_tweets_paginated(
                    topic,
                    "recent",
                    per_topic_cap,
                    Some(&start_time),
                    None,
                )
                .await
                .unwrap_or_default();
            collected.extend(tweets);
        }
        collected
    } else {
        Vec::new()
    };

    // Combine: watchlist first, then search results, dedupe by tweet id.
    let mut seen_ids = std::collections::HashSet::new();
    let mut tweets = Vec::new();
    for t in watchlist_tweets.into_iter().chain(search_tweets) {
        if seen_ids.insert(t.id.clone()) {
            tweets.push(t);
        }
    }

    if let Ok(store) = IntelStore::open() {
        let _ = store.record_discovered_posts("engage_feed", &tweets);
    }

    let now = chrono::Utc::now();
    let mut posts: Vec<FeedPost> = Vec::new();
    let total_found = tweets.len();
    let mut filtered_count = 0usize;

    for t in tweets {
        let author_followers = t.author_followers.unwrap_or(0);
        if author_followers < min_followers {
            filtered_count += 1;
            continue;
        }

        // Skip replies and retweets — we want original posts
        if let Some(refs) = &t.referenced_tweets {
            if refs.iter().any(|r| r.ref_type == "retweeted" || r.ref_type == "replied_to") {
                continue;
            }
        }

        let age_minutes = t.created_at.as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| (now - dt.with_timezone(&chrono::Utc)).num_minutes())
            .unwrap_or(0);

        let metrics = t.public_metrics.as_ref();
        let author_user_id = t.author_id.clone();
        let watchlist_username = t.author_username.clone();
        let author = watchlist_username
            .clone()
            .unwrap_or_else(|| author_user_id.clone().unwrap_or_default());

        posts.push(FeedPost {
            reply_command: format!("xmaster reply {} \"your reply\"", t.id),
            id: t.id,
            author: author.clone(),
            watchlist_username,
            author_user_id,
            author_followers,
            text: t.text,
            age_minutes,
            likes: metrics.map(|m| m.like_count).unwrap_or(0),
            replies: metrics.map(|m| m.reply_count).unwrap_or(0),
            opportunity_score: 0.0, // computed after collection
        });
    }

    // Score by opportunity: freshness + size_fit + conversation openness
    let my_followers = {
        let api2 = crate::providers::xapi::XApi::new(ctx.clone());
        api2.get_me().await.ok().and_then(|u| u.public_metrics.as_ref().map(|m| m.followers_count)).unwrap_or(100) as u64
    };
    for p in &mut posts {
        let freshness = 1.0 - (p.age_minutes as f64 / max_age_mins as f64).min(1.0);
        let size_fit = compute_size_fit(p.author_followers, my_followers);
        let openness = if p.likes > 0 { (p.replies as f64 / p.likes as f64).min(1.0) } else { 0.5 };
        p.opportunity_score = (0.30 * freshness + 0.30 * size_fit + 0.25 * openness + 0.15) as f32;
    }
    posts.sort_by(|a, b| b.opportunity_score.partial_cmp(&a.opportunity_score).unwrap_or(std::cmp::Ordering::Equal));
    posts.truncate(count);

    // Auto-add high-value accounts from search to watchlist (silent, never fails).
    // The topic label is the comma-joined list of all resolved topics from this
    // call, so downstream introspection still knows which fanout discovered them.
    let topic_label = topics.join(",");
    if let Ok(store) = IntelStore::open() {
        for p in &posts {
            if p.author_followers >= 10_000 {
                if let Some(username) = p.watchlist_username.as_deref() {
                    let _ = store.add_watchlist(
                        username,
                        p.author_user_id.as_deref(),
                        if topic_label.is_empty() { None } else { Some(&topic_label) },
                        p.author_followers as i64,
                    );
                }
            }
        }
    }

    let result = FeedResult {
        topics: topics.clone(),
        posts,
        total_found,
        filtered_by_followers: filtered_count,
    };

    output::render(format, &result, None);
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract @usernames from xAI search result text.
fn extract_usernames_from_text(text: &str) -> Vec<String> {
    let mut usernames = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for word in text.split_whitespace() {
        let trimmed = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '@' && c != '_');
        if let Some(name) = trimmed.strip_prefix('@') {
            let clean: String = name
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if clean.len() >= 2 && seen.insert(clean.to_lowercase()) {
                usernames.push(clean);
            }
        }
    }

    usernames
}

// ---------------------------------------------------------------------------
// Swarm: small-to-mid reply targets under a big post
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct SwarmTarget {
    pub reply_id: String,
    pub author: String,
    pub author_followers: u64,
    pub text: String,
    pub age_minutes: i64,
    pub likes: u64,
    pub replies_to_reply: u64,
    pub reply_command: String,
    pub priority_score: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SwarmResult {
    pub target_post_id: String,
    pub min_followers: u64,
    pub max_followers: u64,
    pub total_replies_scanned: usize,
    pub filtered_too_small: usize,
    pub filtered_too_big: usize,
    pub swarm_targets: Vec<SwarmTarget>,
}

impl Tableable for SwarmResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Age", "Author", "Followers", "Text", "Likes", "Reply cmd"]);
        for t in &self.swarm_targets {
            let text_preview: String = t.text.chars().take(60).collect::<String>()
                + if t.text.chars().count() > 60 { "..." } else { "" };
            table.add_row(vec![
                format!("{}m", t.age_minutes),
                format!("@{}", t.author),
                format_followers(t.author_followers),
                text_preview,
                t.likes.to_string(),
                t.reply_command.clone(),
            ]);
        }
        table
    }
}

/// Find small-to-mid accounts replying under a big post. This is the 2026
/// peer-to-peer growth layer — the big account's post is the gathering point,
/// but the reply-to-reply chain under it is where small accounts build
/// relationships with each other and grow together.
pub async fn swarm(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    id: &str,
    min_followers: u64,
    max_followers: u64,
    count: usize,
) -> Result<(), XmasterError> {
    let api = crate::providers::xapi::XApi::new(ctx.clone());
    let target_id = crate::cli::parse_tweet_id(id);

    // Fetch as many replies as we can (API max is 100 per call).
    let fetch_count = (count * 5).clamp(50, 100);
    let replies = api.get_replies(&target_id, fetch_count).await?;
    let total_scanned = replies.len();

    let now = chrono::Utc::now();
    let mut filtered_too_small = 0usize;
    let mut filtered_too_big = 0usize;
    let mut targets: Vec<SwarmTarget> = Vec::new();

    for t in replies {
        let followers = t.author_followers.unwrap_or(0);
        if followers < min_followers {
            filtered_too_small += 1;
            continue;
        }
        if followers > max_followers {
            filtered_too_big += 1;
            continue;
        }

        let age_minutes = t
            .created_at
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| (now - dt.with_timezone(&chrono::Utc)).num_minutes())
            .unwrap_or(0);

        let metrics = t.public_metrics.as_ref();
        let likes = metrics.map(|m| m.like_count).unwrap_or(0);
        let replies_to_reply = metrics.map(|m| m.reply_count).unwrap_or(0);
        let author = t.author_username.clone().unwrap_or_else(|| "unknown".into());

        // Priority: fresher replies + thread-starters (already gathering a
        // conversation) rank higher. Tiny accounts with engaged replies score
        // higher than accounts at the follower-band ceiling.
        let freshness = 1.0 - (age_minutes as f64 / 180.0).clamp(0.0, 1.0); // 3h window
        let conversation_heat = (replies_to_reply as f64 / 5.0).min(1.0);
        let small_bias = 1.0 - (followers as f64 / max_followers.max(1) as f64).clamp(0.0, 1.0);
        let priority_score = (0.4 * freshness + 0.35 * conversation_heat + 0.25 * small_bias) as f32;

        targets.push(SwarmTarget {
            reply_command: format!("xmaster reply {} \"your reply\"", t.id),
            reply_id: t.id,
            author,
            author_followers: followers,
            text: t.text,
            age_minutes,
            likes,
            replies_to_reply,
            priority_score,
        });
    }

    targets.sort_by(|a, b| {
        b.priority_score
            .partial_cmp(&a.priority_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    targets.truncate(count);

    // Silently log these as discovered targets so `inspire` and the hot-target
    // scorer can use them later.
    if let Ok(store) = IntelStore::open() {
        for t in &targets {
            let _ = store.log_engagement(
                "swarm_discovered",
                Some(&t.reply_id),
                None,
                Some(&t.author),
                Some(t.author_followers as i64),
            );
        }
    }

    let result = SwarmResult {
        target_post_id: target_id,
        min_followers,
        max_followers,
        total_replies_scanned: total_scanned,
        filtered_too_small,
        filtered_too_big,
        swarm_targets: targets,
    };

    output::render(format, &result, None);
    Ok(())
}
