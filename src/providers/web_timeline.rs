//! Parse x.com GraphQL timelines (search, threads, profiles, home, mentions)
//! into the API's `TweetData` shape, so commands can't tell which path
//! served them.
//!
//! Every timeline is `instructions[] → entries[] → content`, where content is
//! a single item, a module of items (conversation threads), or a cursor.

use crate::providers::xapi::{
    ReferencedTweet, TweetData, TweetLookup, TweetLookupPublicMetrics, TweetMetrics, UserMetrics, UserResponse,
};
use serde_json::Value;

/// One page of a timeline: its posts in display order and the cursor for the
/// next (older) page.
#[derive(Debug, Default)]
pub struct Page {
    pub tweets: Vec<TweetData>,
    pub bottom: Option<String>,
}

/// The first `instructions` array anywhere under `root` (each operation nests
/// its timeline at a different path).
fn find_instructions(root: &Value) -> Option<&Vec<Value>> {
    match root {
        Value::Object(map) => {
            if let Some(Value::Array(a)) = map.get("instructions") {
                return Some(a);
            }
            map.values().find_map(find_instructions)
        }
        Value::Array(a) => a.iter().find_map(find_instructions),
        _ => None,
    }
}

/// Parse a timeline response. Promoted posts are skipped.
pub fn parse_timeline(root: &Value) -> Page {
    let mut page = Page::default();
    let Some(instructions) = find_instructions(root) else { return page };
    for ins in instructions {
        let entries: Vec<&Value> = match ins.get("entries") {
            Some(Value::Array(a)) => a.iter().collect(),
            // Later pages swap the cursor entries in place; pinned posts are
            // left out so the order stays newest-first like the API's.
            _ if ins.get("type").and_then(Value::as_str) == Some("TimelineReplaceEntry") => {
                ins.get("entry").into_iter().collect()
            }
            _ => continue,
        };
        for entry in entries {
            if entry.get("entryId").and_then(Value::as_str).is_some_and(|id| id.starts_with("promoted")) {
                continue;
            }
            let Some(content) = entry.get("content") else { continue };
            if let Some(c) = bottom_cursor(content).or_else(|| content.get("itemContent").and_then(bottom_cursor)) {
                page.bottom = Some(c);
                continue;
            }
            let items: Vec<&Value> = match content.get("items") {
                Some(Value::Array(a)) => a.iter().filter_map(|i| i.pointer("/item/itemContent")).collect(),
                _ => content.get("itemContent").into_iter().collect(),
            };
            for item in items {
                if item.get("promotedMetadata").is_some() {
                    continue;
                }
                if let Some(t) = item.pointer("/tweet_results/result").and_then(parse_tweet) {
                    page.tweets.push(t);
                }
            }
        }
    }
    page
}

fn bottom_cursor(v: &Value) -> Option<String> {
    (v.get("cursorType").and_then(Value::as_str) == Some("Bottom"))
        .then(|| v.get("value").and_then(Value::as_str).map(str::to_string))
        .flatten()
}

fn str_at<'a>(v: &'a Value, path: &str) -> Option<&'a str> {
    v.pointer(path).and_then(Value::as_str)
}

fn u64_at(v: &Value, path: &str) -> u64 {
    v.pointer(path)
        .and_then(|x| x.as_u64().or_else(|| x.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(0)
}

/// x.com escapes `&`, `<` and `>` in `full_text`; the note text is raw.
fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// `Wed Oct 10 20:19:24 +0000 2018` → `2018-10-10T20:19:24.000Z` (the API's format).
fn iso_date(legacy: &str) -> Option<String> {
    chrono::DateTime::parse_from_str(legacy, "%a %b %d %H:%M:%S %z %Y")
        .ok()
        .map(|d| d.to_utc().to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

/// A `tweet_results.result` → `TweetData`. `None` for tombstones and
/// unavailable posts.
pub fn parse_tweet(result: &Value) -> Option<TweetData> {
    // Visibility-limited posts nest the post one level down
    let r = match result.get("__typename").and_then(Value::as_str) {
        Some("TweetWithVisibilityResults") => result.get("tweet")?,
        _ => result,
    };
    let legacy = r.get("legacy")?;
    let id = r.get("rest_id").and_then(Value::as_str).or_else(|| str_at(legacy, "/id_str"))?;
    let user = r.pointer("/core/user_results/result");
    let username = user
        .and_then(|u| str_at(u, "/core/screen_name").or_else(|| str_at(u, "/legacy/screen_name")))
        .map(str::to_string);

    let mut refs = Vec::new();
    if let Some(rid) = str_at(legacy, "/in_reply_to_status_id_str") {
        refs.push(ReferencedTweet { ref_type: "replied_to".into(), id: rid.into() });
    }
    if let Some(qid) = str_at(legacy, "/quoted_status_id_str") {
        refs.push(ReferencedTweet { ref_type: "quoted".into(), id: qid.into() });
    }

    let retweeted = legacy.pointer("/retweeted_status_result/result").and_then(parse_tweet);
    let text = match &retweeted {
        Some(rt) => format!("RT @{}: {}", rt.author_username.as_deref().unwrap_or(""), rt.text),
        None => str_at(r, "/note_tweet/note_tweet_results/result/text")
            .map(str::to_string)
            .unwrap_or_else(|| unescape(str_at(legacy, "/full_text").unwrap_or_default())),
    };
    if let Some(rt) = &retweeted {
        refs.push(ReferencedTweet { ref_type: "retweeted".into(), id: rt.id.clone() });
    }

    let media_urls = legacy
        .pointer("/extended_entities/media")
        .or_else(|| legacy.pointer("/entities/media"))
        .and_then(Value::as_array)
        .map(|m| m.iter().filter_map(|x| str_at(x, "/media_url_https").map(str::to_string)).collect())
        .unwrap_or_default();

    Some(TweetData {
        id: id.to_string(),
        text,
        author_id: str_at(legacy, "/user_id_str")
            .or_else(|| user.and_then(|u| str_at(u, "/rest_id")))
            .map(str::to_string),
        author_username: username,
        created_at: str_at(legacy, "/created_at").and_then(iso_date),
        conversation_id: str_at(legacy, "/conversation_id_str").map(str::to_string),
        referenced_tweets: (!refs.is_empty()).then_some(refs),
        public_metrics: Some(TweetMetrics {
            like_count: u64_at(legacy, "/favorite_count"),
            retweet_count: u64_at(legacy, "/retweet_count"),
            reply_count: u64_at(legacy, "/reply_count"),
            impression_count: u64_at(r, "/views/count"),
            bookmark_count: u64_at(legacy, "/bookmark_count"),
        }),
        author_followers: user
            .and_then(|u| u.pointer("/relationship_counts/followers").or_else(|| u.pointer("/legacy/followers_count")))
            .and_then(Value::as_u64),
        media_urls,
    })
}

/// A `tweet_results.result` → the batch-lookup shape plus the author's ID.
pub fn parse_lookup(result: &Value) -> Option<(TweetLookup, String)> {
    let t = parse_tweet(result)?;
    let r = result.get("tweet").unwrap_or(result);
    let m = t.public_metrics?;
    let lookup = TweetLookup {
        id: t.id,
        created_at: t.created_at,
        public_metrics: Some(TweetLookupPublicMetrics {
            like_count: m.like_count,
            retweet_count: m.retweet_count,
            reply_count: m.reply_count,
            impression_count: m.impression_count,
            quote_count: u64_at(r, "/legacy/quote_count"),
            bookmark_count: m.bookmark_count,
        }),
        non_public_metrics: None,
    };
    Some((lookup, t.author_id?))
}

/// A `user_results.result` (or `data.user.result`) → the API's user shape.
/// X has been moving fields out of `legacy` (name, handle and avatar to
/// `core`/`avatar`, counts to `relationship_counts`/`tweet_counts`, bio to
/// `profile_bio`); each field reads the new place first, then `legacy`.
pub fn parse_user(r: &Value) -> Option<UserResponse> {
    let first = |paths: &[&str]| paths.iter().find_map(|p| str_at(r, p)).map(str::to_string);
    let count = |paths: &[&str]| paths.iter().find(|p| r.pointer(p).is_some()).map_or(0, |p| u64_at(r, p));
    Some(UserResponse {
        id: str_at(r, "/rest_id")?.to_string(),
        name: first(&["/core/name", "/legacy/name"]).unwrap_or_default(),
        username: first(&["/core/screen_name", "/legacy/screen_name"])?,
        description: first(&["/profile_bio/description", "/legacy/description"]),
        public_metrics: Some(UserMetrics {
            followers_count: count(&["/relationship_counts/followers", "/legacy/followers_count"]),
            following_count: count(&["/relationship_counts/following", "/legacy/friends_count"]),
            tweet_count: count(&["/tweet_counts/tweets", "/legacy/statuses_count"]),
            listed_count: count(&["/legacy/listed_count"]),
        }),
        profile_image_url: first(&["/avatar/image_url", "/legacy/profile_image_url_https"]),
        verified: r
            .get("is_blue_verified")
            .or_else(|| r.pointer("/verification/verified"))
            .and_then(Value::as_bool),
        created_at: first(&["/core/created_at", "/legacy/created_at"]).as_deref().and_then(iso_date),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tweet(id: &str, user: &str, text: &str) -> Value {
        json!({
            "__typename": "Tweet",
            "rest_id": id,
            "core": { "user_results": { "result": {
                "rest_id": "100", "core": { "screen_name": user }, "legacy": { "followers_count": 42 }
            }}},
            "views": { "count": "1234", "state": "EnabledWithCount" },
            "legacy": {
                "full_text": text, "created_at": "Wed Oct 10 20:19:24 +0000 2018",
                "conversation_id_str": "1", "user_id_str": "100",
                "favorite_count": 5, "retweet_count": 2, "reply_count": 1, "bookmark_count": 3,
            }
        })
    }

    fn item(entry_id: &str, result: Value) -> Value {
        json!({ "entryId": entry_id, "content": {
            "entryType": "TimelineTimelineItem",
            "itemContent": { "itemType": "TimelineTweet", "tweet_results": { "result": result } }
        }})
    }

    #[test]
    fn parses_search_page() {
        let mut promoted = item("promoted-tweet-9", tweet("9", "ad", "buy"));
        promoted["entryId"] = json!("promoted-tweet-9");
        let resp = json!({ "data": { "search_by_raw_query": { "search_timeline": { "timeline": { "instructions": [
            { "type": "TimelineAddEntries", "entries": [
                item("tweet-2", tweet("2", "alice", "a &amp; b")),
                promoted,
                { "entryId": "cursor-top-1", "content": { "entryType": "TimelineTimelineCursor", "cursorType": "Top", "value": "T" }},
                { "entryId": "cursor-bottom-1", "content": { "entryType": "TimelineTimelineCursor", "cursorType": "Bottom", "value": "B1" }},
            ]}
        ]}}}}});
        let page = parse_timeline(&resp);
        assert_eq!(page.bottom.as_deref(), Some("B1"));
        assert_eq!(page.tweets.len(), 1);
        let t = &page.tweets[0];
        assert_eq!(t.id, "2");
        assert_eq!(t.text, "a & b");
        assert_eq!(t.author_username.as_deref(), Some("alice"));
        assert_eq!(t.author_id.as_deref(), Some("100"));
        assert_eq!(t.author_followers, Some(42));
        assert_eq!(t.created_at.as_deref(), Some("2018-10-10T20:19:24.000Z"));
        let m = t.public_metrics.as_ref().unwrap();
        assert_eq!((m.like_count, m.retweet_count, m.reply_count, m.impression_count, m.bookmark_count), (5, 2, 1, 1234, 3));
    }

    #[test]
    fn later_pages_replace_the_cursor() {
        let resp = json!({ "data": { "x": { "instructions": [
            { "type": "TimelineAddEntries", "entries": [ item("tweet-3", tweet("3", "bob", "hi")) ] },
            { "type": "TimelineReplaceEntry", "entry_id_to_replace": "cursor-bottom-0",
              "entry": { "entryId": "cursor-bottom-0", "content": { "cursorType": "Bottom", "value": "B2" }}},
        ]}}});
        let page = parse_timeline(&resp);
        assert_eq!(page.bottom.as_deref(), Some("B2"));
        assert_eq!(page.tweets[0].id, "3");
    }

    #[test]
    fn parses_conversation_modules_and_reply_refs() {
        let mut reply = tweet("11", "carol", "@alice note preview…");
        reply["legacy"]["in_reply_to_status_id_str"] = json!("10");
        reply["note_tweet"] = json!({ "note_tweet_results": { "result": { "text": "@alice the full long note" }}});
        let limited = json!({ "__typename": "TweetWithVisibilityResults", "tweet": tweet("12", "dave", "limited") });
        let resp = json!({ "data": { "threaded_conversation_with_injections_v2": { "instructions": [
            { "type": "TimelineAddEntries", "entries": [
                item("tweet-10", tweet("10", "alice", "root")),
                { "entryId": "conversationthread-11", "content": {
                    "entryType": "TimelineTimelineModule",
                    "items": [
                        { "entryId": "a", "item": { "itemContent": { "tweet_results": { "result": reply }}}},
                        { "entryId": "b", "item": { "itemContent": { "tweet_results": { "result": limited }}}},
                        { "entryId": "c", "item": { "itemContent": { "tweet_results": { "result": { "__typename": "TweetTombstone" }}}}},
                    ]
                }},
                { "entryId": "cursor-bottom-x", "content": { "entryType": "TimelineTimelineItem",
                    "itemContent": { "itemType": "TimelineTimelineCursor", "cursorType": "Bottom", "value": "B3" }}},
            ]}
        ]}}});
        let page = parse_timeline(&resp);
        assert_eq!(page.bottom.as_deref(), Some("B3"));
        let ids: Vec<&str> = page.tweets.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["10", "11", "12"]);
        let r = &page.tweets[1];
        assert_eq!(r.text, "@alice the full long note");
        let refs = r.referenced_tweets.as_ref().unwrap();
        assert_eq!((refs[0].ref_type.as_str(), refs[0].id.as_str()), ("replied_to", "10"));
    }

    #[test]
    fn retweets_carry_the_original() {
        let mut rt = tweet("21", "bob", "RT @alice: trunc…");
        rt["legacy"]["retweeted_status_result"] = json!({ "result": tweet("20", "alice", "original text") });
        let t = parse_tweet(&rt).unwrap();
        assert_eq!(t.text, "RT @alice: original text");
        let refs = t.referenced_tweets.unwrap();
        assert_eq!((refs[0].ref_type.as_str(), refs[0].id.as_str()), ("retweeted", "20"));
    }

    #[test]
    fn parses_user_new_and_legacy_layouts() {
        // October 2026 layout: no `legacy` at all
        let u = parse_user(&json!({
            "rest_id": "12", "is_blue_verified": true, "verification": { "verified": false },
            "core": { "name": "Jack", "screen_name": "jack", "created_at": "Tue Mar 21 20:50:14 +0000 2006" },
            "avatar": { "image_url": "https://pbs.twimg.com/a.jpg" },
            "profile_bio": { "description": "bio" },
            "relationship_counts": { "followers": 5, "following": 6 },
            "tweet_counts": { "tweets": 7, "media_tweets": 1 }
        }))
        .unwrap();
        assert_eq!((u.id.as_str(), u.username.as_str(), u.name.as_str()), ("12", "jack", "Jack"));
        assert_eq!(u.description.as_deref(), Some("bio"));
        let m = u.public_metrics.unwrap();
        assert_eq!((m.followers_count, m.following_count, m.tweet_count, m.listed_count), (5, 6, 7, 0));
        assert_eq!(u.created_at.as_deref(), Some("2006-03-21T20:50:14.000Z"));
        assert_eq!(u.verified, Some(true));
        let old = parse_user(&json!({ "rest_id": "1", "legacy": {
            "screen_name": "a", "name": "A", "followers_count": 9, "listed_count": 2 } })).unwrap();
        assert_eq!(old.username, "a");
        assert_eq!(old.public_metrics.unwrap().followers_count, 9);
        assert!(parse_user(&json!({ "rest_id": "1", "legacy": {} })).is_none());
    }

    #[test]
    fn lookup_carries_quotes_and_author() {
        let mut t = tweet("40", "zed", "hi");
        t["legacy"]["quote_count"] = json!(4);
        let (l, author) = parse_lookup(&t).unwrap();
        assert_eq!((l.id.as_str(), author.as_str()), ("40", "100"));
        let m = l.public_metrics.unwrap();
        assert_eq!((m.quote_count, m.impression_count, m.like_count), (4, 1234, 5));
    }

    #[test]
    fn media_urls_from_extended_entities() {
        let mut t = tweet("30", "eve", "pic https://t.co/x");
        t["legacy"]["extended_entities"] = json!({ "media": [
            { "media_url_https": "https://pbs.twimg.com/media/a.jpg", "type": "photo" },
            { "media_url_https": "https://pbs.twimg.com/ext_tw_video_thumb/b.jpg", "type": "video" },
        ]});
        assert_eq!(parse_tweet(&t).unwrap().media_urls.len(), 2);
    }
}
