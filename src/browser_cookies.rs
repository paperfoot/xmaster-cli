//! Extract X.com cookies from local browser databases.
//!
//! Supports Chrome (encrypted, macOS Keychain) and Firefox (plain SQLite).
//! Used by `xmaster config web-login` for zero-interaction cookie capture.

use crate::errors::XmasterError;
use std::path::PathBuf;

/// Cookies needed for X web session auth.
pub struct WebCookies {
    pub ct0: String,
    pub auth_token: String,
}

/// Try all supported browsers, return the first that has valid X cookies.
pub fn extract() -> Result<WebCookies, XmasterError> {
    // Try Chrome first (most popular), then Firefox
    type BrowserExtractor = fn() -> Result<WebCookies, XmasterError>;
    let browsers: Vec<(&str, BrowserExtractor)> = vec![
        ("Chrome", extract_chrome),
        ("Firefox", extract_firefox),
        ("Brave", extract_brave),
        ("Chromium", extract_chromium),
        ("Edge", extract_edge),
    ];

    let mut errors = Vec::new();
    for (name, extractor) in &browsers {
        match extractor() {
            Ok(cookies) => {
                eprintln!("  Found valid X cookies in {name}");
                return Ok(cookies);
            }
            Err(e) => {
                let msg = format!("{name}: {e}");
                eprintln!("  {msg}");
                errors.push(msg);
            }
        }
    }

    // Filter out "not found" errors to show only real failures
    let real_errors: Vec<_> = errors
        .iter()
        .filter(|e| !e.contains("not found"))
        .collect();

    let detail = if real_errors.is_empty() {
        "No supported browsers found with X cookies.".into()
    } else {
        real_errors
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("; ")
    };

    Err(XmasterError::Config(format!(
        "{detail} Make sure you're logged into x.com in Chrome, Firefox, Brave, or Edge."
    )))
}

// ---------------------------------------------------------------------------
// Chrome (macOS) — encrypted cookies via Keychain
// ---------------------------------------------------------------------------

fn extract_chrome() -> Result<WebCookies, XmasterError> {
    let cookie_db = chrome_cookie_path("Google/Chrome")?;
    extract_chromium_based(&cookie_db)
}

fn extract_brave() -> Result<WebCookies, XmasterError> {
    let cookie_db = chrome_cookie_path("BraveSoftware/Brave-Browser")?;
    extract_chromium_based(&cookie_db)
}

fn extract_chromium() -> Result<WebCookies, XmasterError> {
    let cookie_db = chrome_cookie_path("Chromium")?;
    extract_chromium_based(&cookie_db)
}

fn extract_edge() -> Result<WebCookies, XmasterError> {
    let cookie_db = chrome_cookie_path("Microsoft Edge")?;
    extract_chromium_based(&cookie_db)
}

fn chrome_cookie_path(browser_dir: &str) -> Result<PathBuf, XmasterError> {
    let home = std::env::var("HOME").map_err(|_| XmasterError::Config("HOME not set".into()))?;

    // Try Default profile first, then Profile 1
    let profiles = ["Default", "Profile 1", "Profile 2"];
    for profile in &profiles {
        let path = PathBuf::from(&home)
            .join("Library/Application Support")
            .join(browser_dir)
            .join(profile)
            .join("Cookies");
        if path.exists() {
            return Ok(path);
        }
    }

    Err(XmasterError::Config(format!(
        "Cookie database not found for {browser_dir}"
    )))
}

fn extract_chromium_based(cookie_db: &PathBuf) -> Result<WebCookies, XmasterError> {
    // Copy to temp file with random name (avoid collisions, no leftover leaks)
    let tmp = std::env::temp_dir().join(format!(
        "xmaster_cookies_{}.sqlite",
        rand::random::<u32>()
    ));

    // Ensure temp files are cleaned up even on error
    let result = extract_chromium_inner(cookie_db, &tmp);

    // Always cleanup regardless of success/failure
    let _ = std::fs::remove_file(&tmp);
    for suffix in &["-wal", "-journal", "-shm"] {
        let _ = std::fs::remove_file(PathBuf::from(format!("{}{suffix}", tmp.display())));
    }

    result
}

fn extract_chromium_inner(
    cookie_db: &PathBuf,
    tmp: &PathBuf,
) -> Result<WebCookies, XmasterError> {
    std::fs::copy(cookie_db, tmp).map_err(|e| {
        XmasterError::Config(format!("Cannot copy cookie DB (is browser running with lock?): {e}"))
    })?;

    // Copy journal/WAL files if present (Chrome may use either mode)
    for suffix in &["-wal", "-journal", "-shm"] {
        let src = PathBuf::from(format!("{}{suffix}", cookie_db.display()));
        let dst = PathBuf::from(format!("{}{suffix}", tmp.display()));
        if src.exists() {
            let _ = std::fs::copy(&src, &dst);
        }
    }

    let conn = rusqlite::Connection::open(tmp)
        .map_err(|e| XmasterError::Config(format!("Cannot open cookie DB: {e}")))?;

    // Enable WAL mode reading
    let _ = conn.pragma_update(None, "journal_mode", "wal");

    // Get the encryption key from macOS Keychain
    let key = get_chrome_key()?;

    // Chrome v130+ (DB version >= 24) prepends a 32-byte SHA256 domain hash
    // to the plaintext before encrypting. We need to strip it after decryption.
    let db_version_str: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'version'",
            [],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| "0".into());
    let db_version: i64 = db_version_str.parse().unwrap_or(0);
    let has_domain_hash = db_version >= 24;

    let mut ct0 = String::new();
    let mut auth_token = String::new();

    // Prefer .x.com cookies (primary domain), fall back to legacy twitter.com
    let mut stmt = conn
        .prepare(
            "SELECT name, encrypted_value, value FROM cookies \
             WHERE host_key IN ('.x.com', 'x.com', '.twitter.com', 'twitter.com') \
             AND name IN ('ct0', 'auth_token') \
             ORDER BY CASE host_key \
                WHEN '.x.com' THEN 1 WHEN 'x.com' THEN 2 \
                WHEN '.twitter.com' THEN 3 ELSE 4 END",
        )
        .map_err(|e| XmasterError::Config(format!("Cookie query failed: {e}")))?;

    let rows = stmt
        .query_map([], |row| {
            let name: String = row.get(0)?;
            let encrypted: Vec<u8> = row.get(1)?;
            let plaintext: String = row.get::<_, String>(2).unwrap_or_default();
            Ok((name, encrypted, plaintext))
        })
        .map_err(|e| XmasterError::Config(format!("Cookie read failed: {e}")))?;

    for row in rows {
        let (name, encrypted, plaintext) = row
            .map_err(|e| XmasterError::Config(format!("Row parse error: {e}")))?;

        // Try plaintext first (older Chrome versions), then decrypt
        let value = if !plaintext.is_empty() {
            plaintext
        } else if encrypted.len() > 3 && (&encrypted[..3] == b"v10" || &encrypted[..3] == b"v11") {
            decrypt_chrome_cookie(&encrypted[3..], &key, has_domain_hash)?
        } else if !encrypted.is_empty() {
            decrypt_chrome_cookie(&encrypted, &key, has_domain_hash).unwrap_or_default()
        } else {
            continue;
        };

        if value.is_empty() {
            continue;
        }

        // Only set if not already set (first match wins — .x.com preferred via ORDER BY)
        match name.as_str() {
            "ct0" if ct0.is_empty() => ct0 = value,
            "auth_token" if auth_token.is_empty() => auth_token = value,
            _ => {}
        }
    }

    if ct0.is_empty() || auth_token.is_empty() {
        return Err(XmasterError::Config(
            "Found cookie DB but ct0/auth_token not present — are you logged into x.com?".into(),
        ));
    }

    Ok(WebCookies { ct0, auth_token })
}

/// Get Chrome's encryption key from macOS Keychain.
fn get_chrome_key() -> Result<Vec<u8>, XmasterError> {
    // Try different Keychain service names for different Chromium browsers
    let services = [
        "Chrome Safe Storage",
        "Chromium Safe Storage",
        "Brave Safe Storage",
        "Microsoft Edge Safe Storage",
    ];

    for service in &services {
        let output = std::process::Command::new("security")
            .args(["find-generic-password", "-s", service, "-w"])
            .output();

        if let Ok(out) = output {
            if out.status.success() {
                let password = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !password.is_empty() {
                    return derive_chrome_key(&password);
                }
            }
        }
    }

    Err(XmasterError::Config(
        "Could not get Chrome encryption key from Keychain. \
        You may need to allow access when prompted."
            .into(),
    ))
}

/// Derive the AES-128-CBC key from Chrome's Keychain password using PBKDF2.
fn derive_chrome_key(password: &str) -> Result<Vec<u8>, XmasterError> {
    use pbkdf2::pbkdf2_hmac;
    use sha1::Sha1;

    let salt = b"saltysalt";
    let iterations = 1003;
    let mut key = vec![0u8; 16]; // AES-128

    pbkdf2_hmac::<Sha1>(password.as_bytes(), salt, iterations, &mut key);
    Ok(key)
}

/// Decrypt a Chrome cookie value using AES-128-CBC with PKCS7 padding.
/// Chrome v130+ (has_domain_hash=true) prepends a 32-byte SHA256 hash of the
/// domain to the plaintext before encrypting, which we strip after decryption.
fn decrypt_chrome_cookie(
    encrypted: &[u8],
    key: &[u8],
    has_domain_hash: bool,
) -> Result<String, XmasterError> {
    use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};

    type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

    if encrypted.len() < 16 {
        return Err(XmasterError::Config("Encrypted cookie too short".into()));
    }

    // Chrome uses 16 space characters (0x20) for the IV on macOS
    let iv = [0x20u8; 16];

    let mut buf = encrypted.to_vec();

    let decrypted = Aes128CbcDec::new(key.into(), &iv.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|_| XmasterError::Config("Cookie decryption failed — key may be wrong".into()))?;

    // Chrome v130+ prepends 32-byte SHA256(domain) to plaintext before encrypting
    let cookie_bytes = if has_domain_hash {
        if decrypted.len() >= 32 {
            &decrypted[32..]
        } else {
            return Err(XmasterError::Config("Decrypted cookie too short for v130+ domain hash".into()));
        }
    } else {
        decrypted
    };

    String::from_utf8(cookie_bytes.to_vec())
        .map_err(|_| XmasterError::Config("Decrypted cookie is not valid UTF-8".into()))
}

// ---------------------------------------------------------------------------
// Firefox — plain SQLite (no encryption!)
// ---------------------------------------------------------------------------

fn extract_firefox() -> Result<WebCookies, XmasterError> {
    let home = std::env::var("HOME").map_err(|_| XmasterError::Config("HOME not set".into()))?;
    let profiles_dir = PathBuf::from(&home).join("Library/Application Support/Firefox/Profiles");

    if !profiles_dir.exists() {
        return Err(XmasterError::Config("Firefox profiles directory not found".into()));
    }

    // Find the default profile (usually ends with .default-release or .default)
    let mut cookie_db = None;
    if let Ok(entries) = std::fs::read_dir(&profiles_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".default-release") || name.ends_with(".default") {
                let path = entry.path().join("cookies.sqlite");
                if path.exists() {
                    cookie_db = Some(path);
                    break;
                }
            }
        }
    }

    let cookie_db = cookie_db.ok_or_else(|| {
        XmasterError::Config("No Firefox profile with cookies.sqlite found".into())
    })?;

    // Copy to temp (Firefox also uses WAL)
    let tmp = std::env::temp_dir().join("xmaster_ff_cookies.sqlite");
    std::fs::copy(&cookie_db, &tmp).map_err(|e| {
        XmasterError::Config(format!("Cannot copy Firefox cookie DB: {e}"))
    })?;

    let wal_src = PathBuf::from(format!("{}-wal", cookie_db.display()));
    let wal_dst = PathBuf::from(format!("{}-wal", tmp.display()));
    if wal_src.exists() {
        let _ = std::fs::copy(&wal_src, &wal_dst);
    }

    let conn = rusqlite::Connection::open_with_flags(
        &tmp,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| XmasterError::Config(format!("Cannot open Firefox cookie DB: {e}")))?;

    let mut ct0 = String::new();
    let mut auth_token = String::new();

    let mut stmt = conn
        .prepare(
            "SELECT name, value FROM moz_cookies \
             WHERE host IN ('.x.com', 'x.com', '.twitter.com', 'twitter.com') \
             AND name IN ('ct0', 'auth_token')",
        )
        .map_err(|e| XmasterError::Config(format!("Firefox cookie query failed: {e}")))?;

    let rows = stmt
        .query_map([], |row| {
            let name: String = row.get(0)?;
            let value: String = row.get(1)?;
            Ok((name, value))
        })
        .map_err(|e| XmasterError::Config(format!("Firefox cookie read failed: {e}")))?;

    for row in rows {
        let (name, value) = row
            .map_err(|e| XmasterError::Config(format!("Row parse error: {e}")))?;
        match name.as_str() {
            "ct0" => ct0 = value,
            "auth_token" => auth_token = value,
            _ => {}
        }
    }

    // Cleanup
    let _ = std::fs::remove_file(&tmp);
    let _ = std::fs::remove_file(&wal_dst);

    if ct0.is_empty() || auth_token.is_empty() {
        return Err(XmasterError::Config(
            "Firefox cookies.sqlite found but ct0/auth_token not present — are you logged into x.com in Firefox?".into(),
        ));
    }

    Ok(WebCookies { ct0, auth_token })
}
