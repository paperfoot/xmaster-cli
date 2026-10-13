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
    /// the paid X API: "always" (default) = every write except polls, so no
    /// per-post API fee ($0.015, $0.20 with a link), API as fallback;
    /// "auto" = only what the self-serve API blocks or removed (replies,
    /// quotes, likes, follows) plus reposts and bookmarks; "never" = API only.
    /// Without web cookies everything uses the API.
    /// Set via: xmaster config set settings.web_writes auto
    #[serde(default = "default_web_writes")]
    pub web_writes: String,
    /// Turn off JEV judgments even when a key is configured.
    #[serde(default)]
    pub disable_jev: bool,
    /// Read search, threads and timelines through the browser session
    /// (free) when web cookies exist, instead of billed API reads.
    /// Set via: xmaster config set settings.web_reads false
    #[serde(default = "default_true")]
    pub web_reads: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            timeout: default_timeout(),
            disable_fxtwitter: false,
            web_writes: default_web_writes(),
            disable_jev: false,
            web_reads: true,
        }
    }
}

fn default_timeout() -> u64 {
    15
}

fn default_web_writes() -> String {
    "always".into()
}

fn default_true() -> bool {
    true
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

/// Defaults, then `config.toml`, then `XMASTER_SECTION__KEY` env overrides
/// (e.g. `XMASTER_SETTINGS__WEB_READS=false`).
pub fn load_config() -> Result<AppConfig, XmasterError> {
    let path = config_path();
    let file_err = |e: &dyn std::fmt::Display| XmasterError::Config(format!("{}: {e}", path.display()));
    let file = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text.parse::<toml::Table>().map_err(|e| file_err(&e))?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(file_err(&e)),
    };
    build_config(file, std::env::vars())
}

fn build_config(
    file: Option<toml::Table>,
    env: impl IntoIterator<Item = (String, String)>,
) -> Result<AppConfig, XmasterError> {
    let cfg_err = |e: &dyn std::fmt::Display| XmasterError::Config(e.to_string());
    let toml::Value::Table(mut table) = toml::Value::try_from(AppConfig::default()).map_err(|e| cfg_err(&e))? else {
        unreachable!("a struct serializes to a table")
    };
    if let Some(file) = file {
        merge(&mut table, file);
    }
    for (var, raw) in env {
        let Some(rest) = var.strip_prefix("XMASTER_").filter(|r| r.contains("__")) else { continue };
        let path: Vec<String> = rest.split("__").map(str::to_lowercase).collect();
        if path.iter().any(String::is_empty) {
            continue;
        }
        set_env(&mut table, &path, &raw)
            .map_err(|want| XmasterError::Config(format!("{var}: expected {want}, got {raw:?}")))?;
    }
    toml::Value::Table(table).try_into().map_err(|e: toml::de::Error| cfg_err(&e))
}

/// Recursive table merge; `over` wins.
fn merge(base: &mut toml::Table, over: toml::Table) {
    for (k, v) in over {
        match v {
            toml::Value::Table(o) if base.get(&k).is_some_and(toml::Value::is_table) => {
                if let Some(toml::Value::Table(b)) = base.get_mut(&k) {
                    merge(b, o);
                }
            }
            v => {
                base.insert(k, v);
            }
        }
    }
}

/// Set `path` to `raw`, typed like the value already there (an unknown key
/// takes a bool or integer if it parses as one). Err names the expected type.
fn set_env(table: &mut toml::Table, path: &[String], raw: &str) -> Result<(), &'static str> {
    use toml::Value as V;
    let (last, parents) = path.split_last().ok_or("a key")?;
    let mut t = table;
    for p in parents {
        let V::Table(next) = t.entry(p.as_str()).or_insert_with(|| V::Table(Default::default())) else {
            return Err("a value, not a section");
        };
        t = next;
    }
    let v = raw.trim();
    let value = match t.get(last) {
        Some(V::String(_)) => V::String(raw.into()),
        Some(V::Boolean(_)) => V::Boolean(v.to_ascii_lowercase().parse().map_err(|_| "true or false")?),
        Some(V::Integer(_)) => V::Integer(v.parse().map_err(|_| "an integer")?),
        Some(V::Float(_)) => V::Float(v.parse().map_err(|_| "a number")?),
        _ => v
            .parse()
            .map(V::Boolean)
            .or_else(|_| v.parse().map(V::Integer))
            .unwrap_or_else(|_| V::String(raw.into())),
    };
    t.insert(last.clone(), value);
    Ok(())
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

    /// Serve reads from the browser session (free) instead of the paid API?
    pub fn web_reads(&self) -> bool {
        self.settings.web_reads && self.has_web_cookies()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn env_overrides_are_typed_by_the_field() {
        let c = build_config(
            None,
            env(&[
                ("XMASTER_SETTINGS__WEB_READS", "false"),
                ("XMASTER_SETTINGS__TIMEOUT", "30"),
                ("XMASTER_SETTINGS__WEB_WRITES", "never"),
                ("XMASTER_KEYS__API_KEY", "12345"),
                ("XMASTER_Account__Premium", "TRUE"),
            ]),
        )
        .unwrap();
        assert!(!c.settings.web_reads);
        assert_eq!(c.settings.timeout, 30);
        assert_eq!(c.settings.web_writes, "never");
        assert_eq!(c.keys.api_key, "12345");
        assert!(c.account.premium);
    }

    #[test]
    fn unrelated_vars_are_ignored() {
        let c = build_config(
            None,
            env(&[
                ("XMASTER_CONFIG_DIR", "/tmp/x"),
                ("XMASTER_DENY_COMMANDS", "post"),
                ("OTHER__SETTINGS__TIMEOUT", "1"),
                ("XMASTER___TIMEOUT", "1"),
                ("HOME", "/Users/x"),
            ]),
        )
        .unwrap();
        assert_eq!(c.settings.timeout, 15);
        assert!(c.settings.web_reads);
    }

    #[test]
    fn file_merges_over_defaults_and_env_over_file() {
        let file: toml::Table = "[settings]\nweb_writes = \"auto\"\n[keys]\nweb_ct0 = \"abc\"\n".parse().unwrap();
        let c = build_config(Some(file), env(&[("XMASTER_KEYS__WEB_CT0", "env")])).unwrap();
        assert_eq!(c.settings.web_writes, "auto");
        assert_eq!(c.settings.timeout, 15);
        assert!(c.settings.web_reads);
        assert_eq!(c.keys.web_ct0, "env");
    }

    #[test]
    fn bad_typed_env_value_is_a_config_error() {
        let e = build_config(None, env(&[("XMASTER_SETTINGS__WEB_READS", "nah")])).unwrap_err();
        assert!(e.to_string().contains("XMASTER_SETTINGS__WEB_READS"), "{e}");
    }
}
