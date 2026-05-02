use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::output::{self, OutputFormat, Tableable};
use crate::providers::xapi::XApi;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Serialize)]
struct AmplifierRow {
    username: String,
    repost_count: usize,
    followers: u64,
    latest_repost_id: String,
}

#[derive(Serialize)]
struct AmplifiersResult {
    total_reposts: usize,
    unique_amplifiers: usize,
    amplifiers: Vec<AmplifierRow>,
    suggested_next_commands: Vec<String>,
}

impl Tableable for AmplifiersResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["@Username", "Reposts", "Followers", "Latest Repost"]);
        for a in &self.amplifiers {
            table.add_row(vec![
                format!("@{}", a.username),
                a.repost_count.to_string(),
                a.followers.to_string(),
                a.latest_repost_id.clone(),
            ]);
        }
        table
    }
}

/// Show who amplifies your content — users who repost your tweets.
/// Groups by author, ranked by repost count descending.
pub async fn execute(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    count: usize,
) -> Result<(), XmasterError> {
    let api = XApi::new(ctx.clone());
    let tweets = api.get_reposts_of_me(count).await?;

    if tweets.is_empty() {
        return Err(XmasterError::NotFound(
            "No reposts of your content found. Post more and check back later.".into(),
        ));
    }

    // Group by author username, count reposts per author
    let mut by_author: HashMap<String, (usize, u64, String)> = HashMap::new();
    for t in &tweets {
        let username = t
            .author_username
            .clone()
            .unwrap_or_else(|| t.author_id.clone().unwrap_or_default());
        let followers = t.author_followers.unwrap_or(0);
        let entry = by_author.entry(username.to_lowercase()).or_insert((0, followers, String::new()));
        entry.0 += 1;
        if entry.1 < followers {
            entry.1 = followers;
        }
        if entry.2.is_empty() {
            entry.2 = t.id.clone();
        }
    }

    let mut amplifiers: Vec<AmplifierRow> = by_author
        .into_iter()
        .map(|(username, (repost_count, followers, latest_id))| AmplifierRow {
            username,
            repost_count,
            followers,
            latest_repost_id: latest_id,
        })
        .collect();
    amplifiers.sort_by_key(|a| std::cmp::Reverse(a.repost_count));

    let next_cmds: Vec<String> = amplifiers
        .iter()
        .take(3)
        .map(|a| format!("xmaster engage watchlist add {}  # track top amplifier", a.username))
        .collect();

    let result = AmplifiersResult {
        total_reposts: tweets.len(),
        unique_amplifiers: amplifiers.len(),
        amplifiers,
        suggested_next_commands: next_cmds,
    };
    output::render(format, &result, None);
    Ok(())
}
