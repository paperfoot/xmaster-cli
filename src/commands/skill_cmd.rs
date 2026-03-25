use crate::errors::XmasterError;
use crate::output::{self, OutputFormat, Tableable};
use serde::Serialize;
use std::path::PathBuf;

/// The SKILL.md content, embedded at compile time from the skill file.
const SKILL_CONTENT: &str = include_str!("../../skill/SKILL.md");

/// All directories where agent platforms look for skills.
/// We write to ~/.agents/skills/ (universal) and symlink from platform-specific dirs.
fn skill_targets() -> Vec<SkillTarget> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let h = PathBuf::from(&home);

    vec![
        SkillTarget {
            name: "Universal (.agents)",
            path: h.join(".agents/skills/xmaster"),
            is_primary: true,
        },
        SkillTarget {
            name: "Claude Code / Claude Desktop",
            path: h.join(".claude/skills/xmaster"),
            is_primary: false,
        },
        SkillTarget {
            name: "Codex CLI / Codex App",
            path: h.join(".codex/skills/xmaster"),
            is_primary: false,
        },
        SkillTarget {
            name: "Gemini CLI",
            path: h.join(".gemini/skills/xmaster"),
            is_primary: false,
        },
    ]
}

struct SkillTarget {
    name: &'static str,
    path: PathBuf,
    is_primary: bool,
}

#[derive(Serialize)]
struct InstallResult {
    installed: Vec<InstallEntry>,
    skill_version: String,
    message: String,
}

#[derive(Serialize)]
struct InstallEntry {
    platform: String,
    path: String,
    method: String, // "written" or "symlinked" or "already_current" or "skipped"
}

impl Tableable for InstallResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Platform", "Path", "Status"]);
        for e in &self.installed {
            table.add_row(vec![&e.platform, &e.path, &e.method]);
        }
        table
    }
}

#[derive(Serialize)]
struct StatusResult {
    locations: Vec<StatusEntry>,
    skill_version: String,
    bundled_version: String,
    needs_update: bool,
}

#[derive(Serialize)]
struct StatusEntry {
    platform: String,
    path: String,
    installed: bool,
    current: bool,
}

impl Tableable for StatusResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Platform", "Path", "Installed", "Current"]);
        for e in &self.locations {
            table.add_row(vec![
                e.platform.as_str(),
                e.path.as_str(),
                if e.installed { "Yes" } else { "No" },
                if e.current { "Yes" } else if e.installed { "Outdated" } else { "-" },
            ]);
        }
        if self.needs_update {
            table.add_row(vec!["", "", "", "Run: xmaster skill update"]);
        }
        table
    }
}

fn bundled_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Write the primary skill file and symlink from other locations.
fn install_skill() -> Result<InstallResult, XmasterError> {
    let targets = skill_targets();
    let mut entries = Vec::new();

    // Find the primary (universal) target
    let primary = targets.iter().find(|t| t.is_primary).unwrap();
    let primary_skill_path = primary.path.join("SKILL.md");

    // Write to primary location
    std::fs::create_dir_all(&primary.path)?;
    std::fs::write(&primary_skill_path, SKILL_CONTENT)?;
    entries.push(InstallEntry {
        platform: primary.name.to_string(),
        path: primary_skill_path.to_string_lossy().to_string(),
        method: "written".to_string(),
    });

    // Symlink from platform-specific directories
    for target in targets.iter().filter(|t| !t.is_primary) {
        let target_skill = target.path.join("SKILL.md");

        // Check if it's already a symlink pointing to the right place
        if target_skill.is_symlink() {
            if let Ok(link_target) = std::fs::read_link(&target_skill) {
                if link_target == primary_skill_path {
                    entries.push(InstallEntry {
                        platform: target.name.to_string(),
                        path: target_skill.to_string_lossy().to_string(),
                        method: "already_linked".to_string(),
                    });
                    continue;
                }
            }
            // Wrong symlink — remove and re-create
            let _ = std::fs::remove_file(&target_skill);
        }

        // If it's a regular file (not symlink), check if content matches
        if target_skill.exists() && !target_skill.is_symlink() {
            if let Ok(existing) = std::fs::read_to_string(&target_skill) {
                if existing == SKILL_CONTENT {
                    entries.push(InstallEntry {
                        platform: target.name.to_string(),
                        path: target_skill.to_string_lossy().to_string(),
                        method: "already_current".to_string(),
                    });
                    continue;
                }
            }
            // Outdated content — replace with symlink
            let _ = std::fs::remove_file(&target_skill);
        }

        // Create parent dir and symlink
        if std::fs::create_dir_all(&target.path).is_err() {
            entries.push(InstallEntry {
                platform: target.name.to_string(),
                path: target.path.to_string_lossy().to_string(),
                method: "skipped (no dir)".to_string(),
            });
            continue;
        }

        #[cfg(unix)]
        {
            match std::os::unix::fs::symlink(&primary_skill_path, &target_skill) {
                Ok(_) => {
                    entries.push(InstallEntry {
                        platform: target.name.to_string(),
                        path: target_skill.to_string_lossy().to_string(),
                        method: "symlinked".to_string(),
                    });
                }
                Err(e) => {
                    // Fallback: copy instead of symlink
                    let _ = std::fs::write(&target_skill, SKILL_CONTENT);
                    entries.push(InstallEntry {
                        platform: target.name.to_string(),
                        path: target_skill.to_string_lossy().to_string(),
                        method: format!("copied (symlink failed: {e})"),
                    });
                }
            }
        }

        #[cfg(not(unix))]
        {
            // Windows: just copy
            let _ = std::fs::write(&target_skill, SKILL_CONTENT);
            entries.push(InstallEntry {
                platform: target.name.to_string(),
                path: target_skill.to_string_lossy().to_string(),
                method: "copied".to_string(),
            });
        }
    }

    Ok(InstallResult {
        installed: entries,
        skill_version: bundled_version(),
        message: format!(
            "xmaster skill v{} installed to all detected agent platforms",
            bundled_version()
        ),
    })
}

pub async fn install(format: OutputFormat) -> Result<(), XmasterError> {
    let result = install_skill()?;
    output::render(format, &result, None);
    Ok(())
}

pub async fn update(format: OutputFormat) -> Result<(), XmasterError> {
    // Update is the same as install — overwrites with latest bundled version
    let result = install_skill()?;
    let mut result = result;
    result.message = format!(
        "xmaster skill updated to v{} across all platforms",
        bundled_version()
    );
    output::render(format, &result, None);
    Ok(())
}

pub async fn status(format: OutputFormat) -> Result<(), XmasterError> {
    let targets = skill_targets();
    let mut locations = Vec::new();
    let mut needs_update = false;

    for target in &targets {
        let skill_path = target.path.join("SKILL.md");
        let installed = skill_path.exists();
        let current = if installed {
            std::fs::read_to_string(&skill_path)
                .map(|c| c == SKILL_CONTENT)
                .unwrap_or(false)
        } else {
            false
        };
        if installed && !current {
            needs_update = true;
        }
        locations.push(StatusEntry {
            platform: target.name.to_string(),
            path: skill_path.to_string_lossy().to_string(),
            installed,
            current,
        });
    }

    let result = StatusResult {
        locations,
        skill_version: bundled_version(),
        bundled_version: bundled_version(),
        needs_update,
    };
    output::render(format, &result, None);
    Ok(())
}
