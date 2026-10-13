use assert_cmd::Command;
use predicates::prelude::*;

/// Every test runs with the pacing rails off: a test that reaches a write
/// path must fail on credentials, not wait on a cooldown.
fn xmaster() -> Command {
    let mut cmd = Command::cargo_bin("xmaster").unwrap();
    cmd.env("XMASTER_RAILS", "off");
    cmd
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
        .stdout(predicate::str::contains("\"name\":\"xmaster\""))
        .stdout(predicate::str::contains("\"commands\""))
        .stdout(predicate::str::contains("\"exit_codes\""));
}

/// Raw manifest, no envelope, whatever the flags; `info` is the same command.
#[test]
fn agent_info_is_raw_json() {
    for args in [&["--json", "agent-info"][..], &["info"], &["agent-info", "--quiet"]] {
        let out = xmaster().args(args).output().unwrap();
        assert!(out.status.success(), "{args:?}");
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(v.get("status").is_none() && v.get("data").is_none(), "{args:?}");
        assert_eq!(v["name"], "xmaster");
    }
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
        .stdout(predicate::str::contains("\"status\":\"success\""));
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
        .code(2); // auth_missing: setup problem
}

#[test]
fn like_without_auth_fails_gracefully() {
    xmaster()
        .env("XMASTER_CONFIG_DIR", "/tmp/xmaster-test-nonexistent")
        .args(["like", "12345"])
        .assert()
        .failure()
        .code(2);
}

#[test]
fn search_ai_without_auth_fails_gracefully() {
    xmaster()
        .env("XMASTER_CONFIG_DIR", "/tmp/xmaster-test-nonexistent")
        .env_remove("XMASTER_KEYS_XAI")
        .args(["search-ai", "test query"])
        .assert()
        .failure()
        .code(2);
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
    assert!(stderr.contains("\"status\":\"error\""));
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
        .code(2); // Fails on auth, but shouldn't panic on URL parsing
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
        .code(2)
        .stderr(predicate::str::contains("auth_missing"));
}

#[test]
fn unknown_command_fails() {
    xmaster()
        .arg("nonexistent")
        .assert()
        .code(3)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("agent-info"));
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
        .code(2)
        .stderr(predicate::str::contains("web-login"));
}

// ─── Global Flags ───────────────────────────────────────────────

#[test]
fn manifest_has_the_framework_keys() {
    let out = xmaster().arg("agent-info").output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    for key in ["name", "version", "description", "commands", "global_flags", "exit_codes", "envelope", "config"] {
        assert!(v.get(key).is_some(), "missing {key}");
    }
    assert_eq!(v["auto_json_when_piped"], true);
    assert!(v["global_flags"]["--json"]["description"].is_string() && v["global_flags"]["--quiet"].is_object());
    for code in ["0", "1", "2", "3", "4", "5"] {
        assert!(v["exit_codes"][code].is_string(), "exit code {code}");
    }
    assert_eq!(v["envelope"]["version"], "1");
    assert!(v["config"]["path"].as_str().unwrap().ends_with("config.toml"));
    assert_eq!(v["config"]["env_prefix"], "XMASTER_");
    // One generated line per canonical path; the key is what --command takes.
    let commands = v["commands"].as_object().unwrap();
    assert!(commands.values().all(|l| l.is_string()));
    assert_eq!(commands["timeline"], "--user= -n= --since= --before= --sort=: Home or user timeline");
}

#[test]
fn agent_info_command_returns_one_command_in_full() {
    let out = xmaster().args(["agent-info", "--command", "engage feed"]).output().unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let feed = &v["commands"]["engage feed"];
    assert_eq!(v["commands"].as_object().unwrap().len(), 1);
    assert_eq!(feed["args"][0]["name"], "topics");
    let names: Vec<&str> = feed["options"].as_array().unwrap().iter().map(|o| o["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["--min-followers", "--max-age-mins", "--count"]);
    // A group selects its leaves.
    let out = xmaster().args(["agent-info", "--command", "rails"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["commands"]["rails status"].is_object() && v["commands"]["rails pause"]["options"][0]["type"] == "number");
    // Unknown path: exit 3, the closest path, nothing on stdout.
    let out = xmaster().args(["agent-info", "--command", "enage feed"]).output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert!(out.stdout.is_empty());
    let err: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert!(err["error"]["message"].as_str().unwrap().contains("did you mean: engage feed"), "{err}");
    xmaster().args(["agent-info", "--command", "post", "--tips"]).assert().code(3);
}

/// Discovery and `config path` read no config: a broken file can't stop them.
#[test]
fn discovery_survives_a_broken_config() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), "style = [broken").unwrap();
    for args in [&["agent-info"][..], &["agent-info", "--command", "reply"], &["config", "path"], &["--help"]] {
        xmaster().env("XMASTER_CONFIG_DIR", dir.path()).args(args).assert().success();
    }
    let out = xmaster().env("XMASTER_CONFIG_DIR", dir.path()).args(["config", "path"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["data"]["exists"], true);
    xmaster().env("XMASTER_CONFIG_DIR", dir.path()).arg("me").assert().code(2);
}

/// Piped help is plain text (readable, no escaped JSON string) with Tips and
/// Examples; --json asks for the envelope.
#[test]
fn help_is_text_unless_json_is_asked_for() {
    let out = xmaster().arg("--help").output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("Tips:") && text.contains("Examples:") && text.contains("-n N"), "{text}");
    assert!(serde_json::from_str::<serde_json::Value>(&text).is_err());

    let out = xmaster().args(["--help", "--json"]).output().unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["status"], "success");
    assert!(v["data"]["usage"].as_str().unwrap().contains("Examples:"));

    let out = xmaster().args(["--version", "--json"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["data"]["version"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn contract_hook_pins_every_exit_code() {
    let out = xmaster().args(["contract", "0"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()["data"]["exit_code"], 0);
    for (code, error) in [(1, "server_error"), (2, "config_error"), (3, "invalid_input"), (4, "rate_limited"), (5, "forbidden")] {
        let out = xmaster().args(["contract", &code.to_string()]).output().unwrap();
        assert_eq!(out.status.code(), Some(code));
        assert!(out.stdout.is_empty());
        let v: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(v["error"]["code"], error);
    }
    xmaster().args(["contract", "9"]).assert().code(3);
    // Hidden: not in help, not in the manifest.
    xmaster().arg("--help").assert().stdout(predicate::str::contains("contract").not());
}

#[test]
fn collections_take_n_and_limit() {
    let dir = tempfile::tempdir().unwrap();
    // The flag parses and the command runs: an empty library is `not_found`.
    for flag in ["-n", "--limit", "-c", "--count"] {
        let out = offline(&dir).args(["inspire", flag, "2"]).output().unwrap();
        let err: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(err["error"]["code"], "not_found", "{flag}: {err}");
    }
}

/// A wrong flag names the right one, or the command's flags, never just "see --help".
#[test]
fn parse_errors_suggest_a_scoped_fix() {
    let err = |args: &[&str]| -> serde_json::Value {
        let out = xmaster().args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(3), "{args:?}");
        assert!(out.stdout.is_empty());
        serde_json::from_slice(&out.stderr).unwrap()
    };
    let e = err(&["timeline", "--cont", "3"]);
    assert!(e["error"]["suggestion"].as_str().unwrap().starts_with("Did you mean --count?"), "{e}");
    assert!(!e["error"]["message"].as_str().unwrap().contains("Usage:"), "{e}");
    let e = err(&["timeline", "--zzz"]);
    assert!(e["error"]["suggestion"].as_str().unwrap().contains("Flags for `xmaster timeline`: --user, -c/--count"), "{e}");
    let e = err(&["engage", "fed"]);
    assert!(e["error"]["suggestion"].as_str().unwrap().contains("`xmaster engage feed`"), "{e}");
    let e = err(&["reply", "123"]);
    assert!(e["error"]["suggestion"].as_str().unwrap().contains("Usage: xmaster reply"), "{e}");
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

// ─── Algorithm-claim regression guards ──────────────────────────
// Catch reintroduction of false algorithm claims. Source of truth:
// xai-org/x-algorithm home-mixer/scorers/ranking_scorer.rs (May 15, 2026).

const BANNED_PHRASES: &[&str] = &[
    "~150x",
    "reply_engaged_by_author",
    "algo doc 06",
    "OpenTweet 2026",
    "time_decay_halflife_minutes\": 360",
    "tweets 5+ drop",
];

const REQUIRED_SIGNALS: &[&str] = &["not_dwelled", "quoted_vqv", "cont_click_dwell_time"];

#[test]
fn agent_info_has_no_banned_phrases() {
    for args in [&["agent-info"][..], &["agent-info", "--tips"]] {
        let stdout = String::from_utf8_lossy(&xmaster().args(args).output().expect("run").stdout).into_owned();
        for phrase in BANNED_PHRASES {
            assert!(!stdout.contains(phrase), "{args:?} contains banned phrase: {phrase}");
        }
    }
}

#[test]
fn agent_info_tips_list_the_scorer_terms() {
    let stdout = String::from_utf8_lossy(
        &xmaster().args(["agent-info", "--tips", "algorithm"]).output().expect("run").stdout,
    )
    .into_owned();
    for signal in REQUIRED_SIGNALS {
        assert!(stdout.contains(signal), "agent-info --tips missing scorer term: {signal}");
    }
}

/// Agents load agent-info on every session; keep it near 7 KB (was 5.8 KB
/// before per-path keys and the framework fields). Detail goes behind --command.
#[test]
fn agent_info_stays_small() {
    let out = xmaster().arg("agent-info").env("XMASTER_CONFIG_DIR", "/nonexistent-xmaster-test").output().expect("run");
    assert!(out.status.success());
    assert!(out.stdout.len() <= 7200, "agent-info is {} bytes", out.stdout.len());
}

#[test]
fn analyze_has_no_banned_phrases() {
    for input in ["Just shipped a new feature.", &"a".repeat(900)] {
        let stdout = String::from_utf8_lossy(
            &xmaster()
                .args(["analyze", input, "--json"])
                .output()
                .expect("run")
                .stdout,
        )
        .into_owned();
        for phrase in BANNED_PHRASES {
            assert!(!stdout.contains(phrase), "analyze({input:.20}…) contains banned phrase: {phrase}");
        }
    }
}

// ─── JEV degrades to today's behaviour ──────────────────────────

/// No key: analyze output is the plain preflight result, no `jev` block.
#[test]
fn analyze_without_jev_key_has_no_jev_block() {
    xmaster()
        .env("XMASTER_CONFIG_DIR", "/tmp/xmaster-test-no-jev")
        .env_remove("TYPESAFE_API_KEY")
        .args(["analyze", "Great post!", "--reply-to", "1234567890123456789", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("reply_generic"))
        .stdout(predicate::str::contains("\"jev\"").not());
}

/// A rejected key (or no network) falls back within the deadline.
#[test]
fn analyze_with_bad_jev_key_falls_back() {
    let started = std::time::Instant::now();
    xmaster()
        .env("XMASTER_CONFIG_DIR", "/tmp/xmaster-test-bad-jev")
        .env("TYPESAFE_API_KEY", "not-a-real-key")
        .args(["analyze", "Plain draft with a number: 42.", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"score\""))
        .stdout(predicate::str::contains("\"jev\"").not());
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

/// settings.disable_jev wins over a configured key.
#[test]
fn analyze_with_jev_disabled_skips_it() {
    xmaster()
        .env("XMASTER_CONFIG_DIR", "/tmp/xmaster-test-jev-off")
        .env("TYPESAFE_API_KEY", "not-a-real-key")
        .env("XMASTER_SETTINGS__DISABLE_JEV", "true")
        .args(["analyze", "Plain draft", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"jev\"").not());
}

// ─── Agent conveniences (v1.9) ──────────────────────────────────

/// An empty config dir, JEV off: no keys, no cookies, no network.
fn offline(dir: &tempfile::TempDir) -> Command {
    let mut cmd = xmaster();
    cmd.env("XMASTER_CONFIG_DIR", dir.path())
        .env("XMASTER_SETTINGS__DISABLE_JEV", "true")
        .env_remove("TYPESAFE_API_KEY");
    cmd
}

fn json_of(out: &[u8]) -> serde_json::Value {
    serde_json::from_slice(out).expect("stdout is one JSON envelope")
}

#[test]
fn malformed_ids_and_handles_exit_3_before_any_call() {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        &["read", "notanid"][..],
        &["like", "https://example.com/a/status/1"],
        &["reply", "abc", "hi"],
        &["conversation", "notanid"],
        &["metrics", "123", "nope"],
        &["follow", "not a handle"],
        &["timeline", "--sort", "vibes"],
    ] {
        let out = offline(&dir).arg("--json").args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(3), "{args:?}");
        let err: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(err["error"]["code"], "bad_input", "{args:?}");
    }
}

#[test]
fn x_term_aliases_are_listed_and_parse() {
    xmaster()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("repost"))
        .stdout(predicate::str::contains("conversation"))
        .stdout(predicate::str::contains("quote"));
    for cmd in ["repost", "unrepost", "reposters", "quote", "convo", "conversation"] {
        xmaster().args([cmd, "--help"]).assert().success();
    }
}

#[test]
fn conversation_takes_a_post_url_and_count() {
    xmaster()
        .args(["conversation", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--count"));
}

#[test]
fn post_dry_run_reports_route_and_cost_without_posting() {
    let dir = tempfile::tempdir().unwrap();
    let out = offline(&dir)
        .args(["post", "Rapamycin extended median lifespan 9-14% in mice", "--dry-run", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let d = &json_of(&out.stdout)["data"];
    assert_eq!(d["dry_run"], true);
    assert_eq!(d["kind"], "post");
    // No web cookies: a plain post goes to the paid API.
    assert_eq!(d["route"], "api");
    assert_eq!(d["est_cost_usd"], 0.015);
    assert!(d["preflight_score"].as_u64().is_some());

    let out = offline(&dir)
        .args(["post", "Read it here: example.com/paper", "--dry-run", "--json"])
        .output()
        .unwrap();
    assert_eq!(json_of(&out.stdout)["data"]["est_cost_usd"], 0.2);
}

#[test]
fn reply_and_quote_dry_runs_name_their_target() {
    let dir = tempfile::tempdir().unwrap();
    let out = offline(&dir)
        .args(["reply", "https://x.com/a/status/1234567890", "Which cohort was that?", "--dry-run", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let d = &json_of(&out.stdout)["data"];
    assert_eq!(d["kind"], "reply");
    assert_eq!(d["reply_to"], "1234567890");

    let out = offline(&dir)
        .args(["quote", "1234567890", "This is the key figure", "--dry-run", "--json"])
        .output()
        .unwrap();
    let d = &json_of(&out.stdout)["data"];
    assert_eq!(d["kind"], "quote");
    assert_eq!(d["quote"], "1234567890");
}

#[test]
fn dry_run_rejects_missing_media_and_bad_polls() {
    let dir = tempfile::tempdir().unwrap();
    offline(&dir)
        .args(["post", "hi", "--media", "/nonexistent/x.png", "--dry-run"])
        .assert()
        .code(3);
    offline(&dir)
        .args(["post", "pick one", "--poll", "only-one", "--dry-run"])
        .assert()
        .code(3);
}
