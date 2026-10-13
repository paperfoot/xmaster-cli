use serde::Serialize;
use serde_json::Value;
use std::io::IsTerminal;
use std::sync::atomic::{AtomicU8, Ordering};

/// 0 = auto (pretty on a terminal, compact when piped), 1 = pretty, 2 = compact.
static PRETTY: AtomicU8 = AtomicU8::new(0);

/// Force pretty (`true`) or compact (`false`) JSON, e.g. from a `--pretty` flag.
/// Without it, `XMASTER_PRETTY=1|0` decides, then whether the stream is a terminal.
pub fn set_pretty(on: bool) {
    PRETTY.store(if on { 1 } else { 2 }, Ordering::Relaxed);
}

fn pretty(stream_is_tty: bool) -> bool {
    match PRETTY.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => match std::env::var("XMASTER_PRETTY").as_deref() {
            Ok("1" | "true" | "yes") => true,
            Ok("0" | "false" | "no") => false,
            _ => stream_is_tty,
        },
    }
}

/// Drop `null` object members, recursively. Agents pay per token and an absent
/// key reads the same as `null` in jq and most JSON clients.
fn prune_nulls(v: &mut Value) {
    match v {
        Value::Object(map) => {
            map.retain(|_, v| !v.is_null());
            map.values_mut().for_each(prune_nulls);
        }
        Value::Array(items) => items.iter_mut().for_each(prune_nulls),
        _ => {}
    }
}

fn encode<T: Serialize>(v: &T, pretty: bool) -> String {
    let out = if pretty { serde_json::to_string_pretty(v) } else { serde_json::to_string(v) };
    out.unwrap_or_default()
}

/// `{version, status, data|error, metadata?}`, in that order: a struct keeps
/// field order, where `json!` would sort the keys.
#[derive(Serialize)]
pub struct Envelope<'a> {
    version: &'static str,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a ErrorBody<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Value>,
}

/// The success envelope; `metadata` only when it has something in it.
pub fn envelope<T: Serialize>(data: &T, metadata: Option<Value>) -> Result<Envelope<'static>, serde_json::Error> {
    let mut data = serde_json::to_value(data)?;
    prune_nulls(&mut data);
    let metadata = metadata
        .map(|mut m| {
            prune_nulls(&mut m);
            m
        })
        .filter(|m| !(m.is_null() || m.as_object().is_some_and(|o| o.is_empty())));
    Ok(Envelope { version: "1", status: "success", data: Some(data), error: None, metadata })
}

pub fn render<T: Serialize>(data: &T, metadata: Option<Value>) {
    match envelope(data, metadata) {
        Ok(env) => println!("{}", encode(&env, pretty(std::io::stdout().is_terminal()))),
        Err(e) => render_error(&ErrorBody::plain("json_error", &e.to_string(), "Report this: the result couldn't be encoded as JSON")),
    }
}

/// The `error` object of an error envelope.
#[derive(Serialize)]
pub struct ErrorBody<'a> {
    pub code: &'a str,
    pub message: &'a str,
    pub suggestion: &'a str,
    /// May the same command succeed if run again later, unchanged?
    pub retryable: bool,
    /// Seconds to wait first; only present when `retryable`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
}

impl<'a> ErrorBody<'a> {
    pub fn plain(code: &'a str, message: &'a str, suggestion: &'a str) -> Self {
        Self { code, message, suggestion, retryable: false, retry_after_secs: None }
    }
}

pub fn render_error(body: &ErrorBody) {
    let env = Envelope { version: "1", status: "error", data: None, error: Some(body), metadata: None };
    // Errors go to stderr per agent-cli-framework invariant 6:
    // `tool cmd | jq` must never see error JSON on stdout.
    eprintln!("{}", encode(&env, pretty(std::io::stderr().is_terminal())));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Serialize)]
    struct Post {
        id: &'static str,
        reply_to: Option<&'static str>,
        tags: Vec<Option<u8>>,
    }

    #[test]
    fn drops_nulls_and_empty_metadata() {
        let p = Post { id: "1", reply_to: None, tags: vec![Some(1), None] };
        let env = |m| serde_json::to_value(envelope(&p, m).unwrap()).unwrap();
        // Array positions keep their nulls; object members don't.
        assert_eq!(env(Some(json!({}))), json!({"version":"1","status":"success","data":{"id":"1","tags":[1,null]}}));
        assert!(env(Some(json!({"hint": null}))).get("metadata").is_none());
        assert!(env(None).get("metadata").is_none());
    }

    #[test]
    fn keeps_non_empty_metadata_and_empty_results() {
        let env = serde_json::to_value(envelope(&Vec::<u8>::new(), Some(json!({"hint": "nothing yet"}))).unwrap()).unwrap();
        assert_eq!(env["data"], json!([]));
        assert_eq!(env["metadata"]["hint"], "nothing yet");
    }

    #[test]
    fn compact_is_one_line() {
        let env = envelope(&json!({"a": [1, 2]}), None).unwrap();
        assert_eq!(encode(&env, false), r#"{"version":"1","status":"success","data":{"a":[1,2]}}"#);
        assert!(encode(&env, true).contains("\n  "));
    }

    #[test]
    fn error_body_omits_retry_after_unless_set() {
        let v = serde_json::to_value(ErrorBody::plain("bad_input", "m", "s")).unwrap();
        assert_eq!(v, json!({"code":"bad_input","message":"m","suggestion":"s","retryable":false}));
    }
}
