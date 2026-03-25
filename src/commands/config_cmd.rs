use crate::browser_cookies;
use crate::config::{self, AppConfig};
use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::output::{self, OutputFormat, Tableable};
use crate::providers::xapi::XApi;
use crate::providers::oauth2;
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
struct ConfigDisplay {
    config_path: String,
    api_key: String,
    api_secret: String,
    access_token: String,
    access_token_secret: String,
    bearer_token: String,
    xai_key: String,
    timeout: u64,
    default_count: usize,
}

impl Tableable for ConfigDisplay {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Setting", "Value"]);
        table.add_row(vec!["Config path", &self.config_path]);
        table.add_row(vec!["API Key", &self.api_key]);
        table.add_row(vec!["API Secret", &self.api_secret]);
        table.add_row(vec!["Access Token", &self.access_token]);
        table.add_row(vec!["Access Token Secret", &self.access_token_secret]);
        table.add_row(vec!["Bearer Token", &self.bearer_token]);
        table.add_row(vec!["xAI Key", &self.xai_key]);
        table.add_row(vec!["Timeout (s)", &self.timeout.to_string()]);
        table.add_row(vec!["Default Count", &self.default_count.to_string()]);
        table
    }
}

#[derive(Serialize)]
struct ConfigSetResult {
    key: String,
    success: bool,
}

impl Tableable for ConfigSetResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Key", "Status"]);
        table.add_row(vec![
            self.key.as_str(),
            if self.success { "Updated" } else { "Failed" },
        ]);
        table
    }
}

#[derive(Serialize)]
struct ConfigCheckResult {
    x_auth: AuthStatus,
    xai_auth: AuthStatus,
}

#[derive(Serialize)]
struct AuthStatus {
    configured: bool,
    valid: bool,
    detail: String,
}

impl Tableable for ConfigCheckResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Provider", "Configured", "Valid", "Detail"]);
        table.add_row(vec![
            "X API",
            if self.x_auth.configured { "Yes" } else { "No" },
            if self.x_auth.valid { "Yes" } else { "No" },
            &self.x_auth.detail,
        ]);
        table.add_row(vec![
            "xAI",
            if self.xai_auth.configured { "Yes" } else { "No" },
            if self.xai_auth.valid { "Yes" } else { "No" },
            &self.xai_auth.detail,
        ]);
        table
    }
}

fn mask(key: &str) -> String {
    if key.is_empty() {
        "(not set)".into()
    } else {
        AppConfig::masked_key(key)
    }
}

pub async fn show(_ctx: Arc<AppContext>, format: OutputFormat) -> Result<(), XmasterError> {
    let cfg = config::load_config()?;
    let display = ConfigDisplay {
        config_path: config::config_path().to_string_lossy().to_string(),
        api_key: mask(&cfg.keys.api_key),
        api_secret: mask(&cfg.keys.api_secret),
        access_token: mask(&cfg.keys.access_token),
        access_token_secret: mask(&cfg.keys.access_token_secret),
        bearer_token: mask(&cfg.keys.bearer_token),
        xai_key: mask(&cfg.keys.xai),
        timeout: cfg.settings.timeout,
        default_count: cfg.settings.count,
    };
    output::render(format, &display, None);
    Ok(())
}

pub async fn set(format: OutputFormat, key: &str, value: &str) -> Result<(), XmasterError> {
    let path = config::config_path();

    // Read existing TOML or start fresh
    let existing = if path.exists() {
        std::fs::read_to_string(&path).unwrap_or_default()
    } else {
        // Ensure config directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        String::new()
    };

    let mut doc: toml::Table = existing
        .parse()
        .map_err(|e: toml::de::Error| XmasterError::Config(format!("Failed to parse config: {e}")))?;

    // Parse key path like "keys.api_key" → ["keys", "api_key"]
    let parts: Vec<&str> = key.split('.').collect();
    match parts.len() {
        1 => {
            doc.insert(parts[0].to_string(), toml::Value::String(value.to_string()));
        }
        2 => {
            let section = doc
                .entry(parts[0].to_string())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            if let toml::Value::Table(ref mut t) = section {
                t.insert(parts[1].to_string(), toml::Value::String(value.to_string()));
            }
        }
        _ => {
            return Err(XmasterError::Config(format!("Invalid key path: {key}")));
        }
    }

    let toml_str = toml::to_string_pretty(&doc)
        .map_err(|e| XmasterError::Config(format!("Failed to serialize config: {e}")))?;
    std::fs::write(&path, toml_str)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }

    let display = ConfigSetResult {
        key: key.to_string(),
        success: true,
    };
    output::render(format, &display, None);
    Ok(())
}

pub async fn check(ctx: Arc<AppContext>, format: OutputFormat) -> Result<(), XmasterError> {
    let x_configured = ctx.config.has_x_auth();
    let xai_configured = ctx.config.has_xai_auth();

    let x_auth = if x_configured {
        let api = XApi::new(ctx.clone());
        match api.get_me().await {
            Ok(user) => AuthStatus {
                configured: true,
                valid: true,
                detail: format!("Authenticated as @{}", user.username),
            },
            Err(e) => AuthStatus {
                configured: true,
                valid: false,
                detail: format!("Auth failed: {e}"),
            },
        }
    } else {
        AuthStatus {
            configured: false,
            valid: false,
            detail: "X API credentials not set".into(),
        }
    };

    let xai_auth = AuthStatus {
        configured: xai_configured,
        valid: xai_configured,
        detail: if xai_configured {
            "xAI API key configured".into()
        } else {
            "xAI API key not set".into()
        },
    };

    let display = ConfigCheckResult { x_auth, xai_auth };
    output::render(format, &display, None);
    Ok(())
}

#[derive(Serialize)]
struct SetupGuide {
    steps: Vec<SetupStep>,
    note: String,
}

#[derive(Serialize)]
struct SetupStep {
    step: u32,
    title: String,
    instructions: String,
    url: Option<String>,
    command: Option<String>,
}

impl Tableable for SetupGuide {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Step", "What to do"]);
        for s in &self.steps {
            let mut detail = s.instructions.clone();
            if let Some(ref url) = s.url {
                detail.push_str(&format!("\n  URL: {url}"));
            }
            if let Some(ref cmd) = s.command {
                detail.push_str(&format!("\n  Run: {cmd}"));
            }
            table.add_row(vec![
                format!("{}. {}", s.step, s.title),
                detail,
            ]);
        }
        table.add_row(vec!["Note".into(), self.note.clone()]);
        table
    }
}

pub async fn guide(format: OutputFormat) -> Result<(), XmasterError> {
    let guide = SetupGuide {
        steps: vec![
            SetupStep {
                step: 1,
                title: "Create X Developer Account".into(),
                instructions: "Go to the X Developer Portal. Sign in with your X account. Accept the Developer Agreement (describe your use case as: 'Personal AI assistant for posting and managing my X account').".into(),
                url: Some("https://developer.x.com/en/portal/petition/essential/basic-info".into()),
                command: None,
            },
            SetupStep {
                step: 2,
                title: "Create a Project and App".into(),
                instructions: "In the Developer Portal dashboard, create a new Project. Inside it, create an App. Name it whatever you like (e.g., 'xmaster').".into(),
                url: Some("https://developer.x.com/en/portal/dashboard".into()),
                command: None,
            },
            SetupStep {
                step: 3,
                title: "Set App Permissions to Read+Write+DM".into(),
                instructions: "Go to your App -> Settings -> User authentication settings. Set App permissions to 'Read and write and Direct message'. Set Type of App to 'Native App'. Set Callback URL to http://localhost:3000/callback. Set Website URL to https://github.com/199-biotechnologies/xmaster. Save.".into(),
                url: None,
                command: None,
            },
            SetupStep {
                step: 4,
                title: "Generate Keys and Tokens".into(),
                instructions: "Go to your App -> Keys and tokens tab. Copy: API Key (Consumer Key), API Secret (Consumer Secret). Then under 'Access Token and Secret', click Generate. IMPORTANT: Generate tokens AFTER setting permissions in Step 3, or they'll be read-only.".into(),
                url: None,
                command: None,
            },
            SetupStep {
                step: 5,
                title: "Configure xmaster with your keys".into(),
                instructions: "Run these commands with your actual keys:".into(),
                url: None,
                command: Some("xmaster config set keys.api_key YOUR_API_KEY\nxmaster config set keys.api_secret YOUR_API_SECRET\nxmaster config set keys.access_token YOUR_ACCESS_TOKEN\nxmaster config set keys.access_token_secret YOUR_ACCESS_TOKEN_SECRET".into()),
            },
            SetupStep {
                step: 6,
                title: "Verify everything works".into(),
                instructions: "This should show your X username:".into(),
                url: None,
                command: Some("xmaster config check".into()),
            },
            SetupStep {
                step: 7,
                title: "(Optional) Add xAI key for AI-powered search".into(),
                instructions: "Get an API key from the xAI console. This enables 'xmaster search-ai' which uses Grok for smarter, cheaper search.".into(),
                url: Some("https://console.x.ai/".into()),
                command: Some("xmaster config set keys.xai YOUR_XAI_KEY".into()),
            },
        ],
        note: "If posting fails with 403 'oauth1-permissions', your Access Token was generated before enabling Read+Write. Go back to Keys and tokens, click Regenerate on Access Token, and update xmaster with the new values.".into(),
    };

    output::render(format, &guide, None);
    Ok(())
}

#[derive(Serialize)]
struct AuthResult {
    status: String,
    message: String,
    auth_url: Option<String>,
    next_step: Option<String>,
}

impl Tableable for AuthResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Field", "Value"]);
        table.add_row(vec!["Status", &self.status]);
        table.add_row(vec!["Message", &self.message]);
        if let Some(ref url) = self.auth_url {
            table.add_row(vec!["Auth URL", url]);
        }
        if let Some(ref next) = self.next_step {
            table.add_row(vec!["Next Step", next]);
        }
        table
    }
}

pub async fn auth(ctx: Arc<AppContext>, format: OutputFormat) -> Result<(), XmasterError> {
    let client_id = &ctx.config.keys.oauth2_client_id;
    let client_secret = &ctx.config.keys.oauth2_client_secret;

    if client_id.is_empty() || client_secret.is_empty() {
        let result = AuthResult {
            status: "missing_credentials".into(),
            message: "OAuth 2.0 Client ID and Secret not configured.".into(),
            auth_url: None,
            next_step: Some(
                "Get them from developer.x.com → your app → Keys and tokens → OAuth 2.0 Client ID and Client Secret. \
                Then run: xmaster config set keys.oauth2_client_id YOUR_ID && \
                xmaster config set keys.oauth2_client_secret YOUR_SECRET".into()
            ),
        };
        output::render(format, &result, None);
        return Ok(());
    }

    // Run the full PKCE flow: listener → browser → callback → token exchange → save
    oauth2::authorize(client_id, client_secret).await?;

    let result = AuthResult {
        status: "success".into(),
        message: "OAuth 2.0 authorization complete! Tokens saved to config.".into(),
        auth_url: None,
        next_step: Some("You can now use: xmaster bookmarks list".into()),
    };
    output::render(format, &result, None);
    Ok(())
}

#[derive(Serialize)]
struct WebLoginResult {
    status: String,
    browser: String,
    message: String,
    ct0_preview: String,
    auth_token_preview: String,
}

impl Tableable for WebLoginResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Field", "Value"]);
        table.add_row(vec!["Status", &self.status]);
        table.add_row(vec!["Browser", &self.browser]);
        table.add_row(vec!["ct0", &self.ct0_preview]);
        table.add_row(vec!["auth_token", &self.auth_token_preview]);
        table.add_row(vec!["Message", &self.message]);
        table
    }
}

pub async fn web_login(format: OutputFormat) -> Result<(), XmasterError> {
    if format == OutputFormat::Table {
        eprintln!("Scanning browsers for X session cookies...");
    }

    let cookies = browser_cookies::extract()?;

    // Save to config automatically
    set(OutputFormat::Json, "keys.web_ct0", &cookies.ct0).await?;
    set(OutputFormat::Json, "keys.web_auth_token", &cookies.auth_token).await?;

    let result = WebLoginResult {
        status: "success".into(),
        browser: "auto-detected".into(),
        message: "Web cookies saved. Replies will now auto-fallback to web session if API blocks them.".into(),
        ct0_preview: AppConfig::masked_key(&cookies.ct0),
        auth_token_preview: AppConfig::masked_key(&cookies.auth_token),
    };
    output::render(format, &result, None);
    Ok(())
}
