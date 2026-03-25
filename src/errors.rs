use thiserror::Error;

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

    #[error("Rate limited by {provider} (resets at {reset_at})")]
    RateLimited {
        provider: &'static str,
        reset_at: u64,
    },

    #[error("Server error (HTTP {status}), retries exhausted")]
    ServerError { status: u16 },

    #[error("Reply restricted: {0}")]
    ReplyRestricted(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Media error: {0}")]
    Media(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error("OAuth error: {0}")]
    OAuth(#[from] reqwest_oauth1::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl XmasterError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Config(_) => 2,
            Self::ReplyRestricted(_) => 1,
            Self::AuthMissing { .. } => 3,
            Self::RateLimited { .. } => 4,
            Self::ServerError { .. } => 1,
            Self::Api { .. } | Self::Http(_) => 1,
            Self::Media(_) => 1,
            Self::NotFound(_) => 1,
            Self::Json(_) => 1,
            Self::Io(_) => 1,
            Self::OAuth(_) => 3,
        }
    }

    pub fn error_code(&self) -> &'static str {
        match self {
            Self::ReplyRestricted(_) => "reply_restricted",
            Self::Api { code, .. } => code,
            Self::AuthMissing { .. } => "auth_missing",
            Self::RateLimited { .. } => "rate_limited",
            Self::ServerError { .. } => "server_error",
            Self::Config(_) => "config_error",
            Self::Media(_) => "media_error",
            Self::NotFound(_) => "not_found",
            Self::Http(_) => "http_error",
            Self::Json(_) => "json_error",
            Self::Io(_) => "io_error",
            Self::OAuth(_) => "oauth_error",
        }
    }

    pub fn suggestion(&self) -> String {
        match self {
            Self::ReplyRestricted(_) => {
                "X restricts programmatic replies to non-mentioners. \
                Run: xmaster config web-login \
                (auto-captures cookies from your browser, zero manual steps)"
                    .into()
            }
            Self::AuthMissing { provider, .. } => {
                if *provider == "xai" {
                    "Set XMASTER_KEYS__XAI env var or run: xmaster config set keys.xai <key>".into()
                } else {
                    "Set X API credentials via env vars (XMASTER_KEYS__API_KEY, etc.) or run: xmaster config set keys.api_key <key>".into()
                }
            }
            Self::RateLimited { reset_at, .. } => {
                format!("Rate limited. Resets at Unix timestamp {reset_at}. Wait and retry, or check limits with: xmaster config check")
            }
            Self::ServerError { .. } => "X API server error. Try again later.".into(),
            Self::Config(msg) => format!("Fix configuration: {msg}"),
            _ => "Check xmaster --help for usage".into(),
        }
    }

    /// Returns true if this error is retryable (429 or 5xx).
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::RateLimited { .. } | Self::ServerError { .. })
    }
}
