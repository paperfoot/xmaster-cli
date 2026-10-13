//! `xmaster agent-info`: what an agent needs to drive xmaster, in under 6 KB.
//! Commands come from clap and exit codes from `errors::EXIT_CODES`, so
//! neither can drift. Strategy and X-algorithm notes sit behind
//! `--tips [topic]`, loaded only when an agent asks.

use crate::config;
use crate::errors::EXIT_CODES;
use crate::output::{self, OutputFormat, Tableable};
use clap::{ArgAction, CommandFactory};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write;

/// Topics accepted by `agent-info --tips <topic>`.
pub const TIP_TOPICS: &[&str] = &["all", "algorithm", "replies", "posting", "growth", "metrics", "avoid"];

/// Groups listed leaf by leaf; other groups collapse to one line.
const EXPAND: &[&str] = &["engage"];
/// Commands for people, not agents.
const SKIP_COMMANDS: &[&str] = &["help", "star"];
/// Flags left out of the command lines: global, described once in `usage`,
/// or tuning thresholds (`--min-*`, `--max-*`) that `--help` lists.
const SKIP_FLAGS: &[&str] = &["help", "version", "json", "csv", "pretty", "count"];
const SKIP_FLAG_PREFIXES: &[&str] = &["min-", "max-"];

#[derive(Serialize)]
struct AgentInfo {
    name: &'static str,
    version: &'static str,
    description: &'static str,
    usage: &'static str,
    setup: [&'static str; 4],
    workflows: [&'static str; 5],
    commands: Vec<String>,
    exit_codes: BTreeMap<String, &'static str>,
    errors: &'static str,
    cost: Cost,
    pacing: &'static str,
    jev: Jev,
    access: &'static str,
    tips: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    writing_style: Option<&'static str>,
}

#[derive(Serialize)]
struct Cost {
    free: &'static str,
    paid: &'static str,
}

#[derive(Serialize)]
struct Jev {
    active: bool,
    model: &'static str,
    judges: [&'static str; 4],
    setup: &'static str,
}

impl Tableable for AgentInfo {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Field", "Value"]);
        table.add_row(vec!["Name", self.name]);
        table.add_row(vec!["Version", self.version]);
        table.add_row(vec!["Description", self.description]);
        table.add_row(vec!["Setup", &self.setup.join("\n")]);
        table.add_row(vec!["Workflows", &self.workflows.join("\n")]);
        table.add_row(vec!["Commands", &self.commands.join("\n")]);
        let codes: Vec<String> = self.exit_codes.iter().map(|(c, m)| format!("{c}: {m}")).collect();
        table.add_row(vec!["Exit codes", &codes.join("\n")]);
        table.add_row(vec!["Free", self.cost.free]);
        table.add_row(vec!["Paid", self.cost.paid]);
        table.add_row(vec!["Pacing", self.pacing]);
        table.add_row(vec!["JEV", if self.jev.active { "active" } else { self.jev.setup }]);
        table.add_row(vec!["Tips", self.tips]);
        if let Some(style) = self.writing_style {
            table.add_row(vec!["Writing style", style]);
        }
        table
    }
}

pub fn execute(format: OutputFormat, tips: Option<&str>) {
    if let Some(topic) = tips {
        return render_tips(format, topic);
    }
    let cfg = config::load_config().ok();
    let jev_active = cfg
        .as_ref()
        .is_some_and(|c| c.jev_key().is_some() && !c.settings.disable_jev);
    // The voice can run to a page; point at it rather than inline it.
    let writing_style = cfg
        .is_some_and(|c| !c.style.voice.is_empty())
        .then_some("set: `config get style.voice` before drafting");

    let info = AgentInfo {
        name: "xmaster",
        version: env!("CARGO_PKG_VERSION"),
        description: "X CLI for AI agents: read, post and reply to any thread through the user's browser session (free); draft checks; JEV judgments",
        usage: "JSON when piped. Posts: ID or URL; users: handle or URL. Lists: -c N. Flags ending in = take a value. <cmd> --help for more",
        setup: [
            "config web-login: copy X cookies from the user's browser (free reads and writes)",
            "config check: live read-only test of the web session and keys",
            "config set account.bio, niche.topics: what feed and JEV judge fit against",
            "optional: keys.jev (JEV), keys.xai (search-ai), X API keys (paid fallback)",
        ],
        workflows: [
            "reply: engage feed -> conversation <id> -> reply <id> \"text\" (--dry-run first)",
            "inbox: engage inbox <own post id>: reply to recommendations, like triage.like_ids",
            "post: post \"text\" --dry-run, then post (originals need the user's yes)",
            "after: track run; metrics ID1 ID2 ... in one call",
            "failed reply: don't retry blindly or thread it; give the user the text",
        ],
        commands: command_lines(),
        exit_codes: EXIT_CODES.iter().map(|(c, m)| (c.to_string(), *m)).collect(),
        errors: "stderr: {error:{code,message,suggestion,retryable,retry_after_secs?}}. write_unconfirmed: may have landed, check first",
        cost: Cost {
            free: "with web cookies: read, conversation, replies, search, timelines, mentions, profiles, others' metrics; all posts, replies, quotes, likes, reposts, bookmarks, follows, deletes, mutes, blocks",
            paid: "X API ($0.015/post, $0.20 with a link): no web session, polls, DMs, lists, bookmarks, likers, reposters, quotes, followers, users, volume, own private metrics; search-ai, trending: xAI. --dry-run shows route, cost",
        },
        pacing: "Writes keep daily limits and gaps, within normal use. If one is refused, the error says why: wait or try later, don't loop",
        jev: Jev {
            active: jev_active,
            model: crate::providers::jev::MODEL,
            judges: [
                "engage feed: best 5 targets with jev_score, why",
                "engage inbox: recommendations need a reply; like triage.like_ids",
                "replies: drops low-value replies (--all: all)",
                "analyze, post, reply, quote: advisory flags generic/slop/off_topic/bait",
            ],
            setup: "keys.jev or TYPESAFE_API_KEY; off: settings.disable_jev true; on failure: heuristics",
        },
        access: "XMASTER_READ_ONLY=1 blocks writes to X; XMASTER_ALLOW_COMMANDS / XMASTER_DENY_COMMANDS",
        tips: "agent-info --tips [algorithm|replies|posting|growth|metrics|avoid]: ranking facts from X's code; usage advice",
        writing_style,
    };
    output::render(format, &info, None);
}

/// One line per command, generated from clap: `name|alias <arg> [opt] --flag --value=: about`.
/// An undo command shares its base's line: `like/unlike <id>`.
fn command_lines() -> Vec<String> {
    let mut cli = crate::cli::Cli::command();
    cli.build();
    let subs: Vec<&clap::Command> = cli.get_subcommands().filter(|c| !SKIP_COMMANDS.contains(&c.get_name())).collect();
    let has = |n: &str| subs.iter().any(|c| c.get_name() == n);
    let mut out = Vec::new();
    for cmd in &subs {
        let name = cmd.get_name();
        if name.strip_prefix("un").is_some_and(has) {
            continue;
        }
        describe(cmd, "", has(&format!("un{name}")), &mut out);
    }
    out
}

fn describe(cmd: &clap::Command, prefix: &str, undo: bool, out: &mut Vec<String>) {
    let name = format!("{prefix}{}", cmd.get_name());
    let subs: Vec<&clap::Command> = cmd.get_subcommands().filter(|c| c.get_name() != "help").collect();
    if !subs.is_empty() {
        if EXPAND.contains(&name.as_str()) {
            for sub in subs {
                describe(sub, &format!("{name} "), false, out);
            }
        } else {
            let names: Vec<String> = subs.iter().map(|s| leaf_names(s)).collect();
            out.push(format!("{name} {}: {}", names.join("|"), about(cmd)));
        }
        return;
    }
    let mut line = name;
    if undo {
        let _ = write!(line, "/un{}", cmd.get_name());
    }
    for alias in cmd.get_visible_aliases() {
        let _ = write!(line, "|{alias}");
        if undo {
            let _ = write!(line, "/un{alias}");
        }
    }
    for arg in cmd.get_arguments() {
        let id = arg.get_id().as_str();
        if arg.is_positional() {
            let many = matches!(arg.get_action(), ArgAction::Append);
            let dots = if many { "..." } else { "" };
            if arg.is_required_set() {
                let _ = write!(line, " <{id}{dots}>");
            } else {
                let _ = write!(line, " [{id}{dots}]");
            }
        } else if let Some(long) = arg
            .get_long()
            .filter(|l| !SKIP_FLAGS.contains(l) && !SKIP_FLAG_PREFIXES.iter().any(|p| l.starts_with(p)))
        {
            let value = if arg.get_action().takes_values() { "=" } else { "" };
            let _ = write!(line, " --{long}{value}");
        }
    }
    // "like <id>" needs no "Like a post"; keep an about that says more.
    let about = about(cmd);
    let words: Vec<&str> = about.split_whitespace().collect();
    let first = words.first().map(|w| w.to_ascii_lowercase()).unwrap_or_default();
    let restates = words.len() <= 3 && (first == cmd.get_name() || cmd.get_visible_aliases().any(|a| a == first));
    if !restates && !about.is_empty() {
        let _ = write!(line, ": {about}");
    }
    out.push(line);
}

/// `watchlist` becomes `watchlist(add,list,remove)` inside a collapsed group.
fn leaf_names(cmd: &clap::Command) -> String {
    let subs: Vec<&str> = cmd.get_subcommands().map(|s| s.get_name()).filter(|n| *n != "help").collect();
    if subs.is_empty() {
        cmd.get_name().to_string()
    } else {
        format!("{}({})", cmd.get_name(), subs.join(","))
    }
}

/// The about's first clause: "Reply to a post (shorthand ...)" -> "Reply to a post".
fn about(cmd: &clap::Command) -> String {
    let text = cmd.get_about().map(|a| a.to_string()).unwrap_or_default();
    let first = text.lines().next().unwrap_or_default();
    let clause = first.split([':', '(']).next().unwrap_or_default();
    clause.trim().trim_end_matches(['.', ',']).to_string()
}

// ---------------------------------------------------------------------------
// --tips
// ---------------------------------------------------------------------------

/// Production ranking weights: xai-org/x-algorithm home-mixer/params/param.rs
/// (cron-synced from X's config; last sync 2026-10-12). Each multiplies the
/// viewer's predicted probability of the action, not raw counts.
#[derive(Serialize)]
struct Weights {
    share_via_copy_link: f64,
    reply: f64,
    quote: f64,
    share_via_dm: f64,
    follow_author: f64,
    share: f64,
    retweet: f64,
    favorite: f64,
    cont_click_dwell_time: f64,
    click: f64,
    open_link: f64,
    photo_expand: f64,
    dwell: f64,
    quoted_click: f64,
    video_open: f64,
    post_unexplored: f64,
    cont_dwell_time: f64,
    profile_click: f64,
    vqv: f64,
    quoted_vqv: f64,
    not_dwelled: f64,
    block_author: f64,
    not_interested: f64,
    mute_author: f64,
    report: f64,
}

const WEIGHTS: Weights = Weights {
    share_via_copy_link: 20.0,
    reply: 5.0,
    quote: 5.0,
    share_via_dm: 5.0,
    follow_author: 4.0,
    share: 2.0,
    retweet: 1.0,
    favorite: 0.6,
    cont_click_dwell_time: 0.4,
    click: 0.3,
    open_link: 0.2,
    photo_expand: 0.05,
    dwell: 0.05,
    quoted_click: 0.05,
    video_open: 0.02,
    post_unexplored: 0.015,
    cont_dwell_time: 0.006,
    profile_click: 0.0,
    vqv: 0.0,
    quoted_vqv: 0.0,
    not_dwelled: -0.02,
    block_author: -31.2,
    not_interested: -47.52,
    mute_author: -58.8,
    report: -234.0,
};

#[derive(Serialize)]
struct Algorithm {
    source: &'static str,
    score: &'static str,
    weights: Weights,
    facts: &'static [&'static str],
}

#[derive(Serialize)]
struct Tips {
    #[serde(skip_serializing_if = "Option::is_none")]
    algorithm: Option<Algorithm>,
    #[serde(skip_serializing_if = "Option::is_none")]
    replies: Option<&'static [&'static str]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    posting: Option<&'static [&'static str]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    growth: Option<&'static [&'static str]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metrics: Option<&'static [&'static str]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    avoid: Option<&'static [&'static str]>,
}

const ALGORITHM_FACTS: &[&str] = &[
    "Reply weight is 20 (5 + 15) on an original post by an author the viewer mutually follows; the dwell boost is 0",
    "A post whose predicted negatives outweigh its positives sinks below every positive post for that viewer",
    "An author's 1st/2nd/3rd/4th unseen post in one feed request scores x1/x0.625/x0.44/x0.34 (floor x0.25)",
    "Out-of-network posts score x0.75, and so do in-network replies and reposts; topic feeds x0.5",
    "Hard 48 h age cutoff and no continuous time decay; the model sees post age in 1 h buckets",
    "Cold start: each For You request lifts at most one post to rank 16: an original (not a reply or repost) by an author with <=50,000 followers, <=2 h old, under 200 home impressions, picked by a bandit on likes per impression",
    "For You never shows a reply to people who don't follow the replier; strangers see replies only under the post, in the OP's notifications and in search",
    "Only the top-scored post of a conversation reaches a feed (DedupConversationFilter), so a long thread competes with itself",
    "Links carry no ranking penalty (open_link +0.2). The risks are URL reputation and the API's $0.20 fee for a post with a link",
    "Profile clicks, video watch time (vqv) and bookmarks have no ranking weight",
    "The ranker sees post content (text and media embeddings), like/reply/repost/quote/view counts, post age, and whether the author follows the viewer. It does not see follower count, Premium status or hashtags",
    "Premium has no For You multiplier. It seeds user-cred (a PageRank), and verified status is a feature when replies under a post are ordered",
    "Replies under a post are ordered by a model that uses: the OP liked or replied to the reply, the OP follows the replier, verified status, a Grok reply score (0-3), and blocks, mutes and reports on the reply",
];

const REPLIES: &[&str] = &[
    "`reply <id|url> \"text\"` reaches any thread through the web session. Without it, the X API only allows replies to people who mentioned you",
    "Read first: `conversation <id>` returns the parent chain, the post and its top replies in one free call",
    "Name a specific detail from the post (a claim, number, method or result), or ask the author something they can answer. An OP who likes or answers your reply lifts its place under the post",
    "Check drafts with `reply <id> \"text\" --dry-run` or `analyze \"text\" --reply-to <id>`; rewrite once if JEV flags generic, slop, off_topic or bait",
    "X's LLM judge scores every reply to someone else's thread from 0 to 3; a 0 down-ranks it. An account-level AI-slop label hides all your posts from non-followers for 30 days",
    "One reply per stranger's conversation. Don't reply to your own reply under someone else's post: X runs a separate spam check on those chains",
    "Quote or reply: both weigh 5. A quote goes to your followers' feeds and can win the cold-start slot; a reply reaches the people reading that thread",
    "Answer people who reply to you within the first hour: `engage inbox <your post id>`",
    "If a reply fails, never auto-thread or split it: give the user the text to post themselves",
];

const POSTING: &[&str] = &[
    "Front-load the hook: the first line decides whether people stop. Posts over 280 chars (Premium) show their first 280 as the preview",
    "Write for sends and saves: a copy-link share weighs 20 and a DM share 5, against 0.6 for a like",
    "Space originals: your 2nd and 3rd unseen post in a viewer's feed score x0.625 and x0.44. Don't post a new original while the last one is in its first 2 h",
    "Avoid what people mute or report (-58.8, -234): insults, rage bait, spammy claims",
    "Keep threads short, or split strong ideas into standalone posts",
    "Links are fine in the post body; avoid shorteners. A link via the paid API costs $0.20, via the web session nothing",
    "`post \"text\" --dry-run` returns preflight score, JEV flags, X-weighted length, route and cost without posting",
    "`suggest best-time` picks hours from your own history; `schedule add \"text\" --at auto` uses it",
    "Articles are not long posts: `article preview`, `article draft` (web session; --api for the official endpoint), `article publish` makes it public, only with the user's yes",
    "Original posts, DMs, article publish and deletes need the user's explicit approval",
];

const GROWTH: &[&str] = &[
    "Small accounts grow by replying early to bigger accounts in their niche: replies are the only way to reach non-followers without For You",
    "`engage feed [topics]` finds fresh posts worth a reply (with JEV: the best 5, each with a why). With no topics it uses niche.topics",
    "`engage watchlist add <handle>` pins accounts without following; engage feed checks them first",
    "After replying, `track run` records reply-backs and promotes accounts where your replies did well; `engage hot-targets` ranks them",
    "Mutual follows matter: your original posts get reply weight 20 instead of 5 from mutuals. Follow back people who engage with you",
    "`engage swarm <id>` lists smaller accounts replying under a big post, for peer-to-peer replies",
    "Your own originals need early likes in their first 2 h to win the cold-start slot (accounts up to 50k followers)",
];

const METRICS: &[&str] = &[
    "`metrics ID1 ID2 ...` reads up to 100 posts in one call with age, delta since the last snapshot and velocity; never loop per ID",
    "`track run` snapshots recent posts and checks pending reply-backs in one call; `track status`, `report daily|weekly` summarise",
    "`timeline --user <handle> --sort impressions` finds top posts; `--since 24h` shows recent ones",
    "`engage inbox <id>` covers replies, quote posts and replies under quotes; with JEV it triages them",
    "Every read feeds a local library: `inspire --topic X` (or --long) browses it",
];

const AVOID: &[&str] = &[
    "Engagement-bait wording (like if, RT to win, follow for follow, comment X and I'll DM you) hides the post from non-followers, with no reputation exemption",
    "Link shorteners or low-quality redirects hide the post from non-followers; a pinned post with a bad link labels the whole account for 7 days",
    "Near-duplicate text across replies is grouped as copy-paste spam",
    "2 or more @mentions in a post trigger X's real-time spam check",
    "Second-person insults hide the post from non-followers",
    "Replying under a spam- or NSFW-labelled post: your reply is dropped with it",
    "Under the Hood (x.com/i/jf/under_the_hood) lists most labels on your posts, but not the reply down-rank or copy-paste labels, and you never see your own hidden replies",
    "If X shows a captcha, lock or 'automated' warning (code 226), stop all writes and ask the user to clear it on x.com",
];

fn render_tips(format: OutputFormat, topic: &str) {
    output::render(format, &tips_for(topic), None);
}

fn tips_for(topic: &str) -> Tips {
    let all = topic == "all";
    let want = |t: &str| all || topic == t;
    Tips {
        algorithm: want("algorithm").then_some(Algorithm {
            source: "xai-org/x-algorithm: home-mixer/params/param.rs (production values, last sync 2026-10-12) and vm-ranker/params.rs",
            score: "score = sum of weight x the viewer's predicted probability of each action",
            weights: WEIGHTS,
            facts: ALGORITHM_FACTS,
        }),
        replies: want("replies").then_some(REPLIES),
        posting: want("posting").then_some(POSTING),
        growth: want("growth").then_some(GROWTH),
        metrics: want("metrics").then_some(METRICS),
        avoid: want("avoid").then_some(AVOID),
    }
}

impl Tableable for Tips {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Topic", "Tips"]);
        if let Some(a) = &self.algorithm {
            let weights = serde_json::to_value(&a.weights)
                .ok()
                .and_then(|v| v.as_object().cloned())
                .map(|m| m.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            table.add_row(vec!["algorithm".to_string(), format!("{}\n{}\nWeights: {weights}\n{}", a.source, a.score, a.facts.join("\n"))]);
        }
        for (name, lines) in [
            ("replies", self.replies),
            ("posting", self.posting),
            ("growth", self.growth),
            ("metrics", self.metrics),
            ("avoid", self.avoid),
        ] {
            if let Some(lines) = lines {
                table.add_row(vec![name.to_string(), lines.join("\n")]);
            }
        }
        table
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_is_listed() {
        let lines = command_lines();
        let text = lines.join("\n");
        for cmd in crate::cli::Cli::command().get_subcommands().filter(|c| !SKIP_COMMANDS.contains(&c.get_name())) {
            assert!(text.contains(cmd.get_name()), "missing {}", cmd.get_name());
        }
        assert!(lines.iter().any(|l| l.starts_with("like/unlike <id>")), "{text}");
        assert!(lines.iter().any(|l| l.starts_with("reply <id> <text>")), "{text}");
        assert!(lines.iter().any(|l| l.starts_with("engage feed [topics...]")), "{text}");
        assert!(!text.contains("--json") && !text.contains("--help"), "{text}");
    }

    #[test]
    fn each_topic_returns_one_section() {
        for topic in TIP_TOPICS.iter().filter(|t| **t != "all") {
            let v = serde_json::to_value(tips_for(topic)).unwrap();
            let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
            assert_eq!(keys, vec![*topic], "{topic}");
        }
        let all = serde_json::to_value(tips_for("all")).unwrap();
        assert_eq!(all.as_object().unwrap().len(), TIP_TOPICS.len() - 1);
        assert_eq!(all["algorithm"]["weights"]["share_via_copy_link"], 20.0);
    }
}
