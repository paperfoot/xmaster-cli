use crate::errors::XmasterError;

pub async fn execute(check: bool) -> Result<(), XmasterError> {
    let current = env!("CARGO_PKG_VERSION");

    // self_update does blocking I/O — run off the async runtime to avoid
    // Tokio runtime-drop panics (exit 101).
    tokio::task::spawn_blocking(move || {
        let status = self_update::backends::github::Update::configure()
            .repo_owner("199-biotechnologies")
            .repo_name("xmaster")
            .bin_name("xmaster")
            .current_version(current)
            .build()
            .map_err(|e| XmasterError::Config(format!("Update check failed: {e}")))?;

        if check {
            let latest = status
                .get_latest_release()
                .map_err(|e| XmasterError::Config(format!("Failed to check for updates: {e}")))?;

            let latest_ver = latest.version.trim_start_matches('v');
            if latest_ver == current {
                println!("Already up to date (v{current})");
            } else {
                println!("Update available: v{current} -> v{latest_ver}");
                println!("Run `xmaster update` to install");
            }
        } else {
            let result = status
                .update()
                .map_err(|e| XmasterError::Config(format!("Update failed: {e}")))?;

            let new_ver = result.version().trim_start_matches('v');
            if new_ver == current {
                println!("Already up to date (v{current})");
            } else {
                println!("Updated: v{current} -> v{new_ver}");
            }
        }

        Ok(())
    })
    .await
    .map_err(|e| XmasterError::Config(format!("Update task failed: {e}")))?
}
