use crate::errors::XmasterError;
use crate::output::{self, OutputFormat, Tableable};
use serde::{Deserialize, Serialize};

const REPO: &str = "paperfoot/xmaster-cli";

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// Map the current compile-time target to the release asset suffix.
fn asset_name() -> &'static str {
    if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        "xmaster-aarch64-darwin"
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "x86_64") {
        "xmaster-x86_64-darwin"
    } else if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
        "xmaster-x86_64-linux"
    } else if cfg!(target_os = "windows") {
        "xmaster-x86_64-windows.exe"
    } else {
        "xmaster"
    }
}

/// The outcome of `update` / `update --check`.
#[derive(Serialize)]
struct UpdateStatus {
    current: &'static str,
    latest: String,
    update_available: bool,
    /// The binary was replaced (false for `--check` and when up to date).
    updated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    next: Option<&'static str>,
}

impl Tableable for UpdateStatus {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Current", "Latest", "Status"]);
        let status = match (self.updated, self.update_available) {
            (true, _) => "updated",
            (false, true) => "update available: run `xmaster update`",
            (false, false) => "up to date",
        };
        table.add_row(vec![self.current, self.latest.as_str(), status]);
        table
    }
}

/// Is `latest` a newer release than `current`? Numeric, never a downgrade.
fn is_newer(latest: &str, current: &str) -> bool {
    let parts = |v: &str| v.split('.').filter_map(|s| s.parse().ok()).collect::<Vec<u32>>();
    parts(latest) > parts(current)
}

pub async fn execute(format: OutputFormat, check: bool) -> Result<(), XmasterError> {
    let current = env!("CARGO_PKG_VERSION");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(format!("xmaster/{current}"))
        .build()
        .map_err(XmasterError::Http)?;

    let release: GhRelease = client
        .get(format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .send()
        .await?
        .json()
        .await
        .map_err(|e| XmasterError::Api {
            provider: "github",
            code: "update_check_failed",
            message: format!("Failed to check for updates: {e}"),
        })?;

    let latest = release.tag_name.trim_start_matches('v').to_string();
    let available = is_newer(&latest, current);
    if check || !available {
        let status = UpdateStatus {
            current,
            latest,
            update_available: available,
            updated: false,
            next: available.then_some("xmaster update"),
        };
        output::render(format, &status, None);
        return Ok(());
    }
    // Find the right asset for this platform.
    let target = asset_name();
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == target)
        .ok_or_else(|| {
            XmasterError::Config(format!(
                "No release asset found for this platform ({target}). \
                 Install via `cargo install xmaster` or Homebrew instead."
            ))
        })?;

    eprintln!("Downloading {target} v{latest}...");
    let bytes = client
        .get(&asset.browser_download_url)
        .send()
        .await?
        .bytes()
        .await?;

    // Replace the running binary.
    let current_exe = std::env::current_exe()
        .map_err(|e| XmasterError::Io(std::io::Error::new(e.kind(), format!("Cannot locate current binary: {e}"))))?;

    // Write to a temp file next to the binary, then atomic rename.
    let tmp = current_exe.with_extension("tmp");
    std::fs::write(&tmp, &bytes)
        .map_err(|e| XmasterError::Io(std::io::Error::new(e.kind(), format!("Failed to write update: {e}"))))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| XmasterError::Io(std::io::Error::new(e.kind(), format!("Failed to set permissions: {e}"))))?;
    }

    std::fs::rename(&tmp, &current_exe)
        .map_err(|e| XmasterError::Io(std::io::Error::new(e.kind(), format!("Failed to replace binary: {e}"))))?;

    let status = UpdateStatus {
        current,
        latest,
        update_available: true,
        updated: true,
        next: Some("xmaster skill update"),
    };
    output::render(format, &status, None);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn only_newer_releases_count() {
        assert!(is_newer("1.10.0", "1.9.3"));
        assert!(!is_newer("1.8.0", "1.8.0"));
        assert!(!is_newer("1.7.9", "1.8.0"));
    }
}
