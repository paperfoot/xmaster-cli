use crate::cli::parse_tweet_id;
use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::output::{self, OutputFormat, Tableable};
use crate::providers::xapi::{UserResponse, XApi};
use serde::Serialize;
use std::sync::Arc;

// Thin rendering layer shared between `likers` and `retweeters` — both return
// a plain list of users and we want identical output shape for agents.

#[derive(Serialize)]
struct EngagementUserRow {
    username: String,
    name: String,
    followers: u64,
    verified: bool,
}

#[derive(Serialize)]
struct EngagementUserList {
    tweet_id: String,
    action: String,
    count: usize,
    users: Vec<EngagementUserRow>,
}

impl Tableable for EngagementUserList {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["@Username", "Name", "Followers", "Verified"]);
        for u in &self.users {
            table.add_row(vec![
                format!("@{}", u.username),
                u.name.clone(),
                u.followers.to_string(),
                if u.verified { "✓" } else { "—" }.to_string(),
            ]);
        }
        table
    }
}

fn to_row(u: UserResponse) -> EngagementUserRow {
    let metrics = u.public_metrics.as_ref();
    EngagementUserRow {
        username: u.username,
        name: u.name,
        followers: metrics.map(|m| m.followers_count).unwrap_or(0),
        verified: u.verified.unwrap_or(false),
    }
}

pub async fn likers(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    id: &str,
    count: usize,
) -> Result<(), XmasterError> {
    let tweet_id = parse_tweet_id(id);
    let api = XApi::new(ctx.clone());
    let users = api.get_tweet_likers(&tweet_id, count).await?;
    let result = EngagementUserList {
        tweet_id,
        action: "liking_users".into(),
        count: users.len(),
        users: users.into_iter().map(to_row).collect(),
    };
    output::render(format, &result, None);
    Ok(())
}

pub async fn retweeters(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    id: &str,
    count: usize,
) -> Result<(), XmasterError> {
    let tweet_id = parse_tweet_id(id);
    let api = XApi::new(ctx.clone());
    let users = api.get_tweet_retweeters(&tweet_id, count).await?;
    let result = EngagementUserList {
        tweet_id,
        action: "retweeters".into(),
        count: users.len(),
        users: users.into_iter().map(to_row).collect(),
    };
    output::render(format, &result, None);
    Ok(())
}
