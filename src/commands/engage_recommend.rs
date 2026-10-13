use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::intel::store::IntelStore;
use crate::intel::{judge, preflight};
use crate::output::{self, OutputFormat, Tableable};
use crate::providers::jev::{self, Jev};
use crate::providers::xai::XaiSearch;
use crate::providers::xapi::TweetData;
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
        for cand in candidates.values_mut() {
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
    let my_followers = preflight::own_follower_count().unwrap_or(100);
    let facts = LocalFacts::load();

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

            // Reciprocity first: the reply ranker under a post weighs the OP
            // following you, liking your reply and replying to it. (No
            // small-account penalty: Grox's follower buckets only label
            // metrics, they are not scrutiny tiers.)
            let follows_you = facts.followers.contains(&c.username.to_lowercase());
            let score = 0.30 * reciprocity
                + 0.20 * size_fit
                + 0.15 * reach
                + 0.20 * relevance
                + 0.15 * if follows_you { 1.0 } else { 0.0 };

            let mut reasons = Vec::new();
            if follows_you { reasons.push("follows you: they see your posts, and your reply ranks higher under theirs".into()); }
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
    /// Reply-target score (0-1): fresh, early, traction, reply-back odds,
    /// follows you, on topic.
    #[serde(skip_serializing_if = "is_zero")]
    pub opportunity_score: f64,
    /// JEV pick score (0-1) when JEV ranked the feed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jev_score: Option<f64>,
    /// Why it was picked: JEV's judgment, then the reply-target signals.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub why: String,
    /// Mentions one of the feed's topics (keyword overlap).
    #[serde(skip)]
    pub on_topic: bool,
    /// Which search found it (topics in order, then watchlist chunks).
    #[serde(skip)]
    pub source: usize,
}

fn is_zero(v: &f64) -> bool { *v == 0.0 }

/// How JEV ranked the feed: posts judged, and how many were dropped as
/// off-niche, promo, nothing to add, or unjudged.
#[derive(Debug, Clone, Serialize)]
pub struct FeedJev {
    pub model: &'static str,
    pub judged: usize,
    pub dropped: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeedResult {
    /// All topics that were searched in this call, in the order they were
    /// resolved (CLI positional args first, then config.niche.topics fallback).
    /// An empty vec means the call ran in watchlist-only mode (no keyword search).
    pub topics: Vec<String>,
    pub posts: Vec<FeedPost>,
    pub total_found: usize,
    pub filtered_by_followers: usize,
    /// Post-age window used: the preferred `--max-age-mins`, widened in ×2
    /// steps (up to 6 h) when it held too few posts.
    pub window_mins: i64,
    /// Searches that failed (usually X's search rate limit); the feed is
    /// built from the rest. Retry later for a fuller list.
    #[serde(skip_serializing_if = "is_zero_usize")]
    pub failed_searches: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jev: Option<FeedJev>,
}

fn is_zero_usize(v: &usize) -> bool { *v == 0 }

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

/// The feed's preferred window widens in ×2 steps up to this when thin
/// (niche topics can have only a handful of posts in 8 h).
const FEED_WINDOW_CAP_MINS: i64 = 480;
/// Topic and watchlist searches in flight at once.
const FEED_PARALLEL: usize = 4;
/// Searches per `engage feed` call, so one call can't eat X's web search
/// limit (~50 per 15 min): topics first, then up to 2 watchlist chunks.
const FEED_MAX_SEARCHES: usize = 8;
/// Like floor for topic searches: drops zero-traction spam, keeps fresh posts.
const FEED_MIN_FAVES: u32 = 2;
/// X caps a search query at 512 chars; watchlist handles are chunked under this.
const FROM_QUERY_MAX: usize = 450;
/// Most posts kept per author before judging.
const PER_AUTHOR: usize = 2;
/// Every feed search: English originals, no replies or reposts (web
/// search operators, then the API v2 spelling).
const FEED_FILTERS_WEB: &str = "lang:en -filter:replies -filter:nativeretweets";
const FEED_FILTERS_API: &str = "lang:en -is:reply -is:retweet";

fn feed_filters(web: bool) -> &'static str {
    if web { FEED_FILTERS_WEB } else { FEED_FILTERS_API }
}

/// What the local DB knows about you, read once per feed (free).
#[derive(Default)]
struct LocalFacts {
    /// Lowercase handles that follow you, from the last `track followers`.
    followers: std::collections::HashSet<String>,
    /// Lowercase handle → (replies you sent them, times they replied back).
    reply_stats: HashMap<String, (i64, i64)>,
    /// Posts you already replied to, and your own posts.
    handled: std::collections::HashSet<String>,
}

impl LocalFacts {
    fn load() -> Self {
        let mut facts = Self::default();
        let Ok(conn) = crate::intel::open_db("xmaster.db") else { return facts };
        let strings = |sql: &str| -> Vec<String> {
            conn.prepare(sql)
                .and_then(|mut s| s.query_map([], |r| r.get::<_, String>(0))?.collect())
                .unwrap_or_default()
        };
        facts.followers = strings(
            "SELECT lower(username) FROM follower_list
             WHERE snapshot_at = (SELECT MAX(snapshot_at) FROM follower_list)",
        )
        .into_iter()
        .collect();
        facts.handled = strings(
            "SELECT target_tweet_id FROM engagement_actions
             WHERE action_type = 'reply' AND target_tweet_id IS NOT NULL
             UNION SELECT tweet_id FROM posts",
        )
        .into_iter()
        .collect();
        facts.reply_stats = conn
            .prepare(
                "SELECT lower(target_username), COUNT(*), SUM(CASE WHEN got_reply_back = 1 THEN 1 ELSE 0 END)
                 FROM engagement_actions
                 WHERE action_type = 'reply' AND target_username IS NOT NULL GROUP BY 1",
            )
            .and_then(|mut s| {
                s.query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))))?
                    .collect()
            })
            .unwrap_or_default();
        facts
    }
}

/// Search for one topic: all its words (exact phrases are too rare in niche
/// topics), plus the aging/ageing spelling; English originals, with a like
/// floor on the web. A topic that already uses search operators passes
/// through.
fn topic_query(topic: &str, web: bool) -> String {
    let t = topic.trim();
    let core = if t.contains('"') || t.contains(" OR ") || t.contains(':') || t.contains('(') {
        format!("({t})")
    } else {
        let lower = t.to_lowercase();
        let mut forms = vec![lower.clone()];
        if lower.contains("ageing") {
            forms.push(lower.replace("ageing", "aging"));
        } else if lower.contains("aging") {
            forms.push(lower.replace("aging", "ageing"));
        }
        let grouped: Vec<String> =
            forms.iter().map(|f| if f.contains(' ') { format!("({f})") } else { f.clone() }).collect();
        if grouped.len() == 1 { grouped[0].clone() } else { format!("({})", grouped.join(" OR ")) }
    };
    let filters = feed_filters(web);
    if web { format!("{core} {filters} min_faves:{FEED_MIN_FAVES}") } else { format!("{core} {filters}") }
}

/// Watchlist handles as `(from:a OR from:b …)` searches: one call per ~20
/// accounts instead of one timeline read each.
fn watchlist_queries(handles: &[String], web: bool) -> Vec<String> {
    let mut chunks: Vec<Vec<String>> = vec![Vec::new()];
    let mut len = 0;
    for h in handles {
        let term = format!("from:{}", h.trim_start_matches('@'));
        let chunk = chunks.last_mut().expect("never empty");
        if !chunk.is_empty() && len + term.len() + 4 > FROM_QUERY_MAX {
            chunks.push(Vec::new());
            len = 0;
        }
        len += term.len() + 4;
        chunks.last_mut().expect("never empty").push(term);
    }
    chunks
        .into_iter()
        .filter(|c| !c.is_empty())
        .map(|c| format!("({}) {}", c.join(" OR "), feed_filters(web)))
        .collect()
}

/// Run searches, at most `FEED_PARALLEL` at once, results in query order.
/// With web reads on, a failed web search is skipped rather than retried on
/// the paid API. Errors only when every search failed.
async fn run_searches(
    ctx: &Arc<AppContext>,
    queries: Vec<String>,
    want: usize,
    since: &str,
) -> Result<(Vec<Vec<TweetData>>, usize), XmasterError> {
    let web = ctx.config.web_reads();
    let gate = Arc::new(tokio::sync::Semaphore::new(FEED_PARALLEL));
    let mut set = tokio::task::JoinSet::new();
    let n = queries.len();
    for (i, q) in queries.into_iter().enumerate() {
        let (ctx, gate, since) = (ctx.clone(), gate.clone(), since.to_string());
        set.spawn(async move {
            let _permit = gate.acquire_owned().await;
            let res = if web {
                match crate::providers::web::Web::new(&ctx) {
                    // No since_time: X's web search drops posts it should
                    // keep with it (13 Oct 2026). Latest is newest-first and
                    // the feed filters by age itself.
                    Ok(w) => w.search(&q, false, want, None, None).await,
                    Err(e) => Err(e),
                }
            } else {
                let api = crate::providers::xapi::XApi::new(ctx.clone());
                api.search_tweets_paginated(&q, "recent", want, Some(&since), None).await
            };
            (i, res)
        });
    }
    let mut out = vec![Vec::new(); n];
    let mut first_err = None;
    let (mut ok, mut failed) = (0, 0);
    while let Some(Ok((i, res))) = set.join_next().await {
        match res {
            Ok(tweets) => {
                tracing::debug!("engage feed search {i}: {} posts", tweets.len());
                out[i] = tweets;
                ok += 1;
            }
            Err(e) => {
                failed += 1;
                first_err.get_or_insert(e);
            }
        }
    }
    match first_err {
        Some(e) if ok == 0 => Err(e),
        _ => Ok((out, failed)),
    }
}

/// Lowercase alphanumeric tokens.
fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

/// `word` appears in `toks`: exact for short words ("ai"), else as a prefix
/// of a token, plural-insensitive ("agents" matches "agent", "agentic").
fn has_word(toks: &[String], word: &str) -> bool {
    let stem = if word.len() > 4 && word.ends_with('s') { &word[..word.len() - 1] } else { word };
    let alt = if stem.contains("ageing") { stem.replace("ageing", "aging") } else { stem.replace("aging", "ageing") };
    toks.iter().any(|t| {
        if stem.len() <= 3 { t == stem } else { t.starts_with(stem) || t.starts_with(alt.as_str()) }
    })
}

/// Whether the post is about one of `topics`: every word of a plain topic,
/// or any 4+ letter term of an operator topic.
fn mentions_topic(toks: &[String], topics: &[String]) -> bool {
    topics.iter().any(|t| {
        let lower = t.to_lowercase();
        if lower.contains(" or ") || lower.contains('"') || lower.contains(':') {
            tokens(&lower).iter().filter(|w| w.len() >= 4 && *w != "from").any(|w| has_word(toks, w))
        } else {
            lower.split_whitespace().all(|w| has_word(toks, w))
        }
    })
}

/// Link-only, mention-stuffed, hashtag-stuffed or cashtag-shill posts.
fn looks_spammy(text: &str) -> bool {
    let words: Vec<&str> = text.split_whitespace().collect();
    let count = |p: char| words.iter().filter(|w| w.starts_with(p) && w.len() > 1).count();
    preflight::residual_text_len(text) < 40
        || count('@') >= 3
        || count('#') >= 4
        || words.iter().filter(|w| w.starts_with('$') && w[1..].chars().next().is_some_and(char::is_alphabetic)).count() >= 2
}

/// Smallest window (preferred, ×2, … up to `cap`) holding `want` posts.
fn select_window(ages: &[i64], preferred: i64, cap: i64, want: usize) -> i64 {
    let mut w = preferred.clamp(1, cap);
    while w < cap && ages.iter().filter(|a| **a <= w).count() < want {
        w = (w * 2).min(cap);
    }
    w
}

/// Up to `pool` posts (sorted best first), shared round-robin between the
/// searches that found them, on-topic posts before the rest. Each search
/// contributes from its own smallest window (preferred, ×2, … `cap`)
/// holding its share. Returns the picks, best first, and the widest window.
fn balance(posts: Vec<FeedPost>, pool: usize, preferred: i64, cap: i64) -> (Vec<FeedPost>, i64) {
    let mut groups: std::collections::BTreeMap<usize, Vec<FeedPost>> = std::collections::BTreeMap::new();
    for p in posts {
        groups.entry(p.source).or_default().push(p);
    }
    let share = pool.div_ceil(groups.len().max(1));
    let mut widest = preferred.clamp(1, cap);
    let mut queues: Vec<std::collections::VecDeque<FeedPost>> = groups
        .into_values()
        .map(|g| {
            let ages: Vec<i64> = g.iter().map(|p| p.age_minutes).collect();
            let w = select_window(&ages, preferred, cap, share);
            widest = widest.max(w);
            g.into_iter().filter(|p| p.age_minutes <= w).collect()
        })
        .collect();
    let mut out = Vec::new();
    for on_topic in [true, false] {
        let mut lanes: Vec<std::collections::VecDeque<FeedPost>> = queues
            .iter_mut()
            .map(|q| {
                let (take, keep): (Vec<_>, Vec<_>) = q.drain(..).partition(|p| p.on_topic == on_topic);
                q.extend(keep);
                take.into()
            })
            .collect();
        while out.len() < pool && lanes.iter().any(|q| !q.is_empty()) {
            for q in &mut lanes {
                if out.len() < pool {
                    out.extend(q.pop_front());
                }
            }
        }
    }
    out.sort_by(|a, b| b.opportunity_score.total_cmp(&a.opportunity_score).then(a.age_minutes.cmp(&b.age_minutes)));
    (out, widest)
}

/// 1.0 for 2-30 min old (or inside the preferred window), sliding to 0.1 at
/// `cap`. Under 2 min is shaded: X labels too-fast replies
/// (fast_reply_spam_post), so the reply should wait anyway.
fn freshness(age: i64, preferred: i64, cap: i64) -> f64 {
    let peak = preferred.max(30);
    if age < 2 {
        0.8
    } else if age <= peak {
        1.0
    } else {
        1.0 - 0.9 * ((age - peak) as f64 / (cap - peak).max(1) as f64).min(1.0)
    }
}

/// Chance the author replies back, by size (GOS reply-back data: 45% under
/// 20K, 12% to 50K, 5% above), or your own record with them after 2+ replies.
fn reply_back_rate(followers: u64, record: Option<(i64, i64)>) -> f64 {
    match record {
        Some((sent, back)) if sent >= 2 => back as f64 / sent as f64,
        _ if followers < 20_000 => 0.45,
        _ if followers < 50_000 => 0.12,
        _ => 0.05,
    }
}

/// Reply-target score in 0-1 and the reasons behind it. The reply ranker
/// under a post weighs early likes per hour of age, the OP replying to or
/// liking you, and the OP following you (reply two-tower features).
fn score_target(p: &FeedPost, facts: &LocalFacts, preferred: i64, cap: i64) -> (f64, Vec<String>) {
    let handle = p.author.to_lowercase();
    let follows_you = facts.followers.contains(&handle);
    let record = facts.reply_stats.get(&handle).copied();
    let fresh = freshness(p.age_minutes, preferred, cap);
    let open = 1.0 / (1.0 + p.replies as f64 / 8.0);
    let per_hour = p.likes as f64 / (p.age_minutes.max(15) as f64 / 60.0);
    let traction = ((1.0 + per_hour).ln() / 51f64.ln()).min(1.0);
    let back = (reply_back_rate(p.author_followers, record) / 0.45).min(1.0);
    let score = 0.25 * fresh
        + 0.15 * open
        + 0.15 * traction
        + 0.15 * back
        + 0.15 * if follows_you { 1.0 } else { 0.0 }
        + 0.15 * if p.on_topic { 1.0 } else { 0.0 };

    let mut why = Vec::new();
    if follows_you {
        why.push("follows you".to_string());
    }
    if let Some((sent, back)) = record.filter(|r| r.1 > 0) {
        why.push(format!("replied back {back}/{sent}"));
    }
    if (2..=30).contains(&p.age_minutes) {
        why.push(format!("fresh ({} min)", p.age_minutes));
    }
    if p.replies <= 3 {
        why.push(format!("early ({} replies)", p.replies));
    }
    if p.author_followers < 20_000 && record.is_none() {
        why.push("peer-sized (reply-backs ~45%)".into());
    }
    (score, why)
}

pub async fn feed(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    cli_topics: &[String],
    min_followers: u64,
    max_age_mins: u64,
    count: Option<usize>,
) -> Result<(), XmasterError> {
    let mut topics = resolve_topics(cli_topics);
    let keep = count.unwrap_or(if ctx.config.jev_key().is_some() { 5 } else { 10 });
    let preferred = max_age_mins as i64;
    let cap = preferred.max(FEED_WINDOW_CAP_MINS);
    let facts = LocalFacts::load();

    // One concurrent round of searches: each topic, then the watchlist as
    // `from:` chunks. Free through the web session; on the paid API, fewer
    // posts per search.
    let watchlist: Vec<String> = IntelStore::open()
        .ok()
        .and_then(|s| s.list_watchlist().ok())
        .map(|w| w.into_iter().map(|e| e.username).collect())
        .unwrap_or_default();
    let web = ctx.config.web_reads();
    let wl_queries = watchlist_queries(&watchlist, web);
    topics.truncate(FEED_MAX_SEARCHES - wl_queries.len().min(2));
    let mut queries: Vec<String> = topics.iter().map(|t| topic_query(t, web)).collect();
    let room = FEED_MAX_SEARCHES - queries.len();
    queries.extend(wl_queries.into_iter().take(room));
    let want = if web { 20 } else { 10 };
    let since = (chrono::Utc::now() - chrono::Duration::minutes(cap))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let (results, failed_searches) = run_searches(&ctx, queries, want, &since).await?;

    // Dedupe by id; a post keeps the first search (topic before watchlist)
    // that found it.
    let mut seen = std::collections::HashSet::new();
    let (sources, tweets): (Vec<usize>, Vec<TweetData>) = results
        .into_iter()
        .enumerate()
        .flat_map(|(i, ts)| ts.into_iter().map(move |t| (i, t)))
        .filter(|(_, t)| seen.insert(t.id.clone()))
        .unzip();
    if let Ok(store) = IntelStore::open() {
        let _ = store.record_discovered_posts("engage_feed", &tweets);
    }

    let total_found = tweets.len();
    let mut filtered_count = 0usize;
    let judged = ctx.config.jev_key().is_some() && judge::owner(&ctx.config).is_some();
    let now = chrono::Utc::now();
    let mut posts: Vec<FeedPost> = Vec::new();
    for (source, t) in sources.into_iter().zip(tweets) {
        let author_followers = t.author_followers.unwrap_or(0);
        if author_followers < min_followers {
            filtered_count += 1;
            continue;
        }
        let is_reply_or_repost = t
            .referenced_tweets
            .as_ref()
            .is_some_and(|refs| refs.iter().any(|r| r.ref_type == "retweeted" || r.ref_type == "replied_to"));
        let age_minutes = t
            .created_at
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| (now - dt.with_timezone(&chrono::Utc)).num_minutes())
            .unwrap_or(0);
        if is_reply_or_repost || age_minutes > cap || facts.handled.contains(&t.id) || looks_spammy(&t.text) {
            continue;
        }
        // Without JEV, keyword overlap is the niche check (watchlist posts too).
        let on_topic = topics.is_empty() || mentions_topic(&tokens(&t.text), &topics);
        if !judged && !on_topic {
            continue;
        }
        let metrics = t.public_metrics.as_ref();
        let author = t.author_username.clone().unwrap_or_else(|| t.author_id.clone().unwrap_or_default());
        posts.push(FeedPost {
            reply_command: format!("xmaster reply {} \"your reply\"", t.id),
            id: t.id,
            author,
            watchlist_username: t.author_username,
            author_user_id: t.author_id,
            author_followers,
            text: t.text,
            age_minutes,
            likes: metrics.map(|m| m.like_count).unwrap_or(0),
            replies: metrics.map(|m| m.reply_count).unwrap_or(0),
            opportunity_score: 0.0,
            jev_score: None,
            why: String::new(),
            on_topic,
            source,
        });
    }

    // Score, cap per author, then share the pool fairly between searches so
    // a busy topic can't crowd out a niche one.
    for p in &mut posts {
        let (score, why) = score_target(p, &facts, preferred, cap);
        p.opportunity_score = (score * 100.0).round() / 100.0;
        p.why = why.join("; ");
    }
    posts.sort_by(|a, b| b.opportunity_score.total_cmp(&a.opportunity_score).then(a.age_minutes.cmp(&b.age_minutes)));
    let mut per_author: HashMap<String, usize> = HashMap::new();
    posts.retain(|p| {
        let n = per_author.entry(p.author.to_lowercase()).or_default();
        *n += 1;
        *n <= PER_AUTHOR
    });
    let pool = if judged { jev_input_cap(keep) } else { keep * 2 };
    let (mut posts, window) = balance(posts, pool, preferred, cap);

    let jev = if judged { jev_pick(&ctx, &mut posts, keep).await } else { None };
    if jev.is_none() {
        posts.retain(|p| p.on_topic);
        posts.truncate(keep);
    }

    // Grow the watchlist with on-niche accounts worth checking again.
    let topic_label = topics.join(",");
    if let Ok(store) = IntelStore::open() {
        for p in posts.iter().filter(|p| p.author_followers >= 10_000) {
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

    let result = FeedResult {
        topics,
        posts,
        total_found,
        filtered_by_followers: filtered_count,
        window_mins: window,
        failed_searches,
        jev,
    };
    output::render(format, &result, None);
    Ok(())
}

/// Posts sent to JEV: enough to find `keep` good ones, few enough that its
/// parallel requests finish inside their 1.5 s deadline (at ~28 in flight
/// most timed out, live 13 Oct).
fn jev_input_cap(keep: usize) -> usize {
    (keep * 4).clamp(12, 20).min(judge::MAX_ITEMS)
}

/// Keep the posts JEV judges worth a reply, best first, at most `keep`.
/// `None` leaves `posts` untouched: no key, no owner profile, or JEV down.
async fn jev_pick(ctx: &AppContext, posts: &mut Vec<FeedPost>, keep: usize) -> Option<FeedJev> {
    let jev = Jev::from_ctx(ctx)?;
    let owner = judge::owner(&ctx.config)?;
    posts.truncate(jev_input_cap(keep));
    let input: Vec<(&str, &str)> = posts.iter().map(|p| (p.author.as_str(), p.text.as_str())).collect();
    let picks = judge::rank_posts(&jev, &owner, &input).await?;
    let judged = picks.iter().flatten().count();
    let total = posts.len();
    let mut kept: Vec<(f64, FeedPost)> = posts
        .drain(..)
        .zip(picks)
        .filter_map(|(mut post, pick)| {
            tracing::debug!("jev pick {:?} @{}: {}", pick.as_ref().map(|p| p.score), post.author, crate::utils::safe_truncate(&post.text, 80));
            let pick = pick.filter(|p| p.score >= judge::PICK_FLOOR)?;
            post.jev_score = Some((pick.score * 100.0).round() / 100.0);
            post.why = [pick.why, std::mem::take(&mut post.why)]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("; ");
            Some((pick.score, post))
        })
        .collect();
    // Best content first; within a 0.1 band of pick score, the reply-target
    // score (fresh, early, follows you, replies back) breaks the tie.
    kept.sort_by(|a, b| {
        let band = |x: f64| (x * 10.0).round() as i64;
        band(b.0).cmp(&band(a.0)).then(b.1.opportunity_score.total_cmp(&a.1.opportunity_score))
    });
    posts.extend(kept.into_iter().take(keep).map(|(_, p)| p));
    Some(FeedJev { model: jev::MODEL, judged, dropped: total - posts.len() })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn post(id: &str) -> FeedPost {
        FeedPost {
            id: id.into(),
            author: "someone".into(),
            watchlist_username: None,
            author_user_id: None,
            author_followers: 10_000,
            text: "text".into(),
            age_minutes: 5,
            likes: 1,
            replies: 1,
            reply_command: String::new(),
            opportunity_score: 0.5,
            jev_score: None,
            why: String::new(),
            on_topic: true,
            source: 0,
        }
    }

    #[test]
    fn topic_query_quotes_phrases_and_adds_spelling() {
        assert_eq!(
            topic_query("Aging biology", true),
            "((aging biology) OR (ageing biology)) lang:en -filter:replies -filter:nativeretweets min_faves:2"
        );
        assert_eq!(topic_query("longevity", true), "longevity lang:en -filter:replies -filter:nativeretweets min_faves:2");
        // API v2 spelling, no like floor.
        assert_eq!(topic_query("gene therapy", false), "(gene therapy) lang:en -is:reply -is:retweet");
        // Operator topics pass through.
        assert!(topic_query("longevity OR senolytics", true).starts_with("(longevity OR senolytics) "));
    }

    #[test]
    fn watchlist_becomes_few_from_queries() {
        let handles: Vec<String> = (0..41).map(|i| format!("account_{i:02}")).collect();
        let qs = watchlist_queries(&handles, true);
        assert_eq!(qs.len(), 2);
        assert!(qs.iter().all(|q| q.len() <= 512), "{qs:?}");
        assert!(qs[0].starts_with("(from:account_00 OR from:account_01"));
        let all: usize = qs.iter().map(|q| q.matches("from:").count()).sum();
        assert_eq!(all, 41);
        assert!(watchlist_queries(&[], true).is_empty());
    }

    #[test]
    fn topic_match_needs_every_word() {
        let topics = vec!["AI agents".to_string(), "aging biology".to_string()];
        assert!(mentions_topic(&tokens("New AI agent benchmark"), &topics));
        assert!(mentions_topic(&tokens("Ageing biology is underfunded"), &topics));
        assert!(!mentions_topic(&tokens("AI is overhyped"), &topics));
        assert!(!mentions_topic(&tokens("Aim higher, agents of change"), &topics), "\"ai\" must match whole words");
    }

    #[test]
    fn spam_shapes_are_dropped() {
        assert!(looks_spammy("https://t.co/abc"));
        assert!(looks_spammy("gm @a @b @c, the lab results on rapamycin are in and they look strong"));
        assert!(looks_spammy("$PONS and $FET are the AI agent plays this week, the charts look strong here"));
        assert!(!looks_spammy("Rapamycin at 6mg weekly dropped IGF-1 by 12% in our 2024 cohort, data below"));
    }

    #[test]
    fn window_widens_only_when_thin() {
        let ages = [5, 12, 40, 90, 200, 300];
        assert_eq!(select_window(&ages, 30, 360, 2), 30);
        assert_eq!(select_window(&ages, 30, 360, 4), 120);
        assert_eq!(select_window(&ages, 30, 360, 50), 360);
    }

    #[test]
    fn balance_shares_the_pool_between_searches() {
        let mk = |id: &str, source: usize, age: i64, score: f64| FeedPost {
            source,
            age_minutes: age,
            opportunity_score: score,
            ..post(id)
        };
        // A busy topic (source 0) with many fresh posts, a niche one (1) with
        // one older post.
        let mut posts: Vec<FeedPost> = (0..10).map(|i| mk(&format!("a{i}"), 0, 5 + i, 0.9)).collect();
        posts.push(mk("n1", 1, 200, 0.4));
        let mut off = mk("w1", 2, 3, 0.95);
        off.on_topic = false;
        posts.push(off);
        let (picked, window) = balance(posts.clone(), 4, 30, 360);
        assert_eq!(picked.len(), 4);
        assert!(picked.iter().any(|p| p.id == "n1"), "the niche post gets a slot");
        assert!(!picked.iter().any(|p| p.id == "w1"), "off-topic waits for spare room");
        assert_eq!(window, 360, "the thin search widened to the cap");
        assert_eq!(picked[0].opportunity_score, 0.9, "best first");
        let (all, _) = balance(posts, 20, 30, 360);
        assert!(all.iter().any(|p| p.id == "w1"), "and fills it when there is");
    }

    #[test]
    fn freshness_peaks_early_and_shades_the_first_minutes() {
        assert_eq!(freshness(10, 30, 360), 1.0);
        assert!(freshness(1, 30, 360) < 1.0);
        assert!(freshness(120, 30, 360) > freshness(300, 30, 360));
        assert!((freshness(360, 30, 360) - 0.1).abs() < 1e-9);
    }

    #[test]
    fn reply_back_uses_size_prior_then_own_record() {
        assert_eq!(reply_back_rate(5_000, None), 0.45);
        assert_eq!(reply_back_rate(30_000, None), 0.12);
        assert_eq!(reply_back_rate(500_000, Some((1, 1))), 0.05);
        assert_eq!(reply_back_rate(500_000, Some((4, 2))), 0.5);
    }

    #[test]
    fn mutuals_and_reciprocators_rank_higher() {
        let p = post("1");
        let mut facts = LocalFacts::default();
        let (base, _) = score_target(&p, &facts, 30, 360);
        facts.followers.insert("someone".into());
        facts.reply_stats.insert("someone".into(), (3, 2));
        let (boosted, why) = score_target(&p, &facts, 30, 360);
        assert!(boosted > base, "{boosted} vs {base}");
        assert!(why.contains(&"follows you".to_string()));
        assert!(why.contains(&"replied back 2/3".to_string()));
    }

    #[tokio::test]
    async fn feed_keeps_heuristic_list_without_jev() {
        let mut config = crate::config::AppConfig::default();
        config.settings.disable_jev = true;
        let ctx = AppContext::new(config).unwrap();
        let mut posts = vec![post("1"), post("2")];
        assert!(jev_pick(&ctx, &mut posts, 5).await.is_none());
        let ids: Vec<&str> = posts.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["1", "2"]);
    }
}

