use crate::commands::{reclassify_403, WriteRoute};
use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::output::{self, CsvRenderable, OutputFormat, Tableable};
use crate::providers::xapi::XApi;
use serde::Serialize;
use std::sync::Arc;

/// A follow or unfollow, with the profile link, the undo command and the
/// path that carried it.
#[derive(Serialize)]
struct FollowResult {
    action: &'static str,
    username: String,
    url: String,
    undo: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    via: Option<&'static str>,
}

impl Tableable for FollowResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Action", "Username", "Status"]);
        table.add_row(vec![self.action, &format!("@{}", self.username), "OK"]);
        table
    }
}

impl CsvRenderable for FollowResult {
    fn csv_headers() -> Vec<&'static str> {
        vec!["action", "username", "url", "undo"]
    }

    fn csv_rows(&self) -> Vec<Vec<String>> {
        vec![vec![self.action.into(), self.username.clone(), self.url.clone(), self.undo.clone()]]
    }
}

#[derive(Serialize)]
struct UserList {
    users: Vec<UserRow>,
}

#[derive(Serialize)]
struct UserRow {
    username: String,
    name: String,
    followers: u64,
    following: u64,
    tweets: u64,
    verified: bool,
}

impl Tableable for UserList {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Username", "Name", "Followers", "Following", "Tweets", "Verified"]);
        for u in &self.users {
            table.add_row(vec![
                format!("@{}", u.username),
                u.name.clone(),
                u.followers.to_string(),
                u.following.to_string(),
                u.tweets.to_string(),
                if u.verified { "Yes" } else { "No" }.to_string(),
            ]);
        }
        table
    }
}

impl CsvRenderable for UserList {
    fn csv_headers() -> Vec<&'static str> {
        vec!["username", "name", "followers", "following", "tweets", "verified"]
    }

    fn csv_rows(&self) -> Vec<Vec<String>> {
        self.users
            .iter()
            .map(|u| {
                vec![
                    format!("@{}", u.username),
                    u.name.clone(),
                    u.followers.to_string(),
                    u.following.to_string(),
                    u.tweets.to_string(),
                    if u.verified { "Yes" } else { "No" }.to_string(),
                ]
            })
            .collect()
    }
}

fn render_follow(ctx: &AppContext, format: OutputFormat, action: &'static str, undo: &str, username: &str) {
    let display = FollowResult {
        action,
        username: username.to_string(),
        url: format!("https://x.com/{username}"),
        undo: format!("xmaster {undo} {username}"),
        via: WriteRoute::web_only(ctx),
    };
    if format == OutputFormat::Table {
        eprintln!("Undo: {}", display.undo);
    }
    output::render_csv(format, &display, None);
}

pub async fn follow(ctx: Arc<AppContext>, format: OutputFormat, username: &str) -> Result<(), XmasterError> {
    XApi::new(ctx.clone()).follow_username(username).await.map_err(reclassify_403)?;
    render_follow(&ctx, format, "follow", "unfollow", username);
    Ok(())
}

pub async fn unfollow(ctx: Arc<AppContext>, format: OutputFormat, username: &str) -> Result<(), XmasterError> {
    XApi::new(ctx.clone()).unfollow_username(username).await.map_err(reclassify_403)?;
    render_follow(&ctx, format, "unfollow", "follow", username);
    Ok(())
}

pub async fn followers(ctx: Arc<AppContext>, format: OutputFormat, username: &str, count: usize) -> Result<(), XmasterError> {
    let api = XApi::new(ctx.clone());
    let user = api.get_user_by_username(username).await?;
    let users = api.get_user_followers(&user.id, count).await?;
    let display = UserList {
        users: users.into_iter().map(|u| {
            let metrics = u.public_metrics.as_ref();
            UserRow {
                username: u.username,
                name: u.name,
                followers: metrics.map(|m| m.followers_count).unwrap_or(0),
                following: metrics.map(|m| m.following_count).unwrap_or(0),
                tweets: metrics.map(|m| m.tweet_count).unwrap_or(0),
                verified: u.verified.unwrap_or(false),
            }
        }).collect(),
    };
    output::render_csv(format, &display, None);
    Ok(())
}

pub async fn following(ctx: Arc<AppContext>, format: OutputFormat, username: &str, count: usize) -> Result<(), XmasterError> {
    let api = XApi::new(ctx.clone());
    let user = api.get_user_by_username(username).await?;
    let users = api.get_user_following(&user.id, count).await?;
    let display = UserList {
        users: users.into_iter().map(|u| {
            let metrics = u.public_metrics.as_ref();
            UserRow {
                username: u.username,
                name: u.name,
                followers: metrics.map(|m| m.followers_count).unwrap_or(0),
                following: metrics.map(|m| m.following_count).unwrap_or(0),
                tweets: metrics.map(|m| m.tweet_count).unwrap_or(0),
                verified: u.verified.unwrap_or(false),
            }
        }).collect(),
    };
    output::render_csv(format, &display, None);
    Ok(())
}
