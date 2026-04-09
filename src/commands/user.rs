use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::output::{self, OutputFormat, Tableable};
use crate::providers::xapi::XApi;
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
struct UserInfo {
    id: String,
    username: String,
    name: String,
    description: String,
    followers: u64,
    following: u64,
    tweets: u64,
    verified: bool,
    created_at: String,
    profile_image_url: String,
}

impl Tableable for UserInfo {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Field", "Value"]);
        table.add_row(vec!["ID", &self.id]);
        table.add_row(vec!["Username", &format!("@{}", self.username)]);
        table.add_row(vec!["Name", &self.name]);
        table.add_row(vec!["Bio", &self.description]);
        table.add_row(vec!["Followers", &self.followers.to_string()]);
        table.add_row(vec!["Following", &self.following.to_string()]);
        table.add_row(vec!["Tweets", &self.tweets.to_string()]);
        table.add_row(vec!["Verified", if self.verified { "Yes" } else { "No" }]);
        table.add_row(vec!["Created", &self.created_at]);
        if !self.profile_image_url.is_empty() {
            table.add_row(vec!["Avatar", &self.profile_image_url]);
        }
        table
    }
}

fn to_user_info(u: crate::providers::xapi::UserResponse) -> UserInfo {
    let metrics = u.public_metrics.as_ref();
    UserInfo {
        id: u.id,
        username: u.username,
        name: u.name,
        description: u.description.unwrap_or_default(),
        followers: metrics.map(|m| m.followers_count).unwrap_or(0),
        following: metrics.map(|m| m.following_count).unwrap_or(0),
        tweets: metrics.map(|m| m.tweet_count).unwrap_or(0),
        verified: u.verified.unwrap_or(false),
        created_at: u.created_at.unwrap_or_default(),
        profile_image_url: u.profile_image_url.unwrap_or_default(),
    }
}

pub async fn info(ctx: Arc<AppContext>, format: OutputFormat, username: &str) -> Result<(), XmasterError> {
    let api = XApi::new(ctx.clone());
    let user = api.get_user_by_username(username).await?;
    output::render(format, &to_user_info(user), None);
    Ok(())
}

pub async fn me(ctx: Arc<AppContext>, format: OutputFormat) -> Result<(), XmasterError> {
    let api = XApi::new(ctx.clone());
    let user = api.get_me().await?;
    output::render(format, &to_user_info(user), None);
    Ok(())
}

#[derive(Serialize)]
struct BulkUsers {
    requested: usize,
    returned: usize,
    users: Vec<UserInfo>,
}

impl Tableable for BulkUsers {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["@Username", "Name", "Followers", "Tweets", "Verified"]);
        for u in &self.users {
            table.add_row(vec![
                format!("@{}", u.username),
                u.name.clone(),
                u.followers.to_string(),
                u.tweets.to_string(),
                if u.verified { "✓" } else { "—" }.to_string(),
            ]);
        }
        table
    }
}

/// Batch lookup of multiple users by username — single call for many users.
/// Empty input returns a clean error rather than a silent empty response.
pub async fn bulk(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    usernames: &[String],
) -> Result<(), XmasterError> {
    if usernames.is_empty() {
        return Err(XmasterError::Config(
            "xmaster users <u1> <u2> ... — provide at least one username".into(),
        ));
    }
    // Normalize: strip any leading @ the user typed.
    let clean: Vec<String> = usernames
        .iter()
        .map(|u| u.trim_start_matches('@').to_string())
        .filter(|u| !u.is_empty())
        .collect();

    let api = XApi::new(ctx.clone());
    let users = api.get_users_by_usernames(&clean).await?;

    let result = BulkUsers {
        requested: clean.len(),
        returned: users.len(),
        users: users.into_iter().map(to_user_info).collect(),
    };
    output::render(format, &result, None);
    Ok(())
}
