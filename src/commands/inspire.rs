use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::intel::store::IntelStore;
use crate::output::{self, CsvRenderable, OutputFormat, Tableable};
use chrono::Utc;
use serde::Serialize;
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Serialize)]
struct InspireResults {
    query: String,
    count: usize,
    library_size: i64,
    posts: Vec<InspireRow>,
}

#[derive(Serialize)]
struct InspireRow {
    id: String,
    author: String,
    text: String,
    likes: i64,
    impressions: i64,
    source: String,
    /// Days since user last posted something with trigram Jaccard >= 0.35 to
    /// this candidate. None = no near-duplicate in last 30 days. Helps avoid
    /// reposting variants of content already in viewers' impression bloom.
    #[serde(skip_serializing_if = "Option::is_none")]
    last_posted_similar_days: Option<i64>,
}

impl Tableable for InspireResults {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["ID", "Author", "Text", "Likes", "Views", "Via", "Last similar"]);
        for p in &self.posts {
            let truncated = if p.text.len() > 120 {
                let boundary = p.text.floor_char_boundary(117);
                format!("{}...", &p.text[..boundary])
            } else {
                p.text.clone()
            };
            let freshness = match p.last_posted_similar_days {
                Some(d) => format!("{}d ago", d),
                None => "—".into(),
            };
            table.add_row(vec![
                &p.id, &p.author, &truncated,
                &p.likes.to_string(), &p.impressions.to_string(), &p.source,
                &freshness,
            ]);
        }
        table
    }
}

impl CsvRenderable for InspireResults {
    fn csv_headers() -> Vec<&'static str> {
        vec!["id", "author", "text", "likes", "impressions", "source", "last_posted_similar_days"]
    }
    fn csv_rows(&self) -> Vec<Vec<String>> {
        self.posts.iter().map(|p| vec![
            p.id.clone(), p.author.clone(), p.text.clone(),
            p.likes.to_string(), p.impressions.to_string(), p.source.clone(),
            p.last_posted_similar_days.map(|d| d.to_string()).unwrap_or_default(),
        ]).collect()
    }
}

/// Trigram set of a text (lowercase alphanumeric runs only).
fn trigrams(text: &str) -> HashSet<String> {
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { ' ' })
        .collect();
    let chars: Vec<char> = cleaned.chars().collect();
    let mut set = HashSet::new();
    if chars.len() < 3 {
        return set;
    }
    for w in chars.windows(3) {
        if !w.iter().all(|c| c.is_whitespace()) {
            set.insert(w.iter().collect::<String>());
        }
    }
    set
}

fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count();
    let union = a.union(b).count();
    inter as f32 / union as f32
}

pub async fn execute(
    _ctx: Arc<AppContext>,
    format: OutputFormat,
    topic: Option<&str>,
    author: Option<&str>,
    min_likes: Option<i64>,
    min_chars: Option<i64>,
    count: usize,
) -> Result<(), XmasterError> {
    let store = IntelStore::open()
        .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;

    let library_size = store.discovered_posts_count()
        .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?;

    let rows = store.query_discovered_posts(topic, author, min_likes, min_chars, count)
        .map_err(|e| XmasterError::Config(format!("Query error: {e}")))?;

    if rows.is_empty() {
        let hint = if library_size == 0 {
            "Library is empty. Run `xmaster search`, `xmaster timeline`, or `xmaster read` to start building it."
        } else if min_chars.is_some() {
            "No long-form posts in the library yet. Run `xmaster search-ai` on accounts that publish Articles or long notes (e.g. @beaverd, @KobeissiLetter, @thedankoe) to seed the corpus."
        } else {
            "No posts match your filters. Try broader criteria or omit --min-likes."
        };
        return Err(XmasterError::NotFound(hint.into()));
    }

    // Pull my last-30-days published posts to compute trigram freshness.
    // If the user has posted something similar recently, the impression bloom
    // filter means re-posting that variant has near-zero reach to viewers who
    // already saw the original.
    let now_ts = Utc::now().timestamp();
    let thirty_days_ago = now_ts - 30 * 24 * 3600;
    let my_recent: Vec<(String, i64, HashSet<String>)> = store
        .get_post_history(200)
        .map_err(|e| XmasterError::Config(format!("DB error: {e}")))?
        .into_iter()
        .filter(|p| p.posted_at >= thirty_days_ago)
        .map(|p| (p.text.clone(), p.posted_at, trigrams(&p.text)))
        .collect();

    let display = InspireResults {
        query: topic.unwrap_or("all").to_string(),
        count: rows.len(),
        library_size,
        posts: rows.into_iter().map(|r| {
            let candidate_grams = trigrams(&r.text);
            let mut best: Option<(i64, f32)> = None;
            for (_text, posted_at, grams) in &my_recent {
                let sim = jaccard(&candidate_grams, grams);
                if sim >= 0.35 {
                    let days_ago = (now_ts - posted_at) / 86400;
                    match best {
                        None => best = Some((days_ago, sim)),
                        Some((_, prev_sim)) if sim > prev_sim => best = Some((days_ago, sim)),
                        _ => {}
                    }
                }
            }
            InspireRow {
                id: r.tweet_id,
                author: if r.author_username.is_empty() { "?".into() } else { format!("@{}", r.author_username) },
                text: r.text,
                likes: r.like_count,
                impressions: r.impression_count,
                source: r.last_source,
                last_posted_similar_days: best.map(|(d, _)| d),
            }
        }).collect(),
    };
    output::render_csv(format, &display, None);
    Ok(())
}
