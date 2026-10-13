use figment::{
    providers::{Env, Format, Serialized, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::errors::XmasterError;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub keys: Keys,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub style: Style,
    #[serde(default)]
    pub account: AccountConfig,
    #[serde(default)]
    pub niche: Niche,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Niche {
    /// Comma-separated list of topics the user is interested in, used as the
    /// default fanout for `xmaster engage feed` when no topic is passed.
    /// Example: "AI science,biotech longevity,gene therapy,longevity research".
    /// Set via: xmaster config set niche.topics "AI,biotech,gene therapy"
    #[serde(default)]
    pub topics: String,
}

impl Niche {
    /// Parse the comma-separated `topics` field into a deduped, trimmed Vec.
    /// Empty string returns an empty Vec.
    pub fn topic_list(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        self.topics
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter(|s| seen.insert(s.to_lowercase()))
            .map(str::to_string)
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AccountConfig {
    /// Whether the user has X Premium. Set via: xmaster config set account.premium true
    #[serde(default)]
    pub premium: bool,
    /// User's X bio. Used by `config check` to audit the 4-element formula.
    /// Set via: xmaster config set account.bio "I help X do Y by Z. Proof. Cadence. Link."
    #[serde(default)]
    pub bio: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Style {
    /// The user's writing voice for X posts. Used by analyze and agent-info.
    /// Set via: xmaster config set style.voice "your style description"
    #[serde(default)]
    pub voice: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Keys {
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub api_secret: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub access_token_secret: String,
    #[serde(default)]
    pub xai: String,
    // OAuth 2.0 PKCE (required for bookmarks endpoint)
    #[serde(default)]
    pub oauth2_client_id: String,
    #[serde(default)]
    pub oauth2_client_secret: String,
    #[serde(default)]
    pub oauth2_access_token: String,
    #[serde(default)]
    pub oauth2_refresh_token: String,
    /// Unix epoch when oauth2_access_token expires (0 = unknown). Written on
    /// every token save so ensure_oauth2_token can skip needless refreshes.
    #[serde(default)]
    pub oauth2_expires_at: i64,
    // Web session cookies (fallback for reply restrictions)
    #[serde(default)]
    pub web_ct0: String,
    #[serde(default)]
    pub web_auth_token: String,
    /// GraphQL CreateTweet query ID (rotates every few weeks, auto-updated)
    #[serde(default)]
    pub graphql_create_tweet_id: String,
    /// GraphQL CreateNoteTweet query ID for Premium long posts (rotates, auto-updated)
    #[serde(default)]
    pub graphql_create_note_tweet_id: String,
    /// TypeSafe JEV API key (optional decision model). Falls back to the
    /// TYPESAFE_API_KEY env var. Set via: xmaster config set keys.jev KEY
    #[serde(default)]
    pub jev: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    /// Disable FxTwitter Article-enrichment fallback. Default false (enabled).
    /// FxTwitter is the third-party service xmaster uses to read X Article
    /// bodies — the public v2 API doesn't expose them. Set true to opt out;
    /// `xmaster read`/`metrics`/`timeline` will still work but won't surface
    /// Article content.
    /// Set via: xmaster config set settings.disable_fxtwitter true
    #[serde(default)]
    pub disable_fxtwitter: bool,
    /// Which writes go through the browser session (web cookies) instead of
    /// the paid X API: "auto" (default) = replies, quotes, likes, reposts,
    /// bookmarks and follows, which the self-serve API blocks or removed;
    /// "always" = every write, plain posts included (no per-post API fee);
    /// "never" = API only. Without web cookies everything uses the API.
    /// Set via: xmaster config set settings.web_writes always
    #[serde(default = "default_web_writes")]
    pub web_writes: String,
    /// Turn off JEV judgments even when a key is configured.
    #[serde(default)]
    pub disable_jev: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            timeout: default_timeout(),
            disable_fxtwitter: false,
            web_writes: default_web_writes(),
            disable_jev: false,
        }
    }
}

fn default_timeout() -> u64 {
    15
}

fn default_web_writes() -> String {
    "auto".into()
}

pub fn config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XMASTER_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    // Use ~/.config/xmaster on all platforms (consistent with search-cli, onchain-cli)
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config").join("xmaster")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn load_config() -> Result<AppConfig, XmasterError> {
    let path = config_path();
    let mut figment = Figment::new()
        .merge(Serialized::defaults(AppConfig::default()));

    // Only merge TOML if file exists (config is optional)
    if path.exists() {
        figment = figment.merge(Toml::file_exact(&path));
    }

    let config: AppConfig = figment
        .merge(Env::prefixed("XMASTER_").split("__"))
        .extract()
        .map_err(|e| XmasterError::Config(e.to_string()))?;
    Ok(config)
}

impl AppConfig {
    pub fn has_x_auth(&self) -> bool {
        !self.keys.api_key.is_empty()
            && !self.keys.api_secret.is_empty()
            && !self.keys.access_token.is_empty()
            && !self.keys.access_token_secret.is_empty()
    }

    pub fn has_xai_auth(&self) -> bool {
        !self.keys.xai.is_empty()
    }

    pub fn has_web_cookies(&self) -> bool {
        !self.keys.web_ct0.is_empty() && !self.keys.web_auth_token.is_empty()
    }

    /// Route a write through the browser session first? `blocked_on_api` is
    /// true for writes the self-serve API rejects (replies to non-mentioners,
    /// quotes, likes, follows) or that are free on the web (reposts, bookmarks).
    pub fn web_first(&self, blocked_on_api: bool) -> bool {
        self.has_web_cookies()
            && match self.settings.web_writes.as_str() {
                "never" => false,
                "always" => true,
                _ => blocked_on_api,
            }
    }

    /// JEV API key from config or TYPESAFE_API_KEY, unless JEV is disabled.
    pub fn jev_key(&self) -> Option<String> {
        if self.settings.disable_jev {
            return None;
        }
        Some(self.keys.jev.clone())
            .filter(|k| !k.is_empty())
            .or_else(|| std::env::var("TYPESAFE_API_KEY").ok().filter(|k| !k.is_empty()))
    }

    pub fn masked_key(key: &str) -> String {
        let n = key.chars().count();
        if n <= 8 {
            "*".repeat(n)
        } else {
            let head: String = key.chars().take(4).collect();
            let tail: String = key.chars().skip(n - 4).collect();
            format!("{head}...{tail}")
        }
    }
}
