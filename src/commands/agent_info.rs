//! `xmaster agent-info`: raw JSON, the same in a terminal and a pipe.
//!
//! The default manifest is small enough to load every session: one line per
//! command, generated from clap so it can't drift from the parser. `--command
//! PATH` returns one command or group in full (arguments, flags, defaults,
//! allowed values); `--tips [topic]` returns strategy and X ranking notes.

use crate::config;
use crate::errors::{XmasterError, EXIT_CODES};
use crate::output;
use clap::{ArgAction, Command, CommandFactory};
use serde::ser::{SerializeMap, Serializer};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::any::TypeId;
use std::collections::BTreeMap;
use std::fmt::Write;

/// Topics accepted by `agent-info --tips <topic>`.
pub const TIP_TOPICS: &[&str] = &["all", "algorithm", "replies", "posting", "growth", "metrics", "avoid"];

/// Tuning thresholds (`--min-*`, `--max-*`) stay out of the one-line form;
/// `--command` lists them.
const SKIP_FLAG_PREFIXES: &[&str] = &["min-", "max-"];
/// Groups listed leaf by leaf in the default manifest; others take one line.
const EXPAND: &[&str] = &["engage"];
/// Commands for people, not agents (`star` opens a browser).
const SKIP_COMMANDS: &[&str] = &["star"];

/// A JSON object that keeps insertion order (clap's: most-used first).
struct Ordered<V>(Vec<(String, V)>);

impl<V: Serialize> Serialize for Ordered<V> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

#[derive(Serialize)]
struct AgentInfo {
    name: &'static str,
    version: &'static str,
    description: &'static str,
    usage: &'static str,
    setup: [&'static str; 4],
    workflows: [&'static str; 5],
    commands: Ordered<String>,
    exit_codes: BTreeMap<String, &'static str>,
    envelope: Envelope,
    cost: Cost,
    pacing: String,
    jev: Jev,
    access: &'static str,
    tips: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    writing_style: Option<&'static str>,
    global_flags: Ordered<Value>,
    config: ConfigInfo,
    auto_json_when_piped: bool,
}

#[derive(Serialize)]
struct Envelope {
    version: &'static str,
    success: &'static str,
    error: &'static str,
}

#[derive(Serialize)]
struct ConfigInfo {
    path: String,
    env_prefix: &'static str,
    env: &'static str,
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

pub fn execute(command: Option<&str>, tips: Option<&str>) -> Result<(), XmasterError> {
    match (command, tips) {
        (_, Some(topic)) => output::json::render_raw(&tips_for(topic)),
        (Some(path), None) => output::json::render_raw(&scoped(path)?),
        (None, None) => output::json::render_raw(&manifest()),
    }
    Ok(())
}

fn manifest() -> AgentInfo {
    let cfg = config::load_config().ok();
    let jev_active = cfg
        .as_ref()
        .is_some_and(|c| c.jev_key().is_some() && !c.settings.disable_jev);
    // The voice can run to a page; point at it rather than inline it.
    let rails = cfg.as_ref().map(|c| c.rails.clone()).unwrap_or_default();
    let writing_style = cfg
        .is_some_and(|c| !c.style.voice.is_empty())
        .then_some("set: `config get style.voice` before drafting");
    let root = root();

    AgentInfo {
        name: "xmaster",
        version: env!("CARGO_PKG_VERSION"),
        description: "X CLI for AI agents: read, post and reply to any thread through the user's browser session (free); draft checks; JEV judgments",
        usage: "<arg> [optional] --flag; flag= takes a value; -n N: how many. Posts: ID or URL; users: handle or URL. agent-info --command PATH: one command in full",
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
        commands: Ordered(listing(&root)),
        exit_codes: EXIT_CODES.iter().map(|(c, m)| (c.to_string(), *m)).collect(),
        envelope: Envelope {
            version: "1",
            success: "stdout: {version,status,data,metadata?}",
            error: "stderr: {version,status,error:{code,message,suggestion,retryable,retry_after_secs?}}. write_unconfirmed: may have landed, check first",
        },
        cost: Cost {
            free: "with web cookies: read, conversation, replies, search, timelines, mentions, profiles, others' metrics; all posts, replies, quotes, likes, reposts, bookmarks, follows, deletes, mutes, blocks",
            paid: "X API ($0.015/post, $0.20 with a link): no web session, polls, DMs, lists, bookmarks, likers, reposters, quotes, followers, users, volume, own private metrics; search-ai, trending: xAI. --dry-run shows route, cost",
        },
        pacing: format!(
            "Writes keep caps and gaps, shared by every session on this machine; a refusal says why and when: \
             wait, don't loop. Caps (config rails.*): replies_per_hour {}, replies_per_day {}, posts_per_day {}; \
             raise one only when the user asks. rails status: counts, recent writes",
            rails.replies_per_hour, rails.replies_per_day, rails.posts_per_day
        ),
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
        global_flags: Ordered(
            root.get_arguments()
                .filter(|a| a.is_global_set())
                .filter_map(|a| Some((format!("--{}", a.get_long()?), json!({ "description": help(a) }))))
                .collect(),
        ),
        config: ConfigInfo {
            path: config::config_path().display().to_string(),
            env_prefix: "XMASTER_",
            env: "XMASTER_SECTION__KEY=value overrides config.toml",
        },
        auto_json_when_piped: true,
    }
}

fn root() -> Command {
    let mut cli = crate::cli::Cli::command();
    cli.build();
    cli
}

/// Every public leaf command by its canonical path ("engage feed"), in
/// clap's order. Hidden commands and clap's `help` are left out.
fn leaves(cmd: &Command) -> Vec<(String, &Command)> {
    fn walk<'a>(cmd: &'a Command, prefix: &str, out: &mut Vec<(String, &'a Command)>) {
        for sub in cmd.get_subcommands().filter(|c| !c.is_hide_set() && c.get_name() != "help") {
            let path = format!("{prefix}{}", sub.get_name());
            if sub.has_subcommands() {
                walk(sub, &format!("{path} "), out);
            } else {
                out.push((path, sub));
            }
        }
    }
    let mut out = Vec::new();
    walk(cmd, "", &mut out);
    out
}

/// Arguments and flags a command lists, without globals and `--help`.
fn own_args(cmd: &Command) -> impl Iterator<Item = &clap::Arg> {
    cmd.get_arguments().filter(|a| {
        !a.is_hide_set()
            && !a.is_global_set()
            && !matches!(a.get_action(), ArgAction::Help | ArgAction::HelpShort | ArgAction::HelpLong | ArgAction::Version)
    })
}

/// The default manifest's `commands`: canonical path -> one line. Groups
/// outside [`EXPAND`] take one line that names their subcommands; every key
/// is a valid `--command` path, which returns the group in full.
fn listing(root: &Command) -> Vec<(String, String)> {
    fn walk(cmd: &Command, prefix: &str, out: &mut Vec<(String, String)>) {
        for sub in cmd.get_subcommands().filter(|c| !c.is_hide_set() && c.get_name() != "help") {
            let path = format!("{prefix}{}", sub.get_name());
            if SKIP_COMMANDS.contains(&path.as_str()) {
                continue;
            }
            if !sub.has_subcommands() {
                out.push((path, line(sub)));
            } else if EXPAND.contains(&path.as_str()) {
                walk(sub, &format!("{path} "), out);
            } else {
                out.push((path, group_line(sub)));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, "", &mut out);
    out
}

/// A leaf: `<id> <text> --media= --dry-run: Reply to a post`.
fn line(cmd: &Command) -> String {
    let mut syntax = String::new();
    for arg in own_args(cmd) {
        let id = arg.get_id().as_str();
        if arg.is_positional() {
            let dots = if matches!(arg.get_action(), ArgAction::Append) { "..." } else { "" };
            let (open, close) = if arg.is_required_set() { ('<', '>') } else { ('[', ']') };
            let _ = write!(syntax, " {open}{id}{dots}{close}");
        } else if id == "count" {
            syntax.push_str(" -n=");
        } else if let Some(long) = arg.get_long().filter(|l| !SKIP_FLAG_PREFIXES.iter().any(|p| l.starts_with(p))) {
            let value = if arg.get_action().takes_values() { "=" } else { "" };
            let _ = write!(syntax, " --{long}{value}");
        }
    }
    // "like <id>" needs no "Like a post"; keep an about that says more.
    let about = about(cmd);
    let words: Vec<&str> = about.split_whitespace().collect();
    let first = words.first().map(|w| w.to_ascii_lowercase()).unwrap_or_default();
    let restates = words.len() <= 3 && (first == cmd.get_name() || cmd.get_visible_aliases().any(|a| a == first));
    let mut line = match (syntax.trim_start(), restates || about.is_empty()) {
        (s, true) => s.to_string(),
        ("", false) => about,
        (s, false) => format!("{s}: {about}"),
    };
    let aliases: Vec<&str> = cmd.get_visible_aliases().collect();
    if !aliases.is_empty() {
        let _ = write!(line, " (alias: {})", aliases.join(", "));
    }
    line.trim_start().to_string()
}

/// A collapsed group: `list|sync|search <query>|folder <id>: Bookmarks ...`.
fn group_line(cmd: &Command) -> String {
    let subs: Vec<String> = cmd
        .get_subcommands()
        .filter(|c| !c.is_hide_set() && c.get_name() != "help")
        .map(|sub| {
            let mut name = sub.get_name().to_string();
            for arg in sub.get_arguments().filter(|a| a.is_positional() && a.is_required_set()) {
                let _ = write!(name, " <{}>", arg.get_id());
            }
            name
        })
        .collect();
    format!("{}: {}", subs.join("|"), about(cmd))
}

/// The about's first clause: "Reply to a post (shorthand ...)" -> "Reply to a post".
fn about(cmd: &Command) -> String {
    let text = cmd.get_about().map(|a| a.to_string()).unwrap_or_default();
    let first = text.lines().next().unwrap_or_default();
    let clause = first.split([':', '(']).next().unwrap_or_default();
    clause.trim().trim_end_matches(['.', ',']).to_string()
}

fn help(arg: &clap::Arg) -> String {
    let text = arg.get_help().map(|h| h.to_string()).unwrap_or_default();
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `agent-info --command PATH`: the matching commands in full. A group
/// selects its leaves; aliases resolve to canonical paths.
fn scoped(path: &str) -> Result<Value, XmasterError> {
    let root = root();
    let mut cmd = &root;
    let mut canonical = Vec::new();
    for word in path.split_whitespace() {
        match cmd.find_subcommand(word).filter(|c| !c.is_hide_set()) {
            Some(sub) => {
                canonical.push(sub.get_name());
                cmd = sub;
            }
            None => return Err(unknown_path(&root, path)),
        }
    }
    if canonical.is_empty() {
        return Err(unknown_path(&root, path));
    }
    let prefix = canonical.join(" ");
    let entries: Vec<(String, Value)> = if cmd.has_subcommands() {
        leaves(cmd).into_iter().map(|(p, c)| (format!("{prefix} {p}"), full(c))).collect()
    } else {
        vec![(prefix, full(cmd))]
    };
    Ok(json!({
        "name": "xmaster",
        "version": env!("CARGO_PKG_VERSION"),
        "commands": serde_json::to_value(Ordered(entries)).unwrap_or_default(),
    }))
}

/// Unknown path: name the closest real paths (groups too), by edit distance.
fn unknown_path(root: &Command, path: &str) -> XmasterError {
    let wanted = path.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase();
    let mut paths: Vec<String> = Vec::new();
    for (leaf, _) in leaves(root) {
        let words: Vec<&str> = leaf.split(' ').collect();
        for n in 1..=words.len() {
            let p = words[..n].join(" ");
            if !paths.contains(&p) {
                paths.push(p);
            }
        }
    }
    let limit = 2.max(wanted.len() / 4);
    let mut close: Vec<(usize, String)> = paths
        .into_iter()
        .map(|p| (distance(&wanted, &p), p))
        .filter(|(d, _)| *d <= limit)
        .collect();
    close.sort();
    let hint = if close.is_empty() {
        "use a key of `xmaster agent-info` commands".to_string()
    } else {
        let names: Vec<String> = close.into_iter().take(3).map(|(_, p)| p).collect();
        format!("did you mean: {}?", names.join(", "))
    };
    XmasterError::InvalidInput(format!("unknown command path '{wanted}'; {hint}"))
}

/// Levenshtein distance between two short strings.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

/// The JSON type an agent should pass, from the arg's value parser.
fn value_type(arg: &clap::Arg) -> &'static str {
    if matches!(arg.get_action(), ArgAction::SetTrue | ArgAction::SetFalse) {
        return "bool";
    }
    let id = arg.get_value_parser().type_id();
    let ints = [TypeId::of::<usize>(), TypeId::of::<u64>(), TypeId::of::<u32>(), TypeId::of::<i64>(), TypeId::of::<i32>()];
    if ints.iter().any(|t| id == *t) {
        "int"
    } else if id == TypeId::of::<f64>() {
        "number"
    } else {
        "string"
    }
}

/// One argument or flag in full.
fn spec(arg: &clap::Arg) -> Value {
    let ty = value_type(arg);
    let mut v = Map::new();
    let name = match arg.get_long() {
        Some(long) if !arg.is_positional() => format!("--{long}"),
        _ => arg.get_id().to_string(),
    };
    v.insert("name".into(), json!(name));
    if let Some(short) = arg.get_short() {
        v.insert("short".into(), json!(format!("-{short}")));
    }
    let mut aliases: Vec<String> = arg.get_visible_aliases().unwrap_or_default().iter().map(|a| format!("--{a}")).collect();
    aliases.extend(arg.get_visible_short_aliases().unwrap_or_default().iter().map(|a| format!("-{a}")));
    if !aliases.is_empty() {
        v.insert("aliases".into(), json!(aliases));
    }
    v.insert("type".into(), json!(ty));
    v.insert("required".into(), json!(arg.is_required_set()));
    if matches!(arg.get_action(), ArgAction::Append) {
        v.insert("multiple".into(), json!(true));
    }
    if let Some(default) = arg.get_default_values().first().map(|d| d.to_string_lossy()) {
        let value = match ty {
            "bool" => json!(default == "true"),
            "int" => default.parse::<i64>().map(|n| json!(n)).unwrap_or(json!(default)),
            "number" => default.parse::<f64>().map(|n| json!(n)).unwrap_or(json!(default)),
            _ => json!(default),
        };
        // A switch that defaults to off says nothing.
        if value != json!(false) {
            v.insert("default".into(), value);
        }
    }
    if ty != "bool" {
        if let Some(values) = arg.get_value_parser().possible_values() {
            let values: Vec<String> = values.filter(|p| !p.is_hide_set()).map(|p| p.get_name().to_string()).collect();
            v.insert("values".into(), json!(values));
        }
    }
    let description = help(arg);
    if !description.is_empty() {
        v.insert("description".into(), json!(description));
    }
    Value::Object(v)
}

/// One command in full, generated from its clap definition.
fn full(cmd: &Command) -> Value {
    let (args, options): (Vec<&clap::Arg>, Vec<&clap::Arg>) = own_args(cmd).partition(|a| a.is_positional());
    let mut entry = Map::new();
    entry.insert("description".into(), json!(cmd.get_about().map(|a| a.to_string()).unwrap_or_default()));
    let aliases: Vec<&str> = cmd.get_visible_aliases().collect();
    if !aliases.is_empty() {
        entry.insert("aliases".into(), json!(aliases));
    }
    // A long doc comment's later paragraphs: when to use it, fallbacks.
    if let Some(long) = cmd.get_long_about().map(|l| l.to_string()) {
        let details = long.split_once("\n\n").map(|(_, rest)| rest.trim()).unwrap_or_default();
        if !details.is_empty() {
            entry.insert("details".into(), json!(details));
        }
    }
    entry.insert("args".into(), args.into_iter().map(spec).collect());
    entry.insert("options".into(), options.into_iter().map(spec).collect());
    Value::Object(entry)
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
    "Peer-sized threads: name one specific detail from the post (a claim, number, method or result) and ask one question the author can answer. An OP who likes or answers your reply lifts its place under the post. Image replies on big threads: the image carries the reply; a question is optional",
    "Image reply on a big live thread: `search \"<story keywords> lang:en -filter:replies\" --mode top --since 6h -n 40 --sort velocity` ranks posts by imps_per_min (recent mode spans minutes on a busy story; top spans hours); read the pick with `conversation <id>`, then `reply <id> \"text\" --media img.png`",
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

#[cfg(test)]
mod tests {
    use super::*;

    use clap::Parser;

    fn commands() -> Map<String, Value> {
        serde_json::to_value(manifest()).unwrap()["commands"].as_object().unwrap().clone()
    }

    /// Both directions: every public leaf is listed or sits under a listed
    /// group, and every key is a command clap parses and `--command` accepts.
    #[test]
    fn manifest_covers_every_public_command() {
        let listed = commands();
        for (leaf, _) in leaves(&root()) {
            let covered = SKIP_COMMANDS.contains(&leaf.as_str())
                || listed.keys().any(|k| leaf == *k || leaf.starts_with(&format!("{k} ")));
            assert!(covered, "{leaf} missing");
        }
        for path in listed.keys() {
            let argv = std::iter::once("xmaster").chain(path.split(' ')).chain(["--help"]);
            let err = crate::cli::Cli::try_parse_from(argv).err().expect("--help exits through clap");
            assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp, "{path}");
            assert!(scoped(path).is_ok(), "{path}");
        }
        assert!(!listed.contains_key("contract") && !listed.contains_key("help") && !listed.contains_key("star"));
        assert_eq!(listed["reply"], "<id> <text> --media= --dry-run: Reply to a post");
        assert_eq!(listed["retweet"], "<id> (alias: repost)");
        assert_eq!(listed["me"], "Your own profile");
        assert!(listed["engage feed"].as_str().unwrap().starts_with("[topics...] -n=: "), "{}", listed["engage feed"]);
        assert!(listed["bookmarks"].as_str().unwrap().starts_with("list|sync|search <query>|"), "{}", listed["bookmarks"]);
        let text = Value::Object(listed).to_string();
        assert!(!text.contains("--json") && !text.contains("--help") && !text.contains("--min-"), "{text}");
    }

    #[test]
    fn scoped_paths_return_full_specs() {
        let v = scoped("timeline").unwrap();
        assert_eq!(v.as_object().unwrap().keys().collect::<Vec<_>>(), ["commands", "name", "version"]);
        let count = v["commands"]["timeline"]["options"].as_array().unwrap().iter().find(|o| o["name"] == "--count").unwrap().clone();
        assert_eq!(count, json!({"name":"--count","short":"-c","aliases":["--limit","-n"],"type":"int","required":false,"default":10,"description":"Number of posts"}));
        let sort = &v["commands"]["timeline"]["options"].as_array().unwrap().iter().find(|o| o["name"] == "--sort").unwrap()["values"];
        assert!(sort.as_array().unwrap().contains(&json!("impressions")));
        // A group selects its leaves; aliases and extra spaces resolve.
        let engage = scoped("engage").unwrap();
        assert!(engage["commands"]["engage feed"]["options"].is_array());
        assert!(engage["commands"]["engage watchlist add"]["args"][0]["required"] == json!(true));
        assert!(scoped("  repost ").unwrap()["commands"]["retweet"]["aliases"] == json!(["repost"]));
        assert!(scoped("engage feed").unwrap()["commands"]["engage feed"]["details"].as_str().unwrap().contains("niche.topics"));
        for bad in ["", "enage feed", "contract", "timeline extra"] {
            let e = scoped(bad).unwrap_err();
            assert_eq!(e.exit_code(), 3, "{bad}");
        }
        assert!(scoped("enage").unwrap_err().to_string().contains("did you mean: engage?"));
        assert!(scoped("enage feed").unwrap_err().to_string().contains("did you mean: engage feed"));
        assert_eq!(distance("timelin", "timeline"), 1);
    }

    #[test]
    fn every_count_flag_takes_n_and_limit() {
        let root = root();
        let mut seen = 0;
        for (path, cmd) in leaves(&root) {
            if let Some(arg) = cmd.get_arguments().find(|a| a.get_id() == "count") {
                seen += 1;
                assert_eq!(arg.get_short(), Some('c'), "{path}");
                assert_eq!(arg.get_visible_short_aliases(), Some(vec!['n']), "{path}");
                assert_eq!(arg.get_visible_aliases(), Some(vec!["limit"]), "{path}");
                for flag in ["-n", "--limit", "-c", "--count"] {
                    let mut argv: Vec<&str> = std::iter::once("xmaster").chain(path.split(' ')).collect();
                    // Fill required positionals with a value each parser accepts.
                    argv.extend(cmd.get_arguments().filter(|a| a.is_positional() && a.is_required_set()).map(|_| "123"));
                    argv.extend([flag, "3"]);
                    crate::cli::Cli::try_parse_from(&argv).unwrap_or_else(|e| panic!("{argv:?}: {e}"));
                }
            }
        }
        assert!(seen >= 25, "{seen}");
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

    #[test]
    fn reply_tips_cover_the_image_reply_method() {
        let tips = serde_json::to_value(tips_for("replies")).unwrap()["replies"].to_string();
        for part in ["-filter:replies\\\" --mode top --since 6h -n 40 --sort velocity", "conversation <id>", "--media img.png", "a question is optional"] {
            assert!(tips.contains(part), "missing {part}: {tips}");
        }
    }
}
