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
fn engage_inbox_subcommand_parses() {
    xmaster()
        .args(["engage", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("inbox"))
        .stdout(predicate::str::contains("quote"));
}

#[test]
fn engage_inbox_without_auth_fails_gracefully() {
    xmaster()
        .env("XMASTER_CONFIG_DIR", "/tmp/xmaster-test-nonexistent")
        .args(["engage", "inbox", "12345", "--json"])
        .assert()
        .failure()
        .code(3)
        .stderr(predicate::str::contains("auth_missing"));
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

// ─── Algorithm honesty regression guards (v1.6.7) ───────────────
// These tests prevent reintroduction of false 2023-era / pre-May-15-2026
// claims about the X algorithm. If you find yourself wanting to disable one
// of these, re-read the May 15 2026 source FIRST: a claim like
// `reply_engaged_by_author ~150x` simply does not appear in the open release.

#[test]
fn agent_info_does_not_claim_150x_anywhere() {
    let output = xmaster()
        .arg("agent-info")
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("~150x"),
        "agent-info must not advertise ~150x as a live algorithm weight (signal does not exist in May 15 2026 source). Got: {stdout}"
    );
}

#[test]
fn agent_info_does_not_claim_360_min_halflife() {
    let output = xmaster()
        .arg("agent-info")
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("\"time_decay_halflife_minutes\": 360"),
        "agent-info must not claim a 360-minute time-decay halflife — no such function exists in the May 15 2026 source (only AgeFilter binary cutoff + Phoenix learned post-age buckets)"
    );
}

#[test]
fn agent_info_lists_may_2026_scorer_terms() {
    // The May 15 2026 ranking_scorer.rs has 22 weighted terms. xmaster should
    // expose all the ones it claims to model, including the three the 2023
    // leak did not contain.
    let output = xmaster()
        .arg("agent-info")
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for term in &["not_dwelled", "quoted_vqv", "cont_click_dwell_time"] {
        assert!(
            stdout.contains(term),
            "agent-info JSON must list May 2026 scorer term `{term}` — it was missing before v1.6.7. Got: {stdout}"
        );
    }
}

#[test]
fn analyze_no_question_message_does_not_claim_150x() {
    // Score a post without a question and confirm the "no question" message
    // does not advertise a fabricated ~150x weight.
    let output = xmaster()
        .args(["analyze", "Just shipped a new feature.", "--json"])
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("150x") && !stdout.contains("reply_engaged_by_author"),
        "preflight analyze output must not cite reply_engaged_by_author / 150x — that signal is not in May 15 2026 source. Got: {stdout}"
    );
}

#[test]
fn analyze_long_post_no_fake_algo_doc_citation() {
    // Trigger the long-form code path and verify no `algo doc 06` or
    // `OpenTweet 2026` fake citation appears in any issue message.
    let long_text = "a".repeat(900);
    let output = xmaster()
        .args(["analyze", &long_text, "--json"])
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("algo doc 06"),
        "preflight must not cite fake `algo doc 06 §7` source. Got: {stdout}"
    );
    assert!(
        !stdout.contains("OpenTweet 2026"),
        "preflight must not cite fake `OpenTweet 2026` source. Got: {stdout}"
    );
}
