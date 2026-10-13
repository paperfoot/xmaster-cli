use crate::config::AppConfig;
use crate::errors::XmasterError;
use crate::providers::xapi::UserResponse;
use reqwest::Client;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::OnceCell;

/// Per-process state shared by every provider: config, one HTTP client, and
/// lookups that need doing at most once per command.
pub struct AppContext {
    pub config: AppConfig,
    pub client: Client,
    /// The authenticated account's profile (see `XApi::get_me`).
    pub(crate) me: OnceCell<UserResponse>,
    /// Profiles already resolved this run, by lowercase handle.
    pub(crate) profiles: Mutex<HashMap<String, UserResponse>>,
}

impl AppContext {
    pub fn new(config: AppConfig) -> Result<Self, XmasterError> {
        let client = Client::builder()
            .pool_idle_timeout(Duration::from_secs(60))
            .tcp_nodelay(true)
            .timeout(Duration::from_secs(config.settings.timeout))
            .user_agent(format!("xmaster/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| XmasterError::Config(format!("Failed to build HTTP client: {e}")))?;
        Ok(Self { config, client, me: OnceCell::new(), profiles: Mutex::default() })
    }
}
