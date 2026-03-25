use figment::{
    providers::{Env, Format, Serialized, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::errors::XmasterError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub keys: Keys,
    #[serde(default)]
    pub settings: Settings,
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
    pub bearer_token: String,
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
    // Web session cookies (fallback for reply restrictions)
    #[serde(default)]
    pub web_ct0: String,
    #[serde(default)]
    pub web_auth_token: String,
    /// GraphQL CreateTweet query ID (rotates every few weeks, auto-updated)
    #[serde(default)]
    pub graphql_create_tweet_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    #[serde(default = "default_count")]
    pub count: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            timeout: default_timeout(),
            count: default_count(),
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            keys: Keys::default(),
            settings: Settings::default(),
        }
    }
}

fn default_timeout() -> u64 {
    15
}

fn default_count() -> usize {
    10
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

    pub fn masked_key(key: &str) -> String {
        if key.len() <= 8 {
            "*".repeat(key.len())
        } else {
            format!("{}...{}", &key[..4], &key[key.len() - 4..])
        }
    }
}
