use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::output::{self, CsvRenderable, OutputFormat, Tableable};
use crate::providers::xapi::XApi;
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
struct FollowResult {
    action: String,
    username: String,
    success: bool,
}

impl Tableable for FollowResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Action", "Username", "Status"]);
        table.add_row(vec![
            self.action.as_str(),
            self.username.as_str(),
            if self.success { "OK" } else { "Failed" },
        ]);
        table
    }
}

impl CsvRenderable for FollowResult {
    fn csv_headers() -> Vec<&'static str> {
        vec!["action", "username", "status"]
    }

    fn csv_rows(&self) -> Vec<Vec<String>> {
        vec![vec![
            self.action.clone(),
            self.username.clone(),
            if self.success { "OK" } else { "Failed" }.to_string(),
        ]]
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

/// Print an undo hint to stderr (only in table mode).
fn undo_hint(format: OutputFormat, message: &str) {
    if format == OutputFormat::Table {
        eprintln!("{message}");
    }
}

/// Add a contextual hint when follow/unfollow fails with 403.
fn maybe_add_hint(err: XmasterError) -> XmasterError {
    if let XmasterError::AuthMissing { provider, ref message } = err {
        if message.contains("403") {
            return XmasterError::Api {
                provider,
                code: "forbidden",
                message: format!(
                    "{message}. Hint: You may already follow this user or they have blocked you"
                ),
            };
        }
    }
    err
}

pub async fn follow(ctx: Arc<AppContext>, format: OutputFormat, username: &str) -> Result<(), XmasterError> {
    let api = XApi::new(ctx.clone());
    let username = username.trim_start_matches('@');
    api.follow_username(username).await.map_err(maybe_add_hint)?;
    let display = FollowResult {
        action: "follow".into(),
        username: username.to_string(),
        success: true,
    };
    output::render(format, &display, None);
    undo_hint(format, &format!("Undo: xmaster unfollow {username}"));
    Ok(())
}

pub async fn unfollow(ctx: Arc<AppContext>, format: OutputFormat, username: &str) -> Result<(), XmasterError> {
    let api = XApi::new(ctx.clone());
    let username = username.trim_start_matches('@');
    api.unfollow_username(username).await.map_err(maybe_add_hint)?;
    let display = FollowResult {
        action: "unfollow".into(),
        username: username.to_string(),
        success: true,
    };
    output::render(format, &display, None);
    undo_hint(format, &format!("Undo: xmaster follow {username}"));
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
