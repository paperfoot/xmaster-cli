use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::output::{self, CsvRenderable, OutputFormat, Tableable};
use crate::providers::xapi::XApi;
use reqwest::Method;
use reqwest_oauth1::OAuthClientProvider;
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Arc;

const BASE: &str = "https://api.x.com/2";

// ---------------------------------------------------------------------------
// OAuth helper
// ---------------------------------------------------------------------------

fn oauth_secrets(ctx: &AppContext) -> reqwest_oauth1::Secrets<'_> {
    let k = &ctx.config.keys;
    reqwest_oauth1::Secrets::new(&k.api_key, &k.api_secret)
        .token(&k.access_token, &k.access_token_secret)
}

fn require_auth(ctx: &AppContext) -> Result<(), XmasterError> {
    if !ctx.config.has_x_auth() {
        return Err(XmasterError::AuthMissing {
            provider: "x",
            message: "X API credentials not configured".into(),
        });
    }
    Ok(())
}

/// Make an OAuth-signed request and return the JSON body.
async fn signed_request(
    ctx: &AppContext,
    method: Method,
    url: &str,
    body: Option<Value>,
) -> Result<Value, XmasterError> {
    require_auth(ctx)?;

    let resp = match method {
        Method::GET => {
            ctx.client.clone().oauth1(oauth_secrets(ctx)).get(url).send().await?
        }
        Method::POST => {
            let mut b = ctx.client.clone().oauth1(oauth_secrets(ctx)).post(url);
            if let Some(ref json) = body {
                b = b
                    .header("Content-Type", "application/json")
                    .body(serde_json::to_string(json)?);
            }
            b.send().await?
        }
        Method::DELETE => {
            ctx.client.clone().oauth1(oauth_secrets(ctx)).delete(url).send().await?
        }
        _ => {
            return Err(XmasterError::Api {
                provider: "x",
                code: "unsupported_method",
                message: format!("Unsupported method: {method}"),
            });
        }
    };

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();

    if text.is_empty() {
        return Ok(Value::Null);
    }

    if !status.is_success() {
        return Err(XmasterError::Api {
            provider: "x",
            code: "api_error",
            message: format!("HTTP {status}: {}", crate::utils::safe_truncate(&text, 200)),
        });
    }

    serde_json::from_str(&text).map_err(|_| XmasterError::Api {
        provider: "x",
        code: "json_parse",
        message: format!("Failed to parse: {}", crate::utils::safe_truncate(&text, 200)),
    })
}

// ---------------------------------------------------------------------------
// Display types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ListCreated {
    id: String,
    name: String,
}

impl Tableable for ListCreated {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Field", "Value"]);
        table.add_row(vec!["List ID", &self.id]);
        table.add_row(vec!["Name", &self.name]);
        table
    }
}

impl CsvRenderable for ListCreated {}

#[derive(Serialize)]
struct ActionResult {
    action: String,
    success: bool,
    detail: String,
}

impl Tableable for ActionResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Action", "Detail", "Status"]);
        table.add_row(vec![
            self.action.as_str(),
            self.detail.as_str(),
            if self.success { "OK" } else { "Failed" },
        ]);
        table
    }
}

impl CsvRenderable for ActionResult {}

#[derive(Serialize)]
struct OwnedLists {
    lists: Vec<OwnedListRow>,
}

#[derive(Serialize)]
struct OwnedListRow {
    id: String,
    name: String,
    members: u64,
    followers: u64,
}

impl Tableable for OwnedLists {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["List ID", "Name", "Members", "Followers"]);
        for l in &self.lists {
            table.add_row(vec![
                l.id.as_str(),
                l.name.as_str(),
                &l.members.to_string(),
                &l.followers.to_string(),
            ]);
        }
        table
    }
}

impl CsvRenderable for OwnedLists {
    fn csv_headers() -> Vec<&'static str> {
        vec!["id", "name", "members", "followers"]
    }
    fn csv_rows(&self) -> Vec<Vec<String>> {
        self.lists
            .iter()
            .map(|l| {
                vec![
                    l.id.clone(),
                    l.name.clone(),
                    l.members.to_string(),
                    l.followers.to_string(),
                ]
            })
            .collect()
    }
}

#[derive(Serialize)]
struct TimelineResult {
    list_id: String,
    tweets: Vec<TweetRow>,
}

#[derive(Serialize)]
struct TweetRow {
    id: String,
    author: String,
    text: String,
}

impl Tableable for TimelineResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["ID", "Author", "Text"]);
        for t in &self.tweets {
            let truncated = if t.text.chars().count() > 80 {
                format!("{}...", crate::utils::safe_truncate(&t.text, 77))
            } else {
                t.text.clone()
            };
            table.add_row(vec![t.id.as_str(), t.author.as_str(), &truncated]);
        }
        table
    }
}

impl CsvRenderable for TimelineResult {
    fn csv_headers() -> Vec<&'static str> {
        vec!["id", "author", "text"]
    }
    fn csv_rows(&self) -> Vec<Vec<String>> {
        self.tweets
            .iter()
            .map(|t| vec![t.id.clone(), t.author.clone(), t.text.clone()])
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

pub async fn create(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    name: &str,
    description: Option<&str>,
) -> Result<(), XmasterError> {
    let mut body = json!({ "name": name });
    if let Some(desc) = description {
        body["description"] = json!(desc);
    }

    let val = signed_request(&ctx, Method::POST, &format!("{BASE}/lists"), Some(body)).await?;
    let data = val.get("data").ok_or_else(|| XmasterError::Api {
        provider: "x",
        code: "no_data",
        message: "No data in response".into(),
    })?;

    let display = ListCreated {
        id: data.get("id").and_then(|v| v.as_str()).unwrap_or("?").to_string(),
        name: data.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
    };
    output::render(format, &display, None);
    Ok(())
}

pub async fn delete(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    id: &str,
) -> Result<(), XmasterError> {
    signed_request(&ctx, Method::DELETE, &format!("{BASE}/lists/{id}"), None).await?;

    let display = ActionResult {
        action: "delete_list".into(),
        success: true,
        detail: format!("List {id}"),
    };
    output::render(format, &display, None);
    Ok(())
}

pub async fn add_member(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    list_id: &str,
    username: &str,
) -> Result<(), XmasterError> {
    let api = XApi::new(ctx.clone());
    let user = api.get_user_by_username(username).await?;

    signed_request(
        &ctx,
        Method::POST,
        &format!("{BASE}/lists/{list_id}/members"),
        Some(json!({ "user_id": user.id })),
    )
    .await?;

    let display = ActionResult {
        action: "add_member".into(),
        success: true,
        detail: format!("@{username} -> list {list_id}"),
    };
    output::render(format, &display, None);
    Ok(())
}

pub async fn remove_member(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    list_id: &str,
    username: &str,
) -> Result<(), XmasterError> {
    let api = XApi::new(ctx.clone());
    let user = api.get_user_by_username(username).await?;

    signed_request(
        &ctx,
        Method::DELETE,
        &format!("{BASE}/lists/{list_id}/members/{}", user.id),
        None,
    )
    .await?;

    let display = ActionResult {
        action: "remove_member".into(),
        success: true,
        detail: format!("@{username} from list {list_id}"),
    };
    output::render(format, &display, None);
    Ok(())
}

pub async fn timeline(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    list_id: &str,
    count: usize,
) -> Result<(), XmasterError> {
    let max = count.clamp(1, 100);
    let url = format!(
        "{BASE}/lists/{list_id}/tweets?max_results={max}&tweet.fields=created_at,author_id&expansions=author_id&user.fields=username"
    );

    let val = signed_request(&ctx, Method::GET, &url, None).await?;

    let tweets_val = val.get("data").and_then(|d| d.as_array()).cloned().unwrap_or_default();
    let includes = val.get("includes");

    let mut author_map = std::collections::HashMap::new();
    if let Some(inc) = includes {
        if let Some(users) = inc.get("users").and_then(|u| u.as_array()) {
            for u in users {
                if let (Some(id), Some(uname)) = (
                    u.get("id").and_then(|i| i.as_str()),
                    u.get("username").and_then(|n| n.as_str()),
                ) {
                    author_map.insert(id.to_string(), uname.to_string());
                }
            }
        }
    }

    let tweets: Vec<TweetRow> = tweets_val
        .iter()
        .map(|t| {
            let id = t.get("id").and_then(|i| i.as_str()).unwrap_or("?").to_string();
            let author_id = t.get("author_id").and_then(|a| a.as_str()).unwrap_or("");
            let author = author_map
                .get(author_id)
                .map(|u| format!("@{u}"))
                .unwrap_or_else(|| author_id.to_string());
            let text = t.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string();
            TweetRow { id, author, text }
        })
        .collect();

    let display = TimelineResult {
        list_id: list_id.to_string(),
        tweets,
    };
    output::render(format, &display, None);
    Ok(())
}

// ---------------------------------------------------------------------------
// List members — `xmaster lists members <list_id>`
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ListMembersResult {
    list_id: String,
    count: usize,
    members: Vec<ListMemberRow>,
}

#[derive(Serialize)]
struct ListMemberRow {
    id: String,
    username: String,
    name: String,
    followers: u64,
    verified: bool,
}

impl Tableable for ListMembersResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["@Username", "Name", "Followers", "Verified"]);
        for m in &self.members {
            table.add_row(vec![
                format!("@{}", m.username),
                m.name.clone(),
                m.followers.to_string(),
                if m.verified { "✓" } else { "—" }.to_string(),
            ]);
        }
        table
    }
}

impl CsvRenderable for ListMembersResult {
    fn csv_headers() -> Vec<&'static str> {
        vec!["id", "username", "name", "followers", "verified"]
    }
    fn csv_rows(&self) -> Vec<Vec<String>> {
        self.members
            .iter()
            .map(|m| {
                vec![
                    m.id.clone(),
                    m.username.clone(),
                    m.name.clone(),
                    m.followers.to_string(),
                    m.verified.to_string(),
                ]
            })
            .collect()
    }
}

pub async fn members(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    list_id: &str,
    count: usize,
) -> Result<(), XmasterError> {
    let api = XApi::new(ctx.clone());
    let users = api.get_list_members(list_id, count).await?;

    let rows: Vec<ListMemberRow> = users
        .into_iter()
        .map(|u| {
            let metrics = u.public_metrics.as_ref();
            ListMemberRow {
                id: u.id,
                username: u.username,
                name: u.name,
                followers: metrics.map(|m| m.followers_count).unwrap_or(0),
                verified: u.verified.unwrap_or(false),
            }
        })
        .collect();

    let display = ListMembersResult {
        list_id: list_id.to_string(),
        count: rows.len(),
        members: rows,
    };
    output::render(format, &display, None);
    Ok(())
}

pub async fn mine(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    count: usize,
) -> Result<(), XmasterError> {
    let api = XApi::new(ctx.clone());
    let uid = api.get_authenticated_user_id().await?;
    let max = count.clamp(1, 100);
    let url = format!(
        "{BASE}/users/{uid}/owned_lists?max_results={max}&list.fields=member_count,follower_count,created_at"
    );

    let val = signed_request(&ctx, Method::GET, &url, None).await?;
    let lists_val = val.get("data").and_then(|d| d.as_array()).cloned().unwrap_or_default();

    let lists: Vec<OwnedListRow> = lists_val
        .iter()
        .map(|l| OwnedListRow {
            id: l.get("id").and_then(|i| i.as_str()).unwrap_or("?").to_string(),
            name: l.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
            members: l.get("member_count").and_then(|m| m.as_u64()).unwrap_or(0),
            followers: l.get("follower_count").and_then(|f| f.as_u64()).unwrap_or(0),
        })
        .collect();

    let display = OwnedLists { lists };
    output::render(format, &display, None);
    Ok(())
}
