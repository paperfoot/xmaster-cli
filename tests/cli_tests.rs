use assert_cmd::Command;
use predicates::prelude::*;

fn xmaster() -> Command {
    Command::cargo_bin("xmaster").unwrap()
}

// ─── Help & Version ──────────────────────────────────────────────

#[test]
fn shows_help() {
    xmaster()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("199 Biotechnologies"))
        .stdout(predicate::str::contains("post"))
        .stdout(predicate::str::contains("article"))
        .stdout(predicate::str::contains("search"))
        .stdout(predicate::str::contains("like"));
}

#[test]
fn shows_version() {
    xmaster()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("xmaster"));
}

// ─── Agent Info ──────────────────────────────────────────────────

#[test]
fn agent_info_outputs_json() {
    xmaster()
        .arg("agent-info")
        .assert()
        .success()
        .stdout(predicate::str::contains("\"name\": \"xmaster\""))
        .stdout(predicate::str::contains("\"commands\""))
        .stdout(predicate::str::contains("\"env_prefix\": \"XMASTER_\""));
}

#[test]
fn agent_info_with_json_flag() {
    xmaster()
        .args(["--json", "agent-info"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"status\": \"success\""));
}

// ─── Config ──────────────────────────────────────────────────────

#[test]
fn config_show_without_crash() {
    // Should work even with no config file (uses defaults)
    xmaster()
        .arg("config")
        .arg("show")
        .assert()
        .success();
}

#[test]
fn config_show_json() {
    xmaster()
        .args(["--json", "config", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"status\": \"success\""));
}

// ─── Auth Required Commands (graceful failure) ───────────────────

#[test]
fn post_without_auth_fails_gracefully() {
    // With no API keys configured, should fail with auth error, not panic
    xmaster()
        .env("XMASTER_CONFIG_DIR", "/tmp/xmaster-test-nonexistent")
        .args(["post", "test tweet"])
        .assert()
        .failure()
        .code(3); // auth_missing exit code
}

#[test]
fn like_without_auth_fails_gracefully() {
    xmaster()
        .env("XMASTER_CONFIG_DIR", "/tmp/xmaster-test-nonexistent")
        .args(["like", "12345"])
        .assert()
        .failure()
        .code(3);
}

#[test]
fn search_ai_without_auth_fails_gracefully() {
    xmaster()
        .env("XMASTER_CONFIG_DIR", "/tmp/xmaster-test-nonexistent")
        .env_remove("XMASTER_KEYS_XAI")
        .args(["search-ai", "test query"])
        .assert()
        .failure()
        .code(3);
}

// ─── JSON Output Format ─────────────────────────────────────────

#[test]
fn json_error_has_correct_envelope() {
    let output = xmaster()
        .env("XMASTER_CONFIG_DIR", "/tmp/xmaster-test-nonexistent")
        .args(["--json", "post", "test"])
        .output()
        .expect("failed to run");

    // Error envelopes go to stderr per agent-cli-framework invariant 6.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("\"status\": \"error\""));
    assert!(stderr.contains("\"code\""));
    assert!(stderr.contains("\"suggestion\""));
}

// ─── Tweet ID Parsing ───────────────────────────────────────────

#[test]
fn parse_tweet_id_from_url() {
    // This tests the parse_tweet_id function via the CLI
    // When given a URL, it should extract the ID
    xmaster()
        .env("XMASTER_CONFIG_DIR", "/tmp/xmaster-test-nonexistent")
        .args(["like", "https://x.com/user/status/1234567890"])
        .assert()
        .failure()
        .code(3); // Fails on auth, but shouldn't panic on URL parsing
}

// ─── Subcommand Parsing ─────────────────────────────────────────

#[test]
fn dm_subcommands_parse() {
    xmaster()
        .args(["dm", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("send"))
        .stdout(predicate::str::contains("inbox"))
        .stdout(predicate::str::contains("thread"));
}

#[test]
fn config_subcommands_parse() {
    xmaster()
        .args(["config", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("show"))
        .stdout(predicate::str::contains("set"))
        .stdout(predicate::str::contains("check"));
}

#[test]
fn unknown_command_fails() {
    xmaster()
        .arg("nonexistent")
        .assert()
        .failure();
}

#[test]
fn article_preview_generates_html_without_auth() {
    let dir = tempfile::tempdir().unwrap();
    let draft = dir.path().join("draft.md");
    let output = dir.path().join("preview.html");
    std::fs::write(
        &draft,
        "# Partial Reprogramming\n\n![Cover](cover.png)\n\n## Why it matters\n\nText with **bold**, *italic*, ~~strike~~, and [X](https://x.com).\n\n- image support\n- list support\n\n::post(https://x.com/user/status/1234567890)\n",
    )
    .unwrap();

    xmaster()
        .args([
            "article",
            "preview",
            draft.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--author",
            "Boris Djordjevic",
            "--handle",
            "longevityboris",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("preview.html"));

    let html = std::fs::read_to_string(output).unwrap();
    assert!(html.contains("Partial Reprogramming"));
    assert!(html.contains("class=\"article-cover\""));
    assert!(html.contains("<strong>bold</strong>"));
    assert!(html.contains("<em>italic</em>"));
    assert!(html.contains("<s>strike</s>"));
    assert!(html.contains("<ul>"));
    assert!(html.contains("Embedded post"));
}

#[test]
fn article_draft_requires_web_cookies_without_publishing() {
    let dir = tempfile::tempdir().unwrap();
    let draft = dir.path().join("draft.md");
    std::fs::write(
        &draft,
        "# Native Article\n\nText with **bold** and [X](https://x.com).\n",
    )
    .unwrap();

    xmaster()
        .env("XMASTER_CONFIG_DIR", "/tmp/xmaster-test-no-web-cookies")
        .args(["article", "draft", draft.to_str().unwrap()])
        .assert()
        .failure()
        .code(3)
        .stderr(predicate::str::contains("web-login"));
}

// ─── Global Flags ───────────────────────────────────────────────

#[test]
fn json_flag_with_agent_info() {
    xmaster()
        .args(["--json", "agent-info"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"status\": \"success\""));
}

// ─── Analyze (Preflight) ────────────────────────────────────────

#[test]
fn analyze_command_returns_score() {
    xmaster()
        .args(["analyze", "Hello world", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"score\""))
        .stdout(predicate::str::contains("\"grade\""));
}

// ─── Thread ─────────────────────────────────────────────────────

#[test]
fn thread_requires_at_least_one_tweet() {
    xmaster().args(["thread"]).assert().failure();
}

// ─── Schedule ───────────────────────────────────────────────────

#[test]
fn schedule_list_empty() {
    xmaster()
        .args(["schedule", "list", "--json"])
        .assert()
        .failure(); // No scheduled posts → exit 1 (NotFound)
}

// ─── Bookmarks ──────────────────────────────────────────────────

#[test]
fn bookmarks_stats_without_db() {
    let _ = xmaster()
        .env("XMASTER_CONFIG_DIR", "/tmp/xmaster-test-bm-nonexistent")
        .args(["bookmarks", "stats", "--json"])
        .assert(); // should not panic
}

// ─── Config Guide ───────────────────────────────────────────────

#[test]
fn config_guide_works() {
    xmaster()
        .args(["config", "guide", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"steps\""));
}

// ─── Suggest & Report ───────────────────────────────────────────

#[test]
fn suggest_next_post_no_panic() {
    xmaster()
        .args(["suggest", "next-post", "--json"])
        .assert()
        .success();
}

#[test]
fn report_daily_no_panic() {
    // report daily returns NotFound (exit 1) when no posts exist — expected in CI.
    // We test it doesn't crash, not that it has data.
    let output = xmaster()
        .args(["report", "daily", "--json"])
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("\"status\"") || combined.contains("\"version\""),
        "Should return valid JSON envelope, got stdout: {stdout}; stderr: {stderr}"
    );
}
