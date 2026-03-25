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

    // Phase 1c: Topic discovery via xAI search
    if let Some(topic_str) = topic {
        let xai = XaiSearch::new(ctx.clone());
        if let Ok(result) = xai.search_posts(topic_str, 20, None, None, None).await {
            // Extract usernames from citations and text
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
                    source: "topic".into(),
                    relevance: 1.0,
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
        output::render_error(
            format,
            "no_candidates",
            "No recommendation candidates found",
            "Try: `xmaster engage recommend --topic \"your niche\"` or engage with more accounts first",
        );
        return Ok(());
    }

    // Phase 2: Score
    let mut scored: Vec<RecommendCandidate> = filtered
        .into_iter()
        .map(|c| {
            let reciprocity = c.reply_rate; // 0-1
            let reach = if c.followers > 0 {
                ((c.followers as f64).log2() / 20.0).min(1.0)
            } else {
                0.0
            };
            let freshness = 1.0; // all candidates are from live/recent data
            let relevance = c.relevance;

            let score = 0.4 * reciprocity + 0.3 * reach + 0.2 * freshness + 0.1 * relevance;

            RecommendCandidate {
                rank: 0, // assigned after sort
                username: c.username,
                followers: c.followers,
                reply_rate: c.reply_rate,
                score,
                source: c.source,
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

/// Try to extract follower count from author_id context. Returns 0 if unavailable.
fn get_mention_followers(_author_id: &Option<String>) -> u64 {
    // Follower counts aren't included in mentions data by default.
    // We rely on the store's target_followers or the scoring formula handles 0.
    0
}
