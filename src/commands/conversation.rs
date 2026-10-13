//! `xmaster conversation <id>`: a post in context — the chain of posts it
//! replies to, the post, and its top replies — in one compact result.
//!
//! With the web session this is one free TweetDetail read (the conversation
//! view x.com shows). Without it: FxTwitter (free) for the post and its
//! parents, and the replies read (`xmaster replies`' path) for replies.

use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::output::{self, OutputFormat, Tableable};
use crate::providers::web::Web;
use crate::providers::web_timeline;
use crate::providers::xapi::{TweetData, XApi};
use crate::providers::fxtwitter;
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::warn;

/// Parent posts shown above the post, nearest last.
const MAX_ANCESTORS: usize = 8;

#[derive(Serialize)]
struct Post {
    id: String,
    author: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    date: Option<String>,
    likes: u64,
    replies: u64,
    reposts: u64,
}

impl From<TweetData> for Post {
    fn from(t: TweetData) -> Self {
        let m = t.public_metrics.as_ref();
        Self {
            author: t.author_username.as_deref().map(|u| format!("@{u}")).or(t.author_id.clone()).unwrap_or_default(),
            likes: m.map_or(0, |m| m.like_count),
            replies: m.map_or(0, |m| m.reply_count),
            reposts: m.map_or(0, |m| m.retweet_count),
            id: t.id,
            text: t.text,
            date: t.created_at,
        }
    }
}

#[derive(Serialize)]
struct Conversation {
    /// The posts this one replies to, root first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    ancestors: Vec<Post>,
    post: Post,
    /// Direct replies, in X's ranking (top first).
    replies: Vec<Post>,
    /// "web" (one free read) or "fxtwitter+api".
    via: &'static str,
}

impl Tableable for Conversation {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["", "Author", "Text", "Likes", "ID"]);
        let rows = self
            .ancestors
            .iter()
            .map(|p| ("parent", p))
            .chain(std::iter::once(("POST", &self.post)))
            .chain(self.replies.iter().map(|p| ("reply", p)));
        for (role, p) in rows {
            table.add_row(vec![
                role.to_string(),
                p.author.clone(),
                crate::utils::ellipsize(&p.text, 120).into_owned(),
                p.likes.to_string(),
                p.id.clone(),
            ]);
        }
        table
    }
}

fn parent_id(t: &TweetData) -> Option<&str> {
    t.referenced_tweets
        .as_ref()?
        .iter()
        .find(|r| r.ref_type == "replied_to")
        .map(|r| r.id.as_str())
}

/// Split a conversation view into (ancestors root-first, the post, direct
/// replies in display order). `None` when the post isn't in the view.
fn split(tweets: Vec<TweetData>, id: &str, count: usize) -> Option<(Vec<TweetData>, TweetData, Vec<TweetData>)> {
    let mut replies = Vec::new();
    let mut others: HashMap<String, TweetData> = HashMap::new();
    let mut focal = None;
    for t in tweets {
        if t.id == id {
            focal = Some(t);
        } else if parent_id(&t) == Some(id) {
            replies.push(t);
        } else {
            others.insert(t.id.clone(), t);
        }
    }
    let focal = focal?;
    let mut ancestors = Vec::new();
    let mut next = parent_id(&focal).map(str::to_string);
    while let Some(pid) = next {
        let Some(parent) = others.remove(&pid) else { break };
        next = parent_id(&parent).map(str::to_string);
        ancestors.push(parent);
        if ancestors.len() == MAX_ANCESTORS {
            break;
        }
    }
    ancestors.reverse();
    replies.truncate(count);
    Some((ancestors, focal, replies))
}

/// One TweetDetail read: the conversation view, ranked like x.com.
async fn via_web(ctx: &AppContext, id: &str) -> Result<Vec<TweetData>, XmasterError> {
    let variables = json!({
        "focalTweetId": id,
        "referrer": "tweet",
        "with_rux_injections": false,
        "rankingMode": "Relevance",
        "includePromotedContent": false,
        "withCommunity": true,
        "withQuickPromoteEligibilityTweetFields": false,
        "withBirdwatchNotes": true,
        "withVoice": true,
    });
    let root = Web::new(ctx)?.query("TweetDetail", variables, &format!("https://x.com/i/status/{id}")).await?;
    Ok(web_timeline::parse_timeline(&root).tweets)
}

/// The post and its parents through FxTwitter (free), the API only for the
/// post itself when FxTwitter can't serve it; replies via `get_replies`.
async fn via_fallback(
    ctx: &Arc<AppContext>,
    id: &str,
    count: usize,
) -> Result<(Vec<TweetData>, TweetData, Vec<TweetData>), XmasterError> {
    let api = XApi::new(ctx.clone());
    let free = !ctx.config.settings.disable_fxtwitter;
    let focal = match free {
        true => fxtwitter::fetch_post(id).await.map(|(t, _)| t),
        false => None,
    };
    let focal = match focal {
        Some(t) => t,
        None => api.get_tweet(id).await?,
    };
    let mut ancestors = Vec::new();
    let mut next = parent_id(&focal).map(str::to_string);
    while let (true, Some(pid)) = (free && ancestors.len() < MAX_ANCESTORS, next) {
        let Some((parent, _)) = fxtwitter::fetch_post(&pid).await else { break };
        next = parent_id(&parent).map(str::to_string);
        ancestors.push(parent);
    }
    ancestors.reverse();
    let replies = if count == 0 { Vec::new() } else { api.get_replies(id, count).await? };
    Ok((ancestors, focal, replies))
}

pub async fn execute(ctx: Arc<AppContext>, format: OutputFormat, id: &str, count: usize) -> Result<(), XmasterError> {
    let mut web = None;
    if ctx.config.web_reads() {
        match via_web(&ctx, id).await {
            Ok(tweets) => match split(tweets, id, count) {
                Some(parts) => web = Some(parts),
                None => warn!("Post {id} missing from its web conversation view; trying FxTwitter"),
            },
            Err(e) => warn!("Web conversation read failed ({e}); trying FxTwitter"),
        }
    }
    let (via, (ancestors, focal, replies)) = match web {
        Some(parts) => ("web", parts),
        None => ("fxtwitter+api", via_fallback(&ctx, id, count).await?),
    };
    let display = Conversation {
        ancestors: ancestors.into_iter().map(Post::from).collect(),
        post: focal.into(),
        replies: replies.into_iter().map(Post::from).collect(),
        via,
    };
    let hint = display.replies.is_empty().then_some("No replies yet");
    match hint {
        Some(h) if count > 0 => output::render_hint(format, &display, h),
        _ => output::render(format, &display, None),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::xapi::ReferencedTweet;

    fn post(id: &str, parent: Option<&str>) -> TweetData {
        TweetData {
            id: id.into(),
            text: format!("post {id}"),
            author_id: None,
            author_username: Some("a".into()),
            created_at: None,
            conversation_id: None,
            referenced_tweets: parent.map(|p| vec![ReferencedTweet { ref_type: "replied_to".into(), id: p.into() }]),
            public_metrics: None,
            author_followers: None,
            media_urls: vec![],
        }
    }

    #[test]
    fn splits_a_conversation_view() {
        // Root 1 <- 2 <- focal 3 <- replies 4, 5; 6 replies to 4 (nested, dropped).
        let view = vec![
            post("1", None),
            post("2", Some("1")),
            post("3", Some("2")),
            post("4", Some("3")),
            post("6", Some("4")),
            post("5", Some("3")),
        ];
        let (anc, focal, replies) = split(view, "3", 10).unwrap();
        assert_eq!(anc.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["1", "2"]);
        assert_eq!(focal.id, "3");
        assert_eq!(replies.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["4", "5"]);
    }

    #[test]
    fn caps_replies_and_reports_missing_focal() {
        let view = vec![post("3", None), post("4", Some("3")), post("5", Some("3"))];
        let (_, _, replies) = split(view, "3", 1).unwrap();
        assert_eq!(replies.len(), 1);
        assert!(split(vec![post("4", Some("3"))], "3", 5).is_none());
    }
}
