use thiserror::Error;

/// The exit-code contract. agent-info and the docs should render this table
/// rather than restate it, so the two can't drift.
#[allow(dead_code)]
pub const EXIT_CODES: [(i32, &str); 6] = [
    (0, "success"),
    (1, "runtime error (network, X server, local DB, media): retry only if error.retryable, after error.retry_after_secs"),
    (2, "setup: missing or rejected credentials, bad config, API credits exhausted: follow error.suggestion, do not retry"),
    (3, "bad input: invalid arguments, unknown post or user, denied command: fix the arguments"),
    (4, "rate limited: wait error.retry_after_secs, then retry"),
    (5, "blocked by X (duplicate text, reply limits, protected or blocking account, endpoint not on your tier): do not retry unchanged; follow error.suggestion"),
];

/// Backoff for a 429 whose reset time X didn't send.
const DEFAULT_RATE_LIMIT_SECS: u64 = 60;

#[derive(Error, Debug)]
pub enum XmasterError {
    #[error("API error from {provider}: {message}")]
    Api {
        provider: &'static str,
        code: &'static str,
        message: String,
    },

    #[error("Authentication missing: {message}")]
    AuthMissing {
        provider: &'static str,
        message: String,
    },

    #[error("Rate limited by {provider}; retry in {}s", secs_until(*reset_at))]
    RateLimited {
        provider: &'static str,
        /// Unix time the window resets; 0 when X didn't say.
        reset_at: u64,
    },

    #[error("Server error (HTTP {status}), retries exhausted")]
    ServerError { status: u16 },

    #[error("Reply restricted: {0}")]
    ReplyRestricted(String),

    /// X refused the action itself (not the credentials). Build it with
    /// [`XmasterError::from_x_error`] or [`XmasterError::removed_from_api`].
    #[error("Blocked by X: {reason}{}", x_type.as_deref().map(|t| format!(" [{t}]")).unwrap_or_default())]
    Forbidden {
        reason: String,
        /// X's error type: the v2 problem slug (`client-forbidden`) or the
        /// legacy/GraphQL code (`code 187`).
        x_type: Option<String>,
    },

    #[error("Payment required: {message}")]
    PaymentRequired { message: String },

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Local database error: {0}")]
    Db(String),

    #[error("Media error: {0}")]
    Media(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Command denied: {0}")]
    CommandDenied(String),

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error("OAuth error: {0}")]
    OAuth(String),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl From<rusqlite::Error> for XmasterError {
    fn from(e: rusqlite::Error) -> Self {
        XmasterError::Db(e.to_string())
    }
}

// TRANSITIONAL: keeps `?` on reqwest-oauth1 sends compiling (xapi.rs,
// rate_limits.rs) until lane transport drops the crate. Delete at merge.
impl From<reqwest_oauth1::Error> for XmasterError {
    fn from(e: reqwest_oauth1::Error) -> Self {
        XmasterError::OAuth(e.to_string())
    }
}

/// Seconds from now until `reset_at` (Unix time); a default when unknown.
fn secs_until(reset_at: u64) -> u64 {
    if reset_at == 0 {
        return DEFAULT_RATE_LIMIT_SECS;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    reset_at.saturating_sub(now).max(1)
}

/// What kind of refusal X sent; picks the suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    Duplicate,
    Automated,
    Locked,
    NotVisible,
    NotOnTier,
    DailyLimit,
    AlreadyDone,
    Other,
}

fn classify(x_type: Option<&str>, reason: &str) -> Refusal {
    let t = x_type.unwrap_or("");
    let r = reason.to_ascii_lowercase();
    let code = t.strip_prefix("code ").and_then(|c| c.parse::<u32>().ok()).unwrap_or(0);
    match code {
        187 => return Refusal::Duplicate,
        226 => return Refusal::Automated,
        64 | 326 => return Refusal::Locked,
        136 | 179 | 385 => return Refusal::NotVisible,
        161 | 344 => return Refusal::DailyLimit,
        139 | 160 | 327 => return Refusal::AlreadyDone,
        _ => {}
    }
    if r.contains("duplicate") {
        Refusal::Duplicate
    } else if r.contains("automated") {
        Refusal::Automated
    } else if r.contains("locked") || r.contains("suspended") {
        Refusal::Locked
    } else if r.contains("already") {
        Refusal::AlreadyDone
    } else if r.contains("daily") {
        Refusal::DailyLimit
    } else if t == "endpoint-removed"
        || t == "client-forbidden"
        || t == "unsupported-authentication"
        || t == "disallowed-resource"
    {
        Refusal::NotOnTier
    } else if t == "not-authorized-for-resource"
        || r.contains("protected")
        || r.contains("blocked")
        || r.contains("not visible")
        || r.contains("deleted")
    {
        Refusal::NotVisible
    } else {
        Refusal::Other
    }
}

/// Does this refusal mean "the author limits who can reply"?
fn is_reply_limit(code: u32, reason: &str) -> bool {
    let r = reason.to_ascii_lowercase();
    code == 433
        || r.contains("not been mentioned")
        || r.contains("reply to this conversation is not allowed")
        || r.contains("restricted who can reply")
}

impl XmasterError {
    /// Turn an X error (HTTP 401/403, or GraphQL `errors[]` beside a 200)
    /// into the right variant, from the body X sent. Understands v2 problems
    /// (`{title, type, detail}`) and legacy/GraphQL errors
    /// (`{errors: [{code, message}]}`). 401 and token-permission problems are
    /// credentials (exit 2); missing posts/users are `NotFound` and too-long
    /// text `InvalidInput` (exit 3); reply limits are `ReplyRestricted`;
    /// anything else X refused is `Forbidden` (exit 5).
    pub fn from_x_error(provider: &'static str, status: u16, body: &str) -> Self {
        let json: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
        let first = json.pointer("/errors/0");
        let code = first
            .and_then(|e| e.get("code"))
            .and_then(|c| c.as_u64())
            .unwrap_or(0) as u32;
        let problem_type = str_at(Some(&json), "type")
            .or_else(|| str_at(first, "type"))
            .filter(|t| *t != "about:blank")
            .map(|t| t.rsplit('/').next().unwrap_or(t).to_string());
        let reason = str_at(Some(&json), "detail")
            .or_else(|| str_at(first, "detail"))
            .or_else(|| str_at(first, "message"))
            .or_else(|| str_at(Some(&json), "title"))
            .map(str::to_string)
            .unwrap_or_else(|| {
                let raw = crate::utils::safe_truncate(body.trim(), 300);
                if raw.is_empty() { format!("HTTP {status}") } else { raw.to_string() }
            });
        let x_type = problem_type.or_else(|| (code != 0).then(|| format!("code {code}")));

        if status == 401 || matches!(code, 32 | 89 | 215) {
            return Self::AuthMissing { provider, message: format!("HTTP {status}: {reason}") };
        }
        if x_type.as_deref() == Some("oauth1-permissions") || code == 261 {
            return Self::AuthMissing {
                provider,
                message: format!(
                    "HTTP {status}: {reason}. The access token predates Read+Write: regenerate \
                    it at developer.x.com (app, Keys and tokens), then run \
                    `xmaster config set keys.access_token <token>` and \
                    `xmaster config set keys.access_token_secret <secret>`"
                ),
            };
        }
        if matches!(code, 8 | 34 | 50 | 144) || x_type.as_deref() == Some("resource-not-found") {
            return Self::NotFound(reason);
        }
        if code == 186 {
            return Self::InvalidInput(format!("{reason} (over 280 weighted characters; longer posts need X Premium)"));
        }
        if is_reply_limit(code, &reason) {
            return Self::ReplyRestricted(reason);
        }
        Self::Forbidden { reason, x_type }
    }

    /// A write X removed from self-serve API tiers (likes and follows on
    /// 2026-04-20; quote posts).
    pub fn removed_from_api(what: &str) -> Self {
        Self::Forbidden {
            reason: format!("X removed {what} from self-serve API tiers on 2026-04-20; no key change fixes this"),
            x_type: Some("endpoint-removed".into()),
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Config(_) | Self::AuthMissing { .. } | Self::PaymentRequired { .. } => 2,
            Self::InvalidInput(_) | Self::NotFound(_) | Self::CommandDenied(_) | Self::OAuth(_) => 3,
            Self::RateLimited { .. } => 4,
            Self::Forbidden { .. } | Self::ReplyRestricted(_) => 5,
            Self::Api { .. }
            | Self::ServerError { .. }
            | Self::Db(_)
            | Self::Media(_)
            | Self::Http(_)
            | Self::Json(_)
            | Self::Io(_) => 1,
        }
    }

    pub fn error_code(&self) -> &'static str {
        match self {
            Self::ReplyRestricted(_) => "reply_restricted",
            Self::Forbidden { .. } => "forbidden",
            Self::PaymentRequired { .. } => "payment_required",
            Self::Api { code, .. } => code,
            Self::AuthMissing { .. } => "auth_missing",
            Self::RateLimited { .. } => "rate_limited",
            Self::ServerError { .. } => "server_error",
            Self::Config(_) => "config_error",
            Self::InvalidInput(_) => "invalid_input",
            Self::Db(_) => "db_error",
            Self::Media(_) => "media_error",
            Self::NotFound(_) => "not_found",
            Self::CommandDenied(_) => "command_denied",
            Self::Http(_) => "http_error",
            Self::Json(_) => "json_error",
            Self::Io(_) => "io_error",
            Self::OAuth(_) => "oauth_error",
        }
    }

    /// The next thing to do, ideally a command to run.
    pub fn suggestion(&self) -> String {
        match self {
            Self::ReplyRestricted(_) => {
                "If web cookies are missing, run `xmaster config web-login` so replies go through \
                your browser session. If the web session was used, the author limits who can \
                reply: pick another post, or quote it with `xmaster post <text> --quote <id>`"
                    .into()
            }
            Self::Forbidden { reason, x_type } => match classify(x_type.as_deref(), reason) {
                Refusal::Duplicate => {
                    "X rejects text identical to one of your recent posts. Reword it; the same text will fail again".into()
                }
                Refusal::Automated => {
                    "X flagged the request as automated (226). Pause 15+ minutes and retry once; \
                    if it repeats, post this one in the app"
                        .into()
                }
                Refusal::Locked => {
                    "The account is locked or suspended. Open x.com in a browser, clear the challenge, then retry".into()
                }
                Refusal::NotVisible => {
                    "The post or account is protected, deleted or blocking you. Check it with \
                    `xmaster read <id>` or `xmaster user <handle>`, then pick another target"
                        .into()
                }
                Refusal::NotOnTier => {
                    "Your X API tier can't do this. Run `xmaster config web-login` so it goes \
                    through your browser session (settings.web_writes = auto or always)"
                        .into()
                }
                Refusal::DailyLimit => "X's daily limit for this action is reached. Retry in about 24h".into(),
                Refusal::AlreadyDone => "Already done on X; nothing to retry".into(),
                Refusal::Other => {
                    "X refused this action. Don't retry it unchanged: change the text or target, \
                    or route it through the web session with `xmaster config web-login`"
                        .into()
                }
            },
            Self::AuthMissing { provider, .. } => match *provider {
                "xai" => "Set XMASTER_KEYS__XAI or run `xmaster config set keys.xai <key>`".into(),
                "x-web" => "Run `xmaster config web-login` (captures your browser's X cookies)".into(),
                _ => "Run `xmaster config check` to see which X key is missing or rejected, then \
                    `xmaster config set keys.<name> <value>` (or XMASTER_KEYS__<NAME>)"
                    .into(),
            },
            Self::PaymentRequired { .. } => {
                "X API pay-per-use credits are exhausted, or this endpoint needs a paid add-on. \
                Top up at https://console.x.com; retrying won't help until then"
                    .into()
            }
            Self::RateLimited { reset_at, .. } => {
                let secs = secs_until(*reset_at);
                if *reset_at == 0 {
                    format!("Wait {secs}s (X sent no reset time), then retry. Limits: `xmaster rate-limits`")
                } else {
                    format!("Wait {secs}s, then retry. Limits: `xmaster rate-limits`")
                }
            }
            Self::ServerError { status } => format!(
                "X returned HTTP {status}. Reads: retry in 30s. Writes: check your profile \
                (`xmaster timeline`) before retrying, the post may have gone through"
            ),
            Self::Api { code: "write_unconfirmed", .. } => {
                "Check your profile (`xmaster timeline`) or the thread before retrying; the write may have gone through".into()
            }
            Self::Api { code: "retry_exhausted", .. } => "X kept failing. Retry in 30s".into(),
            Self::Api { provider: "x-web", .. } => {
                "The web session request failed. Run `xmaster config check` to test the session; \
                `xmaster config web-login` refreshes cookies"
                    .into()
            }
            Self::Api { .. } => "Read the message for X's reason; `xmaster config check` tests keys and the web session".into(),
            Self::Config(_) => "Run `xmaster config check` to see what's missing; `xmaster config show` lists settings".into(),
            Self::InvalidInput(_) => "Fix the arguments; `xmaster <command> --help` shows the usage".into(),
            Self::Db(msg) if is_lock(msg) => "Another xmaster process holds the database. Retry in a few seconds".into(),
            Self::Db(_) => {
                "A local database in ~/.config/xmaster failed (not an X or key problem). Run \
                `xmaster update`, then retry; report it if it persists"
                    .into()
            }
            Self::Media(_) => "Check the file path, type (jpg/png/gif/webp/mp4) and size, then retry".into(),
            Self::NotFound(_) => "Check the ID, URL or handle; it may be deleted or private".into(),
            Self::CommandDenied(_) => {
                "Blocked by XMASTER_ALLOW_COMMANDS / XMASTER_DENY_COMMANDS in this environment; ask the owner".into()
            }
            Self::Http(e) if e.is_connect() || e.is_timeout() => {
                "Network problem reaching X. Reads: retry in a few seconds. Writes: check your \
                profile before retrying, the post may have gone through"
                    .into()
            }
            Self::Http(_) => "HTTP request failed; run `xmaster config check` to test connectivity and keys".into(),
            Self::OAuth(_) => {
                "OAuth signing failed: check keys.api_key, api_secret, access_token and \
                access_token_secret with `xmaster config check`"
                    .into()
            }
            Self::Json(_) => "Unexpected response shape from X; retry once, then report it with RUST_LOG=xmaster=debug".into(),
            Self::Io(_) => "Check the file path and permissions".into(),
        }
    }

    /// Transport level: may `XApi::request` retry this in-process? 429 and
    /// 5xx only; a timeout after send must not be retried (duplicate writes).
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::RateLimited { .. } | Self::ServerError { .. })
    }

    /// Agent level (`error.retryable`): may the same command succeed if run
    /// again later, unchanged? Writes that may have landed are reported as
    /// `write_unconfirmed`, which is not transient.
    pub fn is_transient(&self) -> bool {
        self.retry_after_secs().is_some()
    }

    /// How long to wait before retrying (`error.retry_after_secs`); `None`
    /// when retrying won't help.
    pub fn retry_after_secs(&self) -> Option<u64> {
        match self {
            Self::RateLimited { reset_at, .. } => Some(secs_until(*reset_at)),
            Self::ServerError { .. } | Self::Api { code: "retry_exhausted", .. } => Some(30),
            Self::Http(e) if e.is_connect() || e.is_timeout() => Some(5),
            Self::Db(msg) if is_lock(msg) => Some(2),
            _ => None,
        }
    }
}

fn str_at<'a>(v: Option<&'a serde_json::Value>, key: &str) -> Option<&'a str> {
    v.and_then(|v| v.get(key)).and_then(|s| s.as_str()).filter(|s| !s.is_empty())
}

fn is_lock(msg: &str) -> bool {
    msg.contains("locked") || msg.contains("busy")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn exit_codes_match_the_table() {
        let documented: Vec<i32> = EXIT_CODES.iter().map(|(c, _)| *c).collect();
        let cases = [
            XmasterError::AuthMissing { provider: "x", message: String::new() },
            XmasterError::Config(String::new()),
            XmasterError::PaymentRequired { message: String::new() },
            XmasterError::InvalidInput(String::new()),
            XmasterError::NotFound(String::new()),
            XmasterError::RateLimited { provider: "x", reset_at: 0 },
            XmasterError::Forbidden { reason: String::new(), x_type: None },
            XmasterError::ReplyRestricted(String::new()),
            XmasterError::Db(String::new()),
        ];
        let expected = [2, 2, 2, 3, 3, 4, 5, 5, 1];
        for (e, want) in cases.iter().zip(expected) {
            assert_eq!(e.exit_code(), want, "{e:?}");
            assert!(documented.contains(&want));
        }
    }

    #[test]
    fn sqlite_errors_are_db_not_config() {
        let e: XmasterError = rusqlite::Error::InvalidQuery.into();
        assert_eq!(e.error_code(), "db_error");
        assert_eq!(e.exit_code(), 1);
        assert!(!e.is_transient());
        let locked = XmasterError::Db("database is locked".into());
        assert_eq!(locked.retry_after_secs(), Some(2));
    }

    #[test]
    fn rate_limit_reports_seconds_not_a_timestamp() {
        let e = XmasterError::RateLimited { provider: "x", reset_at: now() + 120 };
        let secs = e.retry_after_secs().unwrap();
        assert!((118..=120).contains(&secs), "{secs}");
        assert!(e.is_transient());
        assert!(e.to_string().contains("retry in"));
        let unknown = XmasterError::RateLimited { provider: "x-web", reset_at: 0 };
        assert_eq!(unknown.retry_after_secs(), Some(DEFAULT_RATE_LIMIT_SECS));
        let past = XmasterError::RateLimited { provider: "x", reset_at: now() - 5 };
        assert_eq!(past.retry_after_secs(), Some(1));
    }

    #[test]
    fn transport_retry_set_is_unchanged() {
        assert!(XmasterError::ServerError { status: 503 }.is_retryable());
        assert!(!XmasterError::Db("database is locked".into()).is_retryable());
        assert!(!XmasterError::Api { provider: "x", code: "write_unconfirmed", message: String::new() }.is_transient());
    }

    #[test]
    fn v2_duplicate_403_is_forbidden_with_reword_hint() {
        let body = r#"{"detail":"You are not allowed to create a Tweet with duplicate content.","type":"about:blank","title":"Forbidden","status":403}"#;
        let e = XmasterError::from_x_error("x", 403, body);
        assert_eq!(e.exit_code(), 5);
        assert_eq!(e.error_code(), "forbidden");
        assert!(e.suggestion().contains("Reword"));
        assert!(!e.is_transient());
    }

    #[test]
    fn v2_reply_restriction_is_reply_restricted() {
        let body = r#"{"detail":"Reply to this conversation is not allowed because you have not been mentioned or otherwise engaged by the author of the post you are replying to.","type":"about:blank","title":"Forbidden","status":403}"#;
        let e = XmasterError::from_x_error("x", 403, body);
        assert!(matches!(e, XmasterError::ReplyRestricted(_)));
        assert_eq!(e.exit_code(), 5);
    }

    #[test]
    fn graphql_codes_classify() {
        let e = XmasterError::from_x_error(
            "x-web",
            200,
            r#"{"errors":[{"message":"Authorization: Status is a duplicate. (187)","code":187}]}"#,
        );
        assert!(matches!(&e, XmasterError::Forbidden { x_type: Some(t), .. } if t == "code 187"));
        assert!(e.suggestion().contains("Reword"));
        let e = XmasterError::from_x_error("x-web", 200, r#"{"errors":[{"message":"Restricted","code":433}]}"#);
        assert!(matches!(e, XmasterError::ReplyRestricted(_)));
        let e = XmasterError::from_x_error("x-web", 200, r#"{"errors":[{"message":"looks automated","code":226}]}"#);
        assert!(e.suggestion().contains("226"));
    }

    #[test]
    fn missing_and_too_long_are_bad_input() {
        let e = XmasterError::from_x_error(
            "x-web",
            200,
            r#"{"errors":[{"message":"_Missing: Tweet record for tweetId: '1' not found","code":144}]}"#,
        );
        assert!(matches!(e, XmasterError::NotFound(_)));
        assert_eq!(e.exit_code(), 3);
        let e = XmasterError::from_x_error("x-web", 200, r#"{"errors":[{"message":"Tweet needs to be a bit shorter.","code":186}]}"#);
        assert_eq!(e.error_code(), "invalid_input");
    }

    #[test]
    fn credential_problems_stay_auth() {
        let e = XmasterError::from_x_error("x", 401, r#"{"title":"Unauthorized","type":"about:blank","status":401,"detail":"Unauthorized"}"#);
        assert_eq!(e.exit_code(), 2);
        let perms = r#"{"title":"Forbidden","type":"https://api.twitter.com/2/problems/oauth1-permissions","detail":"Your client app is not configured with the appropriate oauth1 app permissions for this endpoint."}"#;
        let e = XmasterError::from_x_error("x", 403, perms);
        assert!(matches!(e, XmasterError::AuthMissing { .. }));
        assert!(e.to_string().contains("regenerate"));
    }

    #[test]
    fn tier_and_removed_endpoints_point_at_web_session() {
        let body = r#"{"client_id":"1","reason":"client-not-enrolled","title":"Client Forbidden","type":"https://api.twitter.com/2/problems/client-forbidden","detail":"When authenticating requests to the Twitter API v2 endpoints, you must use keys and tokens from a Twitter developer App that is attached to a Project."}"#;
        let e = XmasterError::from_x_error("x", 403, body);
        assert!(e.to_string().contains("[client-forbidden]"));
        assert!(e.suggestion().contains("web-login"));
        let e = XmasterError::removed_from_api("likes");
        assert_eq!(e.exit_code(), 5);
        assert!(e.suggestion().contains("web-login"));
    }

    #[test]
    fn non_json_body_keeps_raw_text() {
        let e = XmasterError::from_x_error("x", 403, "  <html>nope</html> ");
        assert!(matches!(&e, XmasterError::Forbidden { reason, x_type: None } if reason == "<html>nope</html>"));
        let e = XmasterError::from_x_error("x", 403, "");
        assert!(e.to_string().contains("HTTP 403"));
    }
}
