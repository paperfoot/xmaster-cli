//! OAuth 2.0 PKCE flow for X API endpoints that require it (bookmarks, etc.)
//!
//! Flow:
//! 1. Generate PKCE code_verifier + code_challenge
//! 2. Start a one-shot TCP listener on localhost:3000
//! 3. Open browser to X authorization URL
//! 4. Wait for callback with authorization code
//! 5. Exchange code for access + refresh tokens
//! 6. Save tokens to config

use crate::config::{self, AppConfig};
use crate::errors::XmasterError;
use base64::Engine;
use rand::Rng;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const AUTH_URL: &str = "https://x.com/i/oauth2/authorize";
const TOKEN_URL: &str = "https://api.x.com/2/oauth2/token";
const REDIRECT_URI: &str = "http://localhost:3000/callback";
const SCOPES: &str = "tweet.read tweet.write users.read bookmark.read bookmark.write offline.access";

/// Build a shared reqwest client for OAuth2 operations with proper timeout and user-agent.
fn build_oauth2_client() -> Result<reqwest::Client, crate::errors::XmasterError> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .user_agent(format!("xmaster/{}", env!("CARGO_PKG_VERSION")))
        .pool_idle_timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| crate::errors::XmasterError::Config(format!("Failed to build OAuth2 HTTP client: {e}")))
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    #[allow(dead_code)]
    token_type: String,
    #[allow(dead_code)]
    expires_in: Option<u64>,
    #[allow(dead_code)]
    scope: Option<String>,
}

fn base64_url_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn generate_code_verifier() -> String {
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..32).map(|_| rng.gen()).collect();
    base64_url_encode(&bytes)
}

fn generate_code_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let hash = hasher.finalize();
    base64_url_encode(&hash)
}

fn generate_state() -> String {
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..16).map(|_| rng.gen()).collect();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Run the full OAuth 2.0 PKCE authorization flow.
///
/// 1. Binds a one-shot TCP listener on localhost:3000
/// 2. Opens the browser to the X authorization page
/// 3. Waits for the callback GET request with ?code=...&state=...
/// 4. Exchanges the code for tokens
/// 5. Saves tokens to config
pub async fn authorize(client_id: &str, client_secret: &str) -> Result<(), XmasterError> {
    let code_verifier = generate_code_verifier();
    let code_challenge = generate_code_challenge(&code_verifier);
    let state = generate_state();

    let auth_url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
        AUTH_URL,
        urlencoding::encode(client_id),
        urlencoding::encode(REDIRECT_URI),
        urlencoding::encode(SCOPES),
        urlencoding::encode(&state),
        urlencoding::encode(&code_challenge),
    );

    // Bind the listener BEFORE opening the browser
    let listener = TcpListener::bind("127.0.0.1:3000").await.map_err(|e| {
        XmasterError::Config(format!(
            "Failed to bind localhost:3000 — is something else using that port? {e}"
        ))
    })?;

    eprintln!("Listening on http://localhost:3000/callback ...");
    eprintln!("Opening browser for authorization...");

    // Open browser
    let _ = std::process::Command::new("open").arg(&auth_url).spawn();

    // Wait for the single callback request (with 2-minute timeout)
    let (code, returned_state) = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        wait_for_callback(&listener),
    )
    .await
    .map_err(|_| {
        XmasterError::Config("Authorization timed out after 2 minutes.".into())
    })??;

    // Validate state
    if returned_state != state {
        return Err(XmasterError::Config(
            "State mismatch — possible CSRF attack. Try again.".into(),
        ));
    }

    // Exchange code for tokens
    let tokens = exchange_code(client_id, client_secret, &code, &code_verifier).await?;

    // Save to config
    save_tokens(&tokens.access_token, tokens.refresh_token.as_deref())?;

    eprintln!("OAuth 2.0 authorization complete! Tokens saved.");
    Ok(())
}

/// Wait for the callback GET request on the TCP listener.
/// Returns (code, state) extracted from query params.
/// Sends an HTML success page back to the browser.
async fn wait_for_callback(listener: &TcpListener) -> Result<(String, String), XmasterError> {
    let (mut stream, _addr) = listener.accept().await?;

    // Read the HTTP request (should be small — just a GET with query params)
    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await?;
    let request = String::from_utf8_lossy(&buf[..n]);

    // Parse the request line: GET /callback?code=...&state=... HTTP/1.1
    let first_line = request.lines().next().unwrap_or("");
    let path = first_line.split_whitespace().nth(1).unwrap_or("");

    // Parse query parameters from the path
    let (code, state) = parse_callback_params(path)?;

    // Send success response
    let html = "<html><body><h1>xmaster authorized!</h1><p>You can close this tab.</p></body></html>";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        html.len(),
        html
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;

    Ok((code, state))
}

fn parse_callback_params(path: &str) -> Result<(String, String), XmasterError> {
    let url = url::Url::parse(&format!("http://localhost{path}")).map_err(|e| {
        XmasterError::Config(format!("Failed to parse callback URL: {e}"))
    })?;

    if url.path() != "/callback" {
        return Err(XmasterError::Config(format!(
            "Unexpected callback path: {}",
            url.path()
        )));
    }

    let code = url
        .query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.to_string())
        .ok_or_else(|| XmasterError::Config("Missing 'code' in callback".into()))?;

    let state = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.to_string())
        .ok_or_else(|| XmasterError::Config("Missing 'state' in callback".into()))?;

    Ok((code, state))
}

async fn exchange_code(
    client_id: &str,
    client_secret: &str,
    code: &str,
    code_verifier: &str,
) -> Result<TokenResponse, XmasterError> {
    let client = build_oauth2_client()?;

    let params = [
        ("code", code),
        ("grant_type", "authorization_code"),
        ("client_id", client_id),
        ("redirect_uri", REDIRECT_URI),
        ("code_verifier", code_verifier),
    ];

    let resp = client
        .post(TOKEN_URL)
        .basic_auth(client_id, Some(client_secret))
        .form(&params)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        return Err(XmasterError::AuthMissing {
            provider: "x-oauth2",
            message: format!("Token exchange failed (HTTP {status}): {text}"),
        });
    }

    let tokens: TokenResponse = resp.json().await?;
    Ok(tokens)
}

async fn refresh_token_request(
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<TokenResponse, XmasterError> {
    let client = build_oauth2_client()?;

    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ];

    let resp = client
        .post(TOKEN_URL)
        .basic_auth(client_id, Some(client_secret))
        .form(&params)
        .send()
        .await?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(XmasterError::AuthMissing {
            provider: "x-oauth2",
            message: format!("Token refresh failed: {text}. Run: xmaster config auth"),
        });
    }

    let tokens: TokenResponse = resp.json().await?;
    Ok(tokens)
}

fn save_tokens(access_token: &str, refresh_token: Option<&str>) -> Result<(), XmasterError> {
    let path = config::config_path();
    let existing = if path.exists() {
        std::fs::read_to_string(&path).unwrap_or_default()
    } else {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        String::new()
    };

    let mut doc: toml::Table = existing
        .parse()
        .map_err(|e: toml::de::Error| XmasterError::Config(format!("Parse error: {e}")))?;

    let keys = doc
        .entry("keys".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));

    if let toml::Value::Table(ref mut t) = keys {
        t.insert(
            "oauth2_access_token".to_string(),
            toml::Value::String(access_token.to_string()),
        );
        if let Some(rt) = refresh_token {
            t.insert(
                "oauth2_refresh_token".to_string(),
                toml::Value::String(rt.to_string()),
            );
        }
    }

    let toml_str = toml::to_string_pretty(&doc)
        .map_err(|e| XmasterError::Config(format!("Serialize error: {e}")))?;

    // Atomic write: write to temp file then rename to prevent partial writes
    let tmp_path = path.with_extension("toml.tmp");
    std::fs::write(&tmp_path, &toml_str)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600))?;
    }

    std::fs::rename(&tmp_path, &path)?;

    Ok(())
}

/// Ensure we have a valid OAuth 2.0 access token, refreshing if needed.
pub async fn ensure_oauth2_token(config: &AppConfig) -> Result<String, XmasterError> {
    let access_token = &config.keys.oauth2_access_token;
    let refresh_token = &config.keys.oauth2_refresh_token;
    let client_id = &config.keys.oauth2_client_id;
    let client_secret = &config.keys.oauth2_client_secret;

    if access_token.is_empty() && refresh_token.is_empty() {
        return Err(XmasterError::AuthMissing {
            provider: "x-oauth2",
            message: "No OAuth 2.0 tokens. Run: xmaster config auth".into(),
        });
    }

    // If we have a refresh token, try refreshing (access tokens expire every 2 hours)
    if !refresh_token.is_empty() && !client_id.is_empty() && !client_secret.is_empty() {
        match refresh_token_request(client_id, client_secret, refresh_token).await {
            Ok(tokens) => {
                save_tokens(&tokens.access_token, tokens.refresh_token.as_deref())?;
                return Ok(tokens.access_token);
            }
            Err(e) => {
                // If refresh fails but we have an access token, try it anyway
                if !access_token.is_empty() {
                    eprintln!("Warning: token refresh failed ({e}), using existing access token");
                    return Ok(access_token.clone());
                }
                return Err(e);
            }
        }
    }

    // Fall back to the stored access token
    if !access_token.is_empty() {
        return Ok(access_token.clone());
    }

    Err(XmasterError::AuthMissing {
        provider: "x-oauth2",
        message: "OAuth 2.0 token expired and no refresh token available. Run: xmaster config auth".into(),
    })
}

/// Make an authenticated GET request using OAuth 2.0 bearer token
pub async fn oauth2_get(
    url: &str,
    access_token: &str,
) -> Result<serde_json::Value, XmasterError> {
    let client = build_oauth2_client()?;
    let resp = client
        .get(url)
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .await?;

    if resp.status().as_u16() == 401 {
        return Err(XmasterError::AuthMissing {
            provider: "x-oauth2",
            message: "OAuth 2.0 token expired. Run: xmaster config auth".into(),
        });
    }

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        return Err(XmasterError::Api {
            provider: "x",
            code: "api_error",
            message: format!("HTTP {status}: {text}"),
        });
    }

    let json: serde_json::Value = resp.json().await?;
    Ok(json)
}
