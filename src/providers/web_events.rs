//! Client events: the telemetry ("scribe") x.com sends when a post renders on
//! screen and when it leaves it. The rails send the same pair before every
//! like, reply, quote or repost, so the write follows a view the way it does
//! in the app (X's bot model flags engagement with no prior render event).
//!
//! Shapes come from the x.com web bundle (main.22feefda9e8e84cca.js and the
//! loader.TimelineRenderer chunk, 13 Oct 2026):
//! - module 737084 `clientEvent` form-POSTs `{debug: true, log: JSON(events),
//!   token}` to `/1.1/flow/timeline.json`. `token` is a Castle SDK token,
//!   sent only when `responsive_web_castle_client_event_enabled` is on; we
//!   can't mint one, so it's omitted (the field is optional in the client).
//!   The old `jot/client_event` path survives only in the unload-beacon list.
//! - the scribe logger's `createEventObject`: `_category_: "client_event"`,
//!   `format_version: 2`, `triggered_on` (ms), the data fields, an
//!   `event_namespace` with `client: "m5"`, per-session
//!   `client_event_sequence_start_timestamp` / `client_event_sequence_number`,
//!   and `client_app_id: "3033300"`. Events are flushed in batches ~3 s apart.
//! - the post page analytics namespace is `{page: "tweet"}`; its timeline
//!   scribes renders as `component: "stream", action: "results"` with
//!   `items: [{item_type: 0, id}]`, and time on screen as `element: "linger"`
//!   with `impression_details {visibility_start, visibility_end}` and
//!   `first_impression`.

use crate::context::AppContext;
use crate::providers::web::Web;
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::Duration;

/// Where the current web client posts client events.
pub const PATH: &str = "/i/api/1.1/flow/timeline.json";
const CLIENT_NAME: &str = "m5";
const CLIENT_APP_ID: &str = "3033300";
const ITEM_TWEET: u8 = 0;
/// A view event never holds up a write for longer than this.
const SEND_TIMEOUT: Duration = Duration::from_secs(4);

/// (sequence start ms, next number): one sequence per process, like one
/// x.com page session.
static SEQUENCE: Mutex<Option<(i64, u64)>> = Mutex::new(None);

fn next_sequence(now_ms: i64) -> (i64, u64) {
    let Ok(mut seq) = SEQUENCE.lock() else { return (now_ms, 0) };
    let (start, n) = seq.get_or_insert((now_ms, 0));
    let out = (*start, *n);
    *n += 1;
    out
}

fn event(namespace: Value, data: Value, triggered_on: i64, (seq_start, seq_n): (i64, u64)) -> Value {
    let mut ev = json!({
        "_category_": "client_event",
        "format_version": 2,
        "triggered_on": triggered_on,
    });
    if let (Some(ev), Value::Object(data)) = (ev.as_object_mut(), data) {
        ev.extend(data);
    }
    ev["event_namespace"] = namespace;
    ev["client_event_sequence_start_timestamp"] = json!(seq_start);
    ev["client_event_sequence_number"] = json!(seq_n);
    ev["client_app_id"] = json!(CLIENT_APP_ID);
    ev
}

fn tweet_item(tweet_id: &str) -> Value {
    json!({ "item_type": ITEM_TWEET, "id": tweet_id })
}

/// The post rendered on its page.
pub fn impression(tweet_id: &str, at_ms: i64, seq: (i64, u64)) -> Value {
    event(
        json!({ "page": "tweet", "component": "stream", "action": "results", "client": CLIENT_NAME }),
        json!({ "items": [tweet_item(tweet_id)] }),
        at_ms,
        seq,
    )
}

/// The post stayed on screen from `start_ms` to `end_ms`.
pub fn linger(tweet_id: &str, start_ms: i64, end_ms: i64, seq: (i64, u64)) -> Value {
    let mut item = tweet_item(tweet_id);
    item["impression_details"] = json!({ "visibility_start": start_ms, "visibility_end": end_ms });
    item["first_impression"] = json!(true);
    event(
        json!({ "page": "tweet", "component": "stream", "element": "linger", "action": "results", "client": CLIENT_NAME }),
        json!({ "items": [item] }),
        end_ms,
        seq,
    )
}

/// The form body the client posts for a batch of events.
pub fn form(events: &[Value]) -> Vec<(&'static str, String)> {
    vec![("debug", "true".into()), ("log", Value::Array(events.to_vec()).to_string())]
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Post one batch. Fire-and-forget: errors and timeouts are logged and dropped.
/// The whole send, including the session-state read, stays inside `SEND_TIMEOUT`
/// so a cold cache cannot hold the write.
async fn send(ctx: &AppContext, events: Vec<Value>, tweet_id: &str, handle: Option<&str>) {
    let Ok(web) = Web::new(ctx) else { return };
    let referer = match handle {
        Some(h) => format!("https://x.com/{h}/status/{tweet_id}"),
        None => format!("https://x.com/i/status/{tweet_id}"),
    };
    let post = async {
        // The real client attaches a Castle token when this switch is on; we can't
        // mint one, and token-less events would then stand out. Send nothing.
        if web.switch(crate::providers::web::CASTLE_CLIENT_EVENTS).await != Some(false) {
            tracing::debug!("view events skipped: Castle client-event token required or unknown");
            return;
        }
        match web.post_form(PATH, &form(&events), &referer).await {
            Ok(status) if (200..300).contains(&status) => {}
            Ok(status) => tracing::debug!("view event for {tweet_id}: HTTP {status}"),
            Err(e) => tracing::debug!("view event for {tweet_id} failed: {e}"),
        }
    };
    if tokio::time::timeout(SEND_TIMEOUT, post).await.is_err() {
        tracing::debug!("view event for {tweet_id} timed out");
    }
}

/// Tell X the post was shown on its page at `opened_ms`.
pub async fn send_impression(ctx: &AppContext, tweet_id: &str, handle: Option<&str>, opened_ms: i64) {
    let ev = impression(tweet_id, opened_ms, next_sequence(opened_ms));
    send(ctx, vec![ev], tweet_id, handle).await;
}

/// Tell X the post stayed on screen since `opened_ms`.
pub async fn send_linger(ctx: &AppContext, tweet_id: &str, handle: Option<&str>, opened_ms: i64) {
    let end = now_ms();
    let ev = linger(tweet_id, opened_ms, end.max(opened_ms), next_sequence(end));
    send(ctx, vec![ev], tweet_id, handle).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impression_matches_the_web_clients_event() {
        let ev = impression("123", 1_700_000_000_000, (1_699_999_999_000, 4));
        assert_eq!(ev["_category_"], "client_event");
        assert_eq!(ev["format_version"], 2);
        assert_eq!(ev["triggered_on"], 1_700_000_000_000i64);
        assert_eq!(
            ev["event_namespace"],
            json!({ "page": "tweet", "component": "stream", "action": "results", "client": "m5" })
        );
        assert_eq!(ev["items"], json!([{ "item_type": 0, "id": "123" }]));
        assert_eq!(ev["client_event_sequence_start_timestamp"], 1_699_999_999_000i64);
        assert_eq!(ev["client_event_sequence_number"], 4);
        assert_eq!(ev["client_app_id"], "3033300");
    }

    #[test]
    fn linger_carries_time_on_screen() {
        let ev = linger("123", 1_000, 21_000, (500, 1));
        assert_eq!(ev["event_namespace"]["element"], "linger");
        assert_eq!(ev["event_namespace"]["action"], "results");
        assert_eq!(ev["triggered_on"], 21_000);
        let item = &ev["items"][0];
        assert_eq!(item["id"], "123");
        assert_eq!(item["impression_details"], json!({ "visibility_start": 1_000, "visibility_end": 21_000 }));
        assert_eq!(item["first_impression"], true);
    }

    #[test]
    fn form_is_debug_plus_json_log() {
        let f = form(&[impression("1", 10, (10, 0)), linger("1", 10, 20, (10, 1))]);
        assert_eq!(f[0], ("debug", "true".to_string()));
        assert_eq!(f[1].0, "log");
        let log: Vec<Value> = serde_json::from_str(&f[1].1).unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[1]["client_event_sequence_number"], 1);
    }

    #[test]
    fn sequence_numbers_climb_within_a_process() {
        let (s1, n1) = next_sequence(5);
        let (s2, n2) = next_sequence(9);
        assert_eq!(s1, s2);
        assert!(n2 > n1);
    }
}
