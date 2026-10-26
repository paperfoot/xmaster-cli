//! TypeSafe JEV: fast typed judgments (noul / choice / score) over text.
//!
//! Advisory only. Every call runs under a short deadline and any failure
//! (no key, timeout, HTTP error, odd JSON) returns `None`, so callers keep
//! their heuristic behaviour. Only public post text and the owner's own
//! profile or draft are ever sent; never DMs.

use crate::context::AppContext;
use serde_json::{json, Map, Value};
use std::time::Duration;

const URL: &str = "https://api.typesafe.ai/v1/systemone";
/// Pinned so the eval numbers behind the prompts stay valid when the
/// `jev-latest` alias moves. jev-1.14.0 (26 Oct 2026) was worse here: inbox
/// needs-reply AUC 0.93 -> 0.90 and 7 picker flips at the floor against 2
/// from noise; re-run `~/x/xmaster-jev-eval/eval_114.py` before moving.
pub const MODEL: &str = "jev-1.13.0";
const DEADLINE: Duration = Duration::from_millis(1500);

#[derive(Clone)]
pub struct Jev {
    client: reqwest::Client,
    key: String,
    url: String,
}

impl Jev {
    /// None when no key is configured or `settings.disable_jev` is set.
    pub fn from_ctx(ctx: &AppContext) -> Option<Self> {
        ctx.config.jev_key().map(|key| Self {
            client: ctx.client.clone(),
            key,
            url: URL.into(),
        })
    }

    /// Evaluate `questions` against `state` in one request.
    pub async fn ask(&self, state: &Value, questions: &Value) -> Option<Answers> {
        let body = json!({ "model": MODEL, "state": state, "questions": questions });
        let resp = self
            .client
            .post(&self.url)
            .bearer_auth(&self.key)
            .timeout(DEADLINE)
            .json(&body)
            .send()
            .await
            .map_err(|e| tracing::debug!("jev request failed: {e}"))
            .ok()?;
        if !resp.status().is_success() {
            tracing::debug!("jev returned HTTP {}", resp.status());
            return None;
        }
        let mut v: Value = resp
            .json()
            .await
            .map_err(|e| tracing::debug!("jev body unreadable: {e}"))
            .ok()?;
        match v.get_mut("answers").map(Value::take) {
            Some(Value::Object(map)) => Some(Answers(map)),
            _ => None,
        }
    }

    /// One request per state, all in flight at once (each under the same
    /// deadline). Results keep input order; a failed request is `None`.
    pub async fn ask_all(&self, states: Vec<Value>, questions: &Value) -> Vec<Option<Answers>> {
        let mut set = tokio::task::JoinSet::new();
        let n = states.len();
        for (i, state) in states.into_iter().enumerate() {
            let (jev, questions) = (self.clone(), questions.clone());
            set.spawn(async move { (i, jev.ask(&state, &questions).await) });
        }
        let mut out: Vec<Option<Answers>> = (0..n).map(|_| None).collect();
        while let Some(joined) = set.join_next().await {
            if let Ok((i, answers)) = joined {
                out[i] = answers;
            }
        }
        out
    }
}

/// Answers keyed by question id.
#[derive(Debug, Default)]
pub struct Answers(Map<String, Value>);

#[cfg(test)]
impl From<Map<String, Value>> for Answers {
    fn from(map: Map<String, Value>) -> Self {
        Self(map)
    }
}

impl Answers {
    /// P(yes) for a Noul question.
    pub fn noul(&self, id: &str) -> Option<f64> {
        self.0.get(id)?.get("noul")?.as_f64()
    }

    /// Probability-weighted level for a Score question.
    pub fn score(&self, id: &str) -> Option<f64> {
        self.0.get(id)?.get("score")?.as_f64()
    }

    /// Probability of one option of a Choice question.
    pub fn prob(&self, id: &str, option: &str) -> Option<f64> {
        self.0.get(id)?.get("probabilities")?.get(option)?.as_f64()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// One-shot HTTP server: answers the first request with `status` and
    /// `body` after `delay`. Returns its URL.
    async fn mock(status: u16, body: &'static str, delay: Duration) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 16 * 1024];
            let _ = sock.read(&mut buf).await;
            tokio::time::sleep(delay).await;
            let resp = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        });
        format!("http://{addr}/v1/systemone")
    }

    fn jev(url: String) -> Jev {
        Jev { client: reqwest::Client::new(), key: "test".into(), url }
    }

    const OK: &str = r#"{"model":"jev-1.13.0","answers":{
        "a":{"type":"noul","noul":0.9},
        "b":{"type":"score","score":1.4,"confidence":0.4,"probabilities":{"0":0.1,"1":0.4,"2":0.5}},
        "c":{"type":"choice","choice":"reply","probabilities":{"reply":0.7,"spam":0.3},"confidence":0.5}},
        "usage":{"input_tokens":10,"output_tokens":3}}"#;

    #[tokio::test]
    async fn parses_all_answer_types() {
        let url = mock(200, OK, Duration::ZERO).await;
        let a = jev(url).ask(&json!("s"), &json!({})).await.unwrap();
        assert_eq!(a.noul("a"), Some(0.9));
        assert_eq!(a.score("b"), Some(1.4));
        assert_eq!(a.prob("c", "reply"), Some(0.7));
        assert_eq!(a.noul("b"), None, "wrong answer type is None");
        assert_eq!(a.noul("missing"), None);
    }

    #[tokio::test]
    async fn http_error_is_none() {
        let url = mock(500, r#"{"detail":"boom"}"#, Duration::ZERO).await;
        assert!(jev(url).ask(&json!("s"), &json!({})).await.is_none());
    }

    #[tokio::test]
    async fn garbage_body_is_none() {
        let url = mock(200, "not json", Duration::ZERO).await;
        assert!(jev(url).ask(&json!("s"), &json!({})).await.is_none());
    }

    #[tokio::test]
    async fn slow_server_times_out_to_none() {
        let url = mock(200, OK, Duration::from_secs(4)).await;
        let started = std::time::Instant::now();
        assert!(jev(url).ask(&json!("s"), &json!({})).await.is_none());
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn ask_all_keeps_order_and_isolates_failures() {
        let ok = mock(200, OK, Duration::ZERO).await;
        let jev = jev(ok);
        // The mock serves one connection; the second request must fail alone.
        let out = jev.ask_all(vec![json!("a"), json!("b")], &json!({})).await;
        assert_eq!(out.len(), 2);
        assert_eq!(out.iter().filter(|a| a.is_some()).count(), 1);
    }

    #[tokio::test]
    async fn unreachable_is_none() {
        assert!(jev("http://127.0.0.1:9/v1/systemone".into())
            .ask(&json!("s"), &json!({}))
            .await
            .is_none());
    }
}
