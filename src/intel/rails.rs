//! Pacing rails: one gate before every X write, so agent activity looks like
//! a person's and stops at the first sign of an X challenge.
//!
//! Why: web-session writes go out as X's official web client, which X's bot
//! model and reply-spam judge score on bursts under 1 s, regular timing,
//! same-author streaks, engagement without a prior read, near-duplicate text
//! and repeat replies in a stranger's thread (xai-org/x-algorithm botspam
//! fork: `bdsm/rust/abuse-v3-features/src/derived.rs`, `grox/flows/reply_spam/`,
//! `BBQDuplicateTextRepliesProd.bot`). The real thresholds ship redacted, so
//! the [`Budget`] defaults are safety margins.
//!
//! Flow: [`gate`] reads the target for free (FxTwitter), plans the earliest
//! plausible send time, reserves it in SQLite (parallel agents queue instead
//! of bursting), sleeps when the wait is short and returns a [`Permit`]. After
//! the write, [`Permit::record`] stores the outcome: lock or challenge codes
//! trip a 24 h cooldown, limit codes park that action type, and a new post
//! that doesn't read back is flagged as a possible silent fail.
//!
//! Wiring: wrap each XApi write body in [`guard`], e.g.
//! `rails::guard(&self.ctx, WriteIntent::new(WriteKind::Like).target(tweet_id), async { .. }).await`.
//! Kill switch: `XMASTER_RAILS=off`.

use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::providers::xapi::{TweetData, TweetResponse};
use rand::Rng;
use rusqlite::{params, Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

const DAY: f64 = 86_400.0;
const HOUR: f64 = 3_600.0;
/// X's bot model counts actions less than 1000 ms apart as a burst.
const BURST_SECS: f64 = 1.0;
/// Ancestors walked up from a reply target to find the conversation root.
const MAX_HOPS: usize = 6;
/// A reservation never recorded this long after its slot was abandoned.
const ABANDONED_SECS: f64 = 600.0;
/// All writes pause this long after X serves a challenge page.
const CHALLENGE_SECS: f64 = 1_800.0;
/// Park an action this long when X rate-limits it without a reset time.
const PARK_SECS: f64 = 900.0;
/// Floors for writes X's models don't score as engagement.
const LIGHT_GAP_SECS: f64 = 3.0;
const THREAD_GAP_SECS: f64 = 2.0;
/// Ledger rows older than this are pruned.
const KEEP_DAYS: f64 = 60.0;

// ---------------------------------------------------------------------------
// Budget
// ---------------------------------------------------------------------------

/// Write budget and pacing (`[rails]` in config.toml). Caps count a rolling
/// 24 h (or 1 h) window; 0 turns a cap off. Defaults are margins for a small
/// account writing through the web session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Budget {
    /// Waits up to this long are slept through; longer ones return
    /// `budget_exceeded` with `retry_after_secs`.
    pub max_auto_wait_secs: u64,
    /// Floor between any two writes; log-normal jitter is added on top.
    pub min_gap_secs: u64,
    /// Floor between two replies in other people's threads.
    pub reply_gap_secs: u64,
    /// Log-normal sigma of the jitter (1.1 gives gaps a CV of about 0.6).
    pub jitter_sigma: f64,
    /// Don't reply to or quote a post younger than this.
    pub min_target_age_secs: u64,
    /// Simulated read time before engaging with a post, scaled to its length.
    pub read_min_secs: u64,
    pub read_max_secs: u64,
    pub posts_per_day: u32,
    pub replies_per_day: u32,
    pub replies_per_hour: u32,
    /// Replies inside your own threads (to people who replied to you).
    pub own_replies_per_day: u32,
    pub quotes_per_day: u32,
    pub likes_per_day: u32,
    pub reposts_per_day: u32,
    pub follows_per_day: u32,
    pub unfollows_per_day: u32,
    pub dms_per_day: u32,
    /// Replies per other person's conversation per 24 h.
    pub replies_per_conversation: u32,
    /// Most consecutive actions on one author.
    pub same_author_streak: u32,
    /// Never unfollow an account sooner than this after following it.
    pub unfollow_after_days: u32,
    /// Block text whose word overlap (unigram Jaccard) with one of your own
    /// posts or replies from the last `duplicate_days` reaches this.
    pub duplicate_jaccard: f64,
    pub duplicate_days: u32,
    /// After an X lock or challenge: no writes for this long, then half caps.
    pub cooldown_hours: u32,
    pub half_caps_days: u32,
    /// Read each new post back through FxTwitter to catch silent fails.
    pub verify_writes: bool,
    /// Before a like, reply, quote or repost, send the view events x.com
    /// sends when a post renders and stays on screen (web session only).
    pub view_events: bool,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_auto_wait_secs: 240,
            min_gap_secs: 45,
            reply_gap_secs: 180,
            jitter_sigma: 1.1,
            min_target_age_secs: 90,
            read_min_secs: 8,
            read_max_secs: 40,
            posts_per_day: 10,
            replies_per_day: 20,
            replies_per_hour: 5,
            own_replies_per_day: 50,
            quotes_per_day: 5,
            likes_per_day: 60,
            reposts_per_day: 10,
            follows_per_day: 15,
            unfollows_per_day: 15,
            dms_per_day: 20,
            replies_per_conversation: 1,
            same_author_streak: 2,
            unfollow_after_days: 7,
            duplicate_jaccard: 0.5,
            duplicate_days: 30,
            cooldown_hours: 24,
            half_caps_days: 7,
            verify_writes: true,
            view_events: true,
        }
    }
}

/// The active budget. Lead: return `ctx.config.rails.clone()` once `[rails]`
/// is in AppConfig.
pub fn budget(_ctx: &AppContext) -> Budget {
    Budget::default()
}

// ---------------------------------------------------------------------------
// Intents
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteKind {
    Post,
    Reply,
    Quote,
    Like,
    Unlike,
    Repost,
    Unrepost,
    Bookmark,
    Unbookmark,
    Follow,
    Unfollow,
    Delete,
    Dm,
}

/// One write an agent wants to make.
#[derive(Debug, Clone)]
pub struct WriteIntent {
    pub kind: WriteKind,
    /// The post replied to, quoted, liked, reposted, bookmarked or deleted.
    pub target_id: Option<String>,
    /// The account followed, unfollowed or messaged (handle or user ID).
    pub target_handle: Option<String>,
    pub text: Option<String>,
}

impl WriteIntent {
    pub fn new(kind: WriteKind) -> Self {
        Self { kind, target_id: None, target_handle: None, text: None }
    }

    pub fn target(mut self, post_id: &str) -> Self {
        self.target_id = Some(post_id.to_string());
        self
    }

    pub fn handle(mut self, handle: &str) -> Self {
        self.target_handle = Some(handle.trim_start_matches('@').to_lowercase());
        self
    }

    pub fn text(mut self, text: &str) -> Self {
        self.text = Some(text.to_string());
        self
    }
}

/// What the ledger stores: the intent's kind, with replies split by where
/// they land (a stranger's thread, your own thread, or your own thread's tail).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Act {
    Post,
    Reply,
    OwnReply,
    Thread,
    Quote,
    Like,
    Unlike,
    Repost,
    Unrepost,
    Bookmark,
    Unbookmark,
    Follow,
    Unfollow,
    Delete,
    Dm,
}

const ACTS: [Act; 15] = [
    Act::Post,
    Act::Reply,
    Act::OwnReply,
    Act::Thread,
    Act::Quote,
    Act::Like,
    Act::Unlike,
    Act::Repost,
    Act::Unrepost,
    Act::Bookmark,
    Act::Unbookmark,
    Act::Follow,
    Act::Unfollow,
    Act::Delete,
    Act::Dm,
];

impl Act {
    fn as_str(self) -> &'static str {
        match self {
            Act::Post => "post",
            Act::Reply => "reply",
            Act::OwnReply => "own_reply",
            Act::Thread => "thread",
            Act::Quote => "quote",
            Act::Like => "like",
            Act::Unlike => "unlike",
            Act::Repost => "repost",
            Act::Unrepost => "unrepost",
            Act::Bookmark => "bookmark",
            Act::Unbookmark => "unbookmark",
            Act::Follow => "follow",
            Act::Unfollow => "unfollow",
            Act::Delete => "delete",
            Act::Dm => "dm",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        ACTS.into_iter().find(|a| a.as_str() == s)
    }

    fn of(kind: WriteKind) -> Self {
        match kind {
            WriteKind::Post => Act::Post,
            WriteKind::Reply => Act::Reply,
            WriteKind::Quote => Act::Quote,
            WriteKind::Like => Act::Like,
            WriteKind::Unlike => Act::Unlike,
            WriteKind::Repost => Act::Repost,
            WriteKind::Unrepost => Act::Unrepost,
            WriteKind::Bookmark => Act::Bookmark,
            WriteKind::Unbookmark => Act::Unbookmark,
            WriteKind::Follow => Act::Follow,
            WriteKind::Unfollow => Act::Unfollow,
            WriteKind::Delete => Act::Delete,
            WriteKind::Dm => Act::Dm,
        }
    }

    /// Creates a public post (checked for duplicates, read back afterwards).
    fn creates(self) -> bool {
        matches!(self, Act::Post | Act::Reply | Act::OwnReply | Act::Thread | Act::Quote)
    }

    /// Engages with someone's post, so it gets read first.
    fn reads(self) -> bool {
        matches!(self, Act::Reply | Act::OwnReply | Act::Quote | Act::Like | Act::Repost)
    }

    /// Private writes the bot model doesn't score: a short gap is enough.
    fn light(self) -> bool {
        matches!(self, Act::Bookmark | Act::Unbookmark)
    }

    /// Writes X rate-limits together.
    fn family(self) -> &'static str {
        match self {
            Act::Post | Act::Reply | Act::OwnReply | Act::Thread | Act::Quote => "posting",
            Act::Like | Act::Unlike => "likes",
            Act::Repost | Act::Unrepost => "reposts",
            Act::Bookmark | Act::Unbookmark => "bookmarks",
            Act::Follow | Act::Unfollow => "follows",
            Act::Delete => "deletes",
            Act::Dm => "dms",
        }
    }

    /// (cap, window, label) pairs; a cap of 0 is off.
    fn caps(self, b: &Budget) -> Vec<(u32, f64, &'static str)> {
        match self {
            Act::Post => vec![(b.posts_per_day, DAY, "posts per day")],
            Act::Reply => vec![
                (b.replies_per_day, DAY, "replies per day"),
                (b.replies_per_hour, HOUR, "replies per hour"),
            ],
            Act::OwnReply => vec![(b.own_replies_per_day, DAY, "replies in your threads per day")],
            Act::Quote => vec![(b.quotes_per_day, DAY, "quotes per day")],
            Act::Like => vec![(b.likes_per_day, DAY, "likes per day")],
            Act::Repost => vec![(b.reposts_per_day, DAY, "reposts per day")],
            Act::Follow => vec![(b.follows_per_day, DAY, "follows per day")],
            Act::Unfollow => vec![(b.unfollows_per_day, DAY, "unfollows per day")],
            Act::Dm => vec![(b.dms_per_day, DAY, "DMs per day")],
            _ => vec![],
        }
        .into_iter()
        .filter(|(cap, ..)| *cap > 0)
        .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Pending,
    Ok,
    /// X may have accepted it (timeout after send, 5xx).
    Unconfirmed,
    Failed,
    SilentFail,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Ok => "ok",
            Status::Unconfirmed => "unconfirmed",
            Status::Failed => "failed",
            Status::SilentFail => "possible_silent_fail",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "ok" => Status::Ok,
            "unconfirmed" => Status::Unconfirmed,
            "failed" => Status::Failed,
            "possible_silent_fail" => Status::SilentFail,
            _ => Status::Pending,
        }
    }
}

// ---------------------------------------------------------------------------
// State snapshot
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Row {
    at: f64,
    act: Act,
    status: Status,
    target_id: Option<String>,
    author: Option<String>,
    conversation: Option<String>,
    new_id: Option<String>,
    text: Option<String>,
    /// Sampled gap the next ordinary write must leave after this one.
    next_gap: f64,
    /// Sampled gap the next stranger reply must leave (replies only).
    reply_gap: Option<f64>,
}

impl Row {
    /// Did (or may) this write reach X? Only those count against caps and rules.
    fn landed(&self, now: f64) -> bool {
        match self.status {
            Status::Ok | Status::Unconfirmed | Status::SilentFail => true,
            Status::Pending => self.at > now - ABANDONED_SECS,
            Status::Failed => false,
        }
    }
}

/// A post from the tracker's `posts` table (history from before the rails).
#[derive(Debug, Clone)]
struct OwnPost {
    id: String,
    kind: String,
    text: String,
}

#[derive(Debug, Default)]
struct Snapshot {
    /// Ledger rows, oldest first.
    rows: Vec<Row>,
    posts: Vec<OwnPost>,
    kv: HashMap<String, String>,
}

/// One of our own writes, found by post ID.
enum Parent {
    Ledger(Act, Option<String>),
    Posted(String),
}

impl Snapshot {
    fn kv_f(&self, key: &str) -> f64 {
        self.kv.get(key).and_then(|v| v.parse().ok()).unwrap_or(0.0)
    }

    fn landed(&self, now: f64) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(move |r| r.landed(now))
    }

    fn last(&self) -> Option<&Row> {
        self.rows.iter().max_by(|a, b| a.at.total_cmp(&b.at))
    }

    fn half_caps(&self, now: f64) -> bool {
        self.kv_f("half_caps_until") > now
    }

    /// Our own post with this ID, from the ledger or the tracker.
    fn parent(&self, id: &str, now: f64) -> Option<Parent> {
        if let Some(r) = self.landed(now).find(|r| r.new_id.as_deref() == Some(id)) {
            return Some(Parent::Ledger(r.act, r.conversation.clone()));
        }
        self.posts.iter().find(|p| p.id == id).map(|p| Parent::Posted(p.kind.clone()))
    }

    /// A conversation this post is already known to belong to: (root, ours?).
    fn conversation_of(&self, id: &str, now: f64) -> Option<(String, bool)> {
        for r in self.landed(now) {
            let ours = matches!(r.act, Act::Post | Act::Quote | Act::Thread | Act::OwnReply);
            if r.new_id.as_deref() == Some(id) {
                let root = r.conversation.clone().unwrap_or_else(|| id.to_string());
                return Some((root, ours || r.act == Act::Post));
            }
            if r.target_id.as_deref() == Some(id) && matches!(r.act, Act::Reply | Act::OwnReply) {
                if let Some(c) = &r.conversation {
                    return Some((c.clone(), r.act == Act::OwnReply));
                }
            }
        }
        self.posts
            .iter()
            .find(|p| p.id == id && p.kind != "reply")
            .map(|p| (p.id.clone(), true))
    }
}

/// What the gate learned about the target.
#[derive(Debug, Clone, Default)]
struct Target {
    /// Lowercase handle.
    author: Option<String>,
    created_at: Option<f64>,
    chars: Option<usize>,
    /// Root post of the conversation (replies only).
    conversation: Option<String>,
}

/// A post on the way from a reply target up to its conversation root.
#[derive(Debug, Clone)]
struct Node {
    id: String,
    author: Option<String>,
}

// ---------------------------------------------------------------------------
// Planning (pure: state + clock + rng in, decision out)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Plan {
    go_at: f64,
    why: String,
    next_gap: f64,
    reply_gap: Option<f64>,
}

/// The latest of several "not before" times, with the reason for it.
struct Bound {
    at: f64,
    why: String,
}

impl Bound {
    fn raise(&mut self, at: f64, why: impl FnOnce() -> String) {
        if at > self.at {
            self.at = at;
            self.why = why();
        }
    }
}

fn refuse(code: &'static str, message: String) -> XmasterError {
    XmasterError::Api { provider: "rails", code, message }
}

/// `budget_exceeded`: the write is allowed later, at a known time.
fn later(why: &str, wait: f64, now: f64) -> XmasterError {
    let secs = wait.ceil().max(1.0) as u64;
    refuse(
        "budget_exceeded",
        format!(
            "Not sent: {why}. Next allowed at {} (retry_after_secs={secs}). \
             Nothing was posted; rerun the same command then, or do something else meanwhile",
            fmt_ts(now + secs as f64)
        ),
    )
}

/// Refusals that no wait fixes: pause, cooldown, silent fail.
fn check_locks(snap: &Snapshot, now: f64) -> Result<(), XmasterError> {
    let paused = snap.kv_f("paused_until");
    if paused > now {
        let until = if paused > now + 365.0 * DAY { "you resume".to_string() } else { fmt_ts(paused) };
        return Err(refuse(
            "rails_paused",
            format!("Writes are paused until {until}. Resume with: xmaster rails resume"),
        ));
    }
    let cooldown = snap.kv_f("cooldown_until");
    if cooldown > now {
        let reason = snap.kv.get("cooldown_reason").map(String::as_str).unwrap_or("a challenge");
        return Err(refuse(
            "account_cooldown",
            format!(
                "X challenged this account ({reason}). All writes are stopped until {}. \
                 Open x.com in your browser and clear any lock, captcha or verification, \
                 then run: xmaster rails reset-cooldown --yes. Do not retry writes before that",
                fmt_ts(cooldown)
            ),
        ));
    }
    let challenge = snap.kv_f("challenge_until");
    if challenge > now {
        return Err(refuse(
            "account_cooldown",
            format!(
                "X served a challenge page instead of data. Writes pause until {}. \
                 Open x.com in Chrome to clear it; don't retry before then",
                fmt_ts(challenge)
            ),
        ));
    }
    Ok(())
}

fn effective_cap(cap: u32, half: bool) -> u32 {
    if half {
        (cap / 2).max(1)
    } else {
        cap
    }
}

/// Earliest time any write of `act` may go: X parks, caps, gaps, bursts.
/// `light_gap` / `thread_gap` are the floors for those classes of write.
fn pacing_bound(b: &Budget, snap: &Snapshot, act: Act, now: f64, light_gap: f64, thread_gap: f64) -> Bound {
    let mut bound = Bound { at: now, why: String::new() };

    let parked = snap.kv_f(&format!("blocked_until:{}", act.family()));
    bound.raise(parked, || format!("X rate-limited {} writes", act.family()));

    let half = snap.half_caps(now);
    for (cap, window, label) in act.caps(b) {
        let cap = effective_cap(cap, half);
        let mut ats: Vec<f64> = snap
            .landed(now)
            .filter(|r| r.act == act && r.at > now - window)
            .map(|r| r.at)
            .collect();
        ats.sort_by(f64::total_cmp);
        if ats.len() >= cap as usize {
            let free_at = ats[ats.len() - cap as usize] + window;
            let halved = if half { " (halved after an X challenge)" } else { "" };
            bound.raise(free_at, || format!("cap of {cap} {label} reached{halved}"));
        }
    }

    if let Some(last) = snap.last() {
        bound.raise(last.at + BURST_SECS, || "writes less than 1 s apart look automated".into());
        let (gap, what) = if act == Act::Thread {
            (thread_gap, "thread posts")
        } else if act.light() {
            (light_gap, "writes")
        } else {
            (last.next_gap, "writes")
        };
        bound.raise(last.at + gap, || format!("people leave time between {what} (~{gap:.0}s gap)"));
    }

    if act == Act::Reply {
        let last_reply = snap.rows.iter().filter(|r| r.act == Act::Reply).max_by(|a, z| a.at.total_cmp(&z.at));
        if let Some(r) = last_reply {
            let gap = r.reply_gap.unwrap_or(b.reply_gap_secs as f64);
            bound.raise(r.at + gap, || format!("replies in other people's threads need a ~{gap:.0}s gap"));
        }
    }
    bound
}

#[allow(clippy::too_many_arguments)]
fn plan(
    b: &Budget,
    snap: &Snapshot,
    act: Act,
    text: Option<&str>,
    target: &Target,
    own: Option<&str>,
    now: f64,
    rng: &mut impl Rng,
) -> Result<Plan, XmasterError> {
    check_locks(snap, now)?;

    if act.creates() {
        if let Some(text) = text {
            check_duplicate(b, snap, text, now)?;
        }
    }
    let author = target.author.as_deref().filter(|a| Some(*a) != own);
    if let Some(author) = author {
        check_streak(b, snap, author, now)?;
    }

    let light_gap = jittered(LIGHT_GAP_SECS, b.jitter_sigma, rng);
    let thread_gap = jittered(THREAD_GAP_SECS, b.jitter_sigma, rng);
    let mut bound = pacing_bound(b, snap, act, now, light_gap, thread_gap);

    if act == Act::Reply && b.replies_per_conversation > 0 {
        if let Some(conv) = target.conversation.as_deref() {
            let mut ats: Vec<f64> = snap
                .landed(now)
                .filter(|r| r.act == Act::Reply && r.conversation.as_deref() == Some(conv) && r.at > now - DAY)
                .map(|r| r.at)
                .collect();
            ats.sort_by(f64::total_cmp);
            let cap = b.replies_per_conversation as usize;
            if ats.len() >= cap {
                bound.raise(ats[ats.len() - cap] + DAY, || {
                    "you already replied in this conversation in the last 24 h \
                     (X checks repeat replies in a stranger's thread)"
                        .into()
                });
            }
        }
    }

    if act == Act::Unfollow && b.unfollow_after_days > 0 {
        if let Some(h) = target.author.as_deref() {
            let window = b.unfollow_after_days as f64 * DAY;
            let followed = snap
                .landed(now)
                .filter(|r| r.act == Act::Follow && r.author.as_deref() == Some(h) && r.at > now - window)
                .map(|r| r.at)
                .fold(f64::MIN, f64::max);
            bound.raise(followed + window, || {
                format!(
                    "you followed @{h} less than {} days ago (follow/unfollow churn is a bot signal)",
                    b.unfollow_after_days
                )
            });
        }
    }

    if matches!(act, Act::Reply | Act::OwnReply | Act::Quote) {
        if let Some(created) = target.created_at {
            let settle = b.min_target_age_secs as f64 + rng.gen_range(0.0..20.0);
            bound.raise(created + settle, || {
                format!("the post is only {:.0}s old; instant replies look automated", (now - created).max(0.0))
            });
        }
    }

    let pacing_wait = bound.at - now;
    if pacing_wait > b.max_auto_wait_secs as f64 {
        return Err(later(&bound.why, pacing_wait, now));
    }

    let mut go_at = bound.at;
    let mut why = bound.why;
    if act.reads() {
        let read = read_secs(b, target.chars.unwrap_or(140), rng);
        if now + read > go_at {
            go_at = now + read;
            why = "reading the post first".into();
        }
    }

    Ok(Plan {
        go_at,
        why,
        next_gap: jittered(b.min_gap_secs as f64, b.jitter_sigma, rng),
        reply_gap: (act == Act::Reply).then(|| jittered(b.reply_gap_secs as f64, b.jitter_sigma, rng)),
    })
}

fn check_streak(b: &Budget, snap: &Snapshot, author: &str, now: f64) -> Result<(), XmasterError> {
    let n = b.same_author_streak as usize;
    if n == 0 {
        return Ok(());
    }
    let mut recent: Vec<&Row> = snap.landed(now).collect();
    recent.sort_by(|a, z| z.at.total_cmp(&a.at));
    if recent.len() >= n && recent[..n].iter().all(|r| r.author.as_deref() == Some(author)) {
        return Err(refuse(
            "rails_blocked",
            format!(
                "Not sent: your last {n} actions were all on @{author}. Act on someone else first \
                 (X's bot model scores same-author streaks)"
            ),
        ));
    }
    Ok(())
}

fn check_duplicate(b: &Budget, snap: &Snapshot, text: &str, now: f64) -> Result<(), XmasterError> {
    let since = now - b.duplicate_days as f64 * DAY;
    let mine = words(text);
    let ledger = snap
        .landed(now)
        .filter(|r| r.act.creates() && r.at >= since)
        .filter_map(|r| Some((r.new_id.clone().unwrap_or_else(|| format!("from {}", fmt_ts(r.at))), r.text.as_deref()?)));
    let posts = snap.posts.iter().map(|p| (p.id.clone(), p.text.as_str()));
    for (id, old) in ledger.chain(posts) {
        let theirs = words(old);
        let overlap = if mine.is_empty() && theirs.is_empty() {
            if text.trim() == old.trim() { 1.0 } else { 0.0 }
        } else {
            jaccard(&mine, &theirs)
        };
        if overlap >= b.duplicate_jaccard {
            return Err(refuse(
                "rails_blocked",
                format!(
                    "Not sent: near-duplicate of your post {id} ({overlap:.2} word overlap, limit {:.2}). \
                     X groups copy-paste replies as spam (COPYPASTA_SPAM): reword it",
                    b.duplicate_jaccard
                ),
            ));
        }
    }
    Ok(())
}

/// Lowercase word set, without URLs and @mentions.
fn words(text: &str) -> HashSet<String> {
    text.split_whitespace()
        .filter(|w| !w.starts_with('@') && !w.starts_with("http://") && !w.starts_with("https://"))
        .flat_map(|w| w.split(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    let union = a.union(b).count();
    if union == 0 {
        return 0.0;
    }
    a.intersection(b).count() as f64 / union as f64
}

/// Standard normal (Box-Muller).
fn gauss(rng: &mut impl Rng) -> f64 {
    let u1: f64 = rng.gen_range(f64::EPSILON..1.0);
    let u2: f64 = rng.gen();
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

/// `floor` plus a log-normal extra (median floor/2, capped at 8x floor), so
/// gaps never fall into a fixed cadence (X scores timing regularity).
fn jittered(floor: f64, sigma: f64, rng: &mut impl Rng) -> f64 {
    floor + (floor * 0.5 * (sigma * gauss(rng)).exp()).min(floor * 8.0)
}

/// Time a person spends reading a post of `chars` characters.
fn read_secs(b: &Budget, chars: usize, rng: &mut impl Rng) -> f64 {
    let (lo, hi) = (b.read_min_secs as f64, b.read_max_secs.max(b.read_min_secs) as f64);
    ((lo + chars as f64 / 15.0) * (0.3 * gauss(rng)).exp()).clamp(lo, hi)
}

/// Where a reply lands, from its ancestor chain (target first). `complete`
/// means the chain's last node is the conversation root.
fn classify_reply(
    snap: &Snapshot,
    chain: &[Node],
    complete: bool,
    own: Option<&str>,
    now: f64,
) -> Result<(Act, Option<String>), XmasterError> {
    let Some(target) = chain.first() else { return Ok((Act::Reply, None)) };
    let is_own = |n: &Node| {
        own.is_some_and(|o| n.author.as_deref() == Some(o)) || snap.parent(&n.id, now).is_some()
    };
    let known = chain.iter().find_map(|n| snap.conversation_of(&n.id, now));
    let (root, root_ours) = match known {
        Some(k) => k,
        None => {
            let last = chain.last().unwrap_or(target);
            // An unresolved chain under our own post is most likely our thread
            let ours = if complete { is_own(last) } else { is_own(target) };
            (last.id.clone(), ours)
        }
    };
    match (is_own(target), root_ours) {
        (true, true) => Ok((Act::Thread, Some(root))),
        (true, false) => Err(multi_step()),
        (false, true) => Ok((Act::OwnReply, Some(root))),
        (false, false) => Ok((Act::Reply, Some(root))),
    }
}

fn multi_step() -> XmasterError {
    refuse(
        "rails_blocked",
        "Not sent: this replies to your own reply under someone else's post. X runs a \
         separate spam check on reply chains like that; reply to the other person's post instead"
            .into(),
    )
}

// ---------------------------------------------------------------------------
// Ledger (SQLite, shared by parallel xmaster processes)
// ---------------------------------------------------------------------------

const DDL: &str = "
    CREATE TABLE IF NOT EXISTS rails_ledger (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        at              REAL    NOT NULL,
        kind            TEXT    NOT NULL,
        status          TEXT    NOT NULL,
        target_id       TEXT,
        target_author   TEXT,
        conversation_id TEXT,
        new_id          TEXT,
        text            TEXT,
        next_gap        REAL    NOT NULL,
        reply_gap       REAL,
        error_code      TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_rails_ledger_at ON rails_ledger(at);
    CREATE TABLE IF NOT EXISTS rails_state (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
";

struct Ledger {
    conn: Connection,
}

impl Ledger {
    fn open() -> Result<Self, XmasterError> {
        Self::init(super::open_db("xmaster.db")?)
    }

    #[cfg(test)]
    fn open_at(path: &std::path::Path) -> Result<Self, XmasterError> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "wal").ok();
        conn.pragma_update(None, "busy_timeout", 5000).ok();
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self, XmasterError> {
        conn.execute_batch(DDL)?;
        Ok(Self { conn })
    }

    fn snapshot(&self, b: &Budget, now: f64) -> Result<Snapshot, XmasterError> {
        load(&self.conn, b, now)
    }

    /// Plan this write against everyone's reservations and claim its slot,
    /// atomically, so parallel agents queue instead of bursting.
    #[allow(clippy::too_many_arguments)]
    fn reserve(
        &mut self,
        b: &Budget,
        act: Act,
        intent: &WriteIntent,
        target: &Target,
        own: Option<&str>,
        now: f64,
        rng: &mut impl Rng,
    ) -> Result<(i64, Plan), XmasterError> {
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let snap = load(&tx, b, now)?;
        let plan = plan(b, &snap, act, intent.text.as_deref(), target, own, now, rng)?;
        let author = match act {
            Act::Thread => None,
            _ => target.author.clone(),
        };
        let text = if act.creates() { intent.text.as_deref() } else { None };
        tx.execute(
            "INSERT INTO rails_ledger
                (at, kind, status, target_id, target_author, conversation_id, text, next_gap, reply_gap)
             VALUES (?1, ?2, 'pending', ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                plan.go_at,
                act.as_str(),
                intent.target_id,
                author,
                target.conversation,
                text,
                plan.next_gap,
                plan.reply_gap
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute("DELETE FROM rails_ledger WHERE at < ?1", params![now - KEEP_DAYS * DAY])?;
        tx.commit()?;
        Ok((id, plan))
    }

    fn finish(&self, id: i64, status: Status, new_id: Option<&str>, error_code: Option<&str>) -> Result<(), XmasterError> {
        self.conn.execute(
            "UPDATE rails_ledger
                SET status = ?2, new_id = ?3, error_code = ?4,
                    conversation_id = COALESCE(conversation_id, ?3)
              WHERE id = ?1",
            params![id, status.as_str(), new_id, error_code],
        )?;
        Ok(())
    }

    fn set(&self, key: &str, value: impl ToString) -> Result<(), XmasterError> {
        self.conn.execute(
            "INSERT INTO rails_state (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value.to_string()],
        )?;
        Ok(())
    }

    fn clear(&self, pattern: &str) -> Result<usize, XmasterError> {
        Ok(self.conn.execute("DELETE FROM rails_state WHERE key LIKE ?1", params![pattern])?)
    }

    /// Store what an X error means for future writes.
    fn apply_trip(&self, trip: &Trip, act: Act, b: &Budget, now: f64) -> Result<(), XmasterError> {
        match trip {
            Trip::None => {}
            Trip::Account(reason) => {
                let until = now + b.cooldown_hours as f64 * HOUR;
                self.set("cooldown_until", until)?;
                self.set("cooldown_reason", reason)?;
                self.set("half_caps_until", until + b.half_caps_days as f64 * DAY)?;
            }
            Trip::Challenge => self.set("challenge_until", now + CHALLENGE_SECS)?,
            Trip::Park(until) => self.set(&format!("blocked_until:{}", act.family()), until)?,
        }
        Ok(())
    }
}

fn load(conn: &Connection, b: &Budget, now: f64) -> Result<Snapshot, XmasterError> {
    let days = b.duplicate_days.max(b.unfollow_after_days).max(1) as f64;
    let mut stmt = conn.prepare(
        "SELECT CAST(at AS REAL), kind, status, target_id, target_author, conversation_id,
                new_id, text, CAST(next_gap AS REAL), reply_gap
           FROM rails_ledger WHERE at >= ?1 ORDER BY at ASC",
    )?;
    let rows = stmt
        .query_map(params![now - days * DAY - HOUR], |r| {
            Ok((
                r.get::<_, f64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, f64>(8)?,
                r.get::<_, Option<f64>>(9)?,
            ))
        })?
        .filter_map(Result::ok)
        .filter_map(|(at, kind, status, target_id, author, conversation, new_id, text, next_gap, reply_gap)| {
            Some(Row {
                at,
                act: Act::parse(&kind)?,
                status: Status::parse(&status),
                target_id,
                author,
                conversation,
                new_id,
                text,
                next_gap,
                reply_gap,
            })
        })
        .collect();

    // Posts made before the rails existed (tracker table; may be absent)
    let since = now - b.duplicate_days as f64 * DAY;
    let posts = conn
        .prepare("SELECT tweet_id, content_type, text FROM posts WHERE CAST(posted_at AS INTEGER) >= ?1")
        .and_then(|mut s| {
            s.query_map(params![since as i64], |r| {
                Ok(OwnPost { id: r.get(0)?, kind: r.get(1)?, text: r.get(2)? })
            })?
            .collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or_default();

    let kv = conn
        .prepare("SELECT key, value FROM rails_state")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<HashMap<_, _>, _>>()?;

    Ok(Snapshot { rows, posts, kv })
}

// ---------------------------------------------------------------------------
// X error classification
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum Trip {
    None,
    /// Lock, suspension, captcha or verification: stop every write.
    Account(String),
    /// A challenge page instead of data: pause briefly.
    Challenge,
    /// Rate limit: park this action type until then.
    Park(f64),
}

fn classify(err: &XmasterError, now: f64) -> Trip {
    if let XmasterError::RateLimited { reset_at, .. } = err {
        let reset = *reset_at as f64;
        return Trip::Park(if reset > now { reset } else { now + PARK_SECS });
    }
    let msg = err.to_string();
    let codes: Vec<u32> = regex::Regex::new(r#"(?i)\bcode\b\W{0,4}(\d{2,3})\b"#)
        .expect("valid regex")
        .captures_iter(&msg)
        .filter_map(|c| c[1].parse().ok())
        .collect();
    let has = |c: u32| codes.contains(&c);

    if let Some(c) = [226, 64, 326, 344].into_iter().find(|c| has(*c)) {
        let what = match c {
            226 => "flagged as automated",
            64 => "suspended",
            326 => "locked",
            _ => "daily limit hit",
        };
        return Trip::Account(format!("X code {c}: {what}"));
    }
    let app_permission = msg.contains("oauth1-permissions") || msg.contains("application");
    let lock_words = regex::Regex::new(
        r#"(?i)arkose|captcha|\bverify\b|challenge_login|phone_verify|account/access|account (is )?(temporarily )?(locked|restricted)"#,
    )
    .expect("valid regex");
    if !app_permission && (lock_words.is_match(&msg) || msg.contains("READ_ONLY")) {
        return Trip::Account("X asked for verification or a captcha".into());
    }
    if err.error_code() == "web_challenge" {
        return Trip::Challenge;
    }
    if has(161) {
        let midnight = chrono::DateTime::from_timestamp(now as i64, 0)
            .and_then(|d| d.date_naive().succ_opt())
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|d| d.and_utc().timestamp() as f64)
            .unwrap_or(now + DAY);
        return Trip::Park(midnight);
    }
    if has(88) || msg.contains("HTTP 429") {
        return Trip::Park(now + PARK_SECS);
    }
    Trip::None
}

// ---------------------------------------------------------------------------
// Gate and permit
// ---------------------------------------------------------------------------

static LAST_WAIT: AtomicU64 = AtomicU64::new(0);
static SILENT_FAIL: Mutex<Option<String>> = Mutex::new(None);

/// Seconds the last gate in this process waited (for command output).
pub fn last_wait_secs() -> u64 {
    LAST_WAIT.load(Ordering::Relaxed)
}

/// A post X accepted but that didn't read back, if any, in this process.
pub fn possible_silent_fail() -> Option<String> {
    SILENT_FAIL.lock().ok().and_then(|g| g.clone())
}

fn rails_off() -> bool {
    std::env::var("XMASTER_RAILS").is_ok_and(|v| matches!(v.to_ascii_lowercase().as_str(), "off" | "0" | "false"))
}

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn fmt_ts(secs: f64) -> String {
    chrono::DateTime::from_timestamp(secs as i64, 0)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

/// The logged-in handle from the cached web session, if any.
fn own_handle() -> Option<String> {
    let bytes = std::fs::read(crate::config::config_dir().join("web_session.json")).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("viewer_handle")?.as_str().filter(|h| !h.is_empty()).map(str::to_lowercase)
}

fn replied_to(t: &TweetData) -> Option<String> {
    t.referenced_tweets
        .as_ref()?
        .iter()
        .find(|r| r.ref_type == "replied_to")
        .map(|r| r.id.clone())
}

/// Learn what the write touches: the target's author, age and length, and for
/// replies the conversation root (walking up through free FxTwitter reads).
async fn resolve(
    intent: &WriteIntent,
    snap: &Snapshot,
    own: Option<&str>,
    fx: bool,
    now: f64,
) -> Result<(Act, Target), XmasterError> {
    let act = Act::of(intent.kind);
    match act {
        Act::Follow | Act::Unfollow | Act::Dm => {
            return Ok((act, Target { author: intent.target_handle.clone(), ..Default::default() }))
        }
        Act::Reply | Act::Quote | Act::Like | Act::Repost => {}
        _ => return Ok((act, Target::default())),
    }
    let Some(id) = intent.target_id.as_deref() else { return Ok((act, Target::default())) };
    let unknown = Target { conversation: (act == Act::Reply).then(|| id.to_string()), ..Default::default() };

    // Replying to our own post: a thread, or a chain under someone else's post.
    // Decided from the ledger: a post made seconds ago may not be readable yet.
    if act == Act::Reply {
        match snap.parent(id, now) {
            Some(Parent::Ledger(Act::Reply, _)) => return Err(multi_step()),
            Some(Parent::Ledger(_, conv)) => {
                return Ok((Act::Thread, Target { conversation: conv.or(Some(id.to_string())), ..Default::default() }))
            }
            Some(Parent::Posted(kind)) if kind != "reply" => {
                return Ok((Act::Thread, Target { conversation: Some(id.to_string()), ..Default::default() }))
            }
            _ => {}
        }
    }
    if !fx {
        return Ok((act, unknown));
    }
    let Some((post, _)) = crate::providers::fxtwitter::fetch_post(id).await else { return Ok((act, unknown)) };
    let mut target = Target {
        author: post.author_username.as_deref().map(str::to_lowercase),
        created_at: post
            .created_at
            .as_deref()
            .and_then(|c| chrono::DateTime::parse_from_rfc3339(c).ok())
            .map(|d| d.timestamp() as f64),
        chars: Some(post.text.chars().count()),
        conversation: None,
    };
    if act != Act::Reply {
        return Ok((act, target));
    }

    let mut chain = vec![Node { id: post.id.clone(), author: target.author.clone() }];
    let mut parent = replied_to(&post);
    let mut complete = parent.is_none();
    while let Some(pid) = parent.take() {
        if chain.len() > MAX_HOPS || snap.conversation_of(&pid, now).is_some() {
            chain.push(Node { id: pid, author: None });
            break;
        }
        match crate::providers::fxtwitter::fetch_post(&pid).await {
            Some((p, _)) => {
                parent = replied_to(&p);
                complete = parent.is_none();
                chain.push(Node { id: p.id.clone(), author: p.author_username.as_deref().map(str::to_lowercase) });
            }
            None => chain.push(Node { id: pid, author: None }),
        }
    }
    let (act, conversation) = classify_reply(snap, &chain, complete, own, now)?;
    target.conversation = conversation;
    Ok((act, target))
}

/// Permission to make one write. Call [`Permit::record`] with the result.
#[must_use = "record the write's outcome with Permit::record"]
pub struct Permit {
    row: Option<i64>,
    act: Act,
    budget: Budget,
    verify: bool,
    /// Seconds the gate waited before allowing this write.
    pub waited_secs: u64,
}

/// What recording a write found out.
#[derive(Debug, Default, Clone, Serialize)]
pub struct Recorded {
    /// X accepted the post but it doesn't read back: stop and check.
    pub possible_silent_fail: bool,
    /// X locked, challenged or rate-limited the account: what stops now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tripped: Option<String>,
}

/// A write's result: the new post ID if it created one, or the error.
pub type Outcome<'a> = Result<Option<&'a str>, &'a XmasterError>;

/// Wait until this write looks human, or refuse it. Never sends anything.
pub async fn gate(ctx: &AppContext, intent: &WriteIntent) -> Result<Permit, XmasterError> {
    let b = budget(ctx);
    let act = Act::of(intent.kind);
    if rails_off() {
        return Ok(Permit { row: None, act, budget: b, verify: false, waited_secs: 0 });
    }
    if let Some(id) = possible_silent_fail() {
        return Err(refuse(
            "possible_silent_fail",
            format!(
                "Stopped: X accepted post {id} earlier in this run but it doesn't read back \
                 (possible silent fail). Check https://x.com/i/status/{id} before writing more"
            ),
        ));
    }
    let snap = Ledger::open()?.snapshot(&b, now_secs())?;
    check_locks(&snap, now_secs())?;

    let own = own_handle();
    let fx = !ctx.config.settings.disable_fxtwitter;
    let opened_ms = chrono::Utc::now().timestamp_millis();
    let (act, target) = resolve(intent, &snap, own.as_deref(), fx, now_secs()).await?;

    let now = now_secs();
    let (row, plan) = {
        let mut rng = rand::thread_rng();
        Ledger::open()?.reserve(&b, act, intent, &target, own.as_deref(), now, &mut rng)?
    };
    let wait = (plan.go_at - now_secs()).max(0.0);
    if wait >= 1.0 {
        eprintln!("pacing: waiting {:.0}s like a person would ({})", wait, plan.why);
    }
    // The post "renders" (impression, batched ~2 s later like the app) and
    // stays on screen until the write (linger). Never blocks the write.
    let viewed = intent
        .target_id
        .as_deref()
        .filter(|_| b.view_events && act.reads() && ctx.config.has_web_cookies());
    let handle = target.author.as_deref();
    if let Some(id) = viewed {
        let batch = wait.min(1.5 + rand::random::<f64>() * 1.5);
        tokio::time::sleep(Duration::from_secs_f64(batch)).await;
        crate::providers::web_events::send_impression(ctx, id, handle, opened_ms).await;
    }
    let rest = (plan.go_at - now_secs()).max(0.0);
    if rest > 0.0 {
        tokio::time::sleep(Duration::from_secs_f64(rest)).await;
    }
    if let Some(id) = viewed {
        crate::providers::web_events::send_linger(ctx, id, handle, opened_ms).await;
    }
    let waited_secs = wait.round() as u64;
    LAST_WAIT.store(waited_secs, Ordering::Relaxed);
    let verify = b.verify_writes && fx && act.creates();
    Ok(Permit { row: Some(row), act, budget: b, verify, waited_secs })
}

impl Permit {
    /// Store the write's outcome. Never fails the caller: the write is done.
    pub async fn record(self, outcome: Outcome<'_>) -> Recorded {
        let Some(row) = self.row else { return Recorded::default() };
        let mut rec = Recorded::default();
        let now = now_secs();
        match outcome {
            Ok(new_id) => {
                let mut status = Status::Ok;
                if let (true, Some(id)) = (self.verify, new_id) {
                    if missing_after_post(id).await {
                        status = Status::SilentFail;
                        rec.possible_silent_fail = true;
                        if let Ok(mut g) = SILENT_FAIL.lock() {
                            *g = Some(id.to_string());
                        }
                        eprintln!(
                            "rails: X accepted post {id} but it doesn't read back (possible silent \
                             fail). No more writes this run; check https://x.com/i/status/{id}"
                        );
                    }
                }
                if let Ok(l) = Ledger::open() {
                    l.finish(row, status, new_id, None).ok();
                }
            }
            Err(e) => {
                let trip = classify(e, now);
                let status = if e.error_code() == "write_unconfirmed" { Status::Unconfirmed } else { Status::Failed };
                if let Ok(l) = Ledger::open() {
                    l.finish(row, status, None, Some(e.error_code())).ok();
                    l.apply_trip(&trip, self.act, &self.budget, now).ok();
                }
                rec.tripped = match trip {
                    Trip::None => None,
                    Trip::Account(reason) => Some(format!(
                        "{reason}: all writes stop for {} h. Open x.com to clear it, then run \
                         xmaster rails reset-cooldown --yes",
                        self.budget.cooldown_hours
                    )),
                    Trip::Challenge => Some("challenge page: writes pause for 30 min".into()),
                    Trip::Park(until) => Some(format!("{} writes parked until {}", self.act.family(), fmt_ts(until))),
                };
                if let Some(t) = &rec.tripped {
                    eprintln!("rails: {t}");
                }
            }
        }
        rec
    }
}

/// What a write returns: the new post's ID when it created one.
pub trait Written {
    fn new_id(&self) -> Option<&str> {
        None
    }
}

impl Written for () {}

impl Written for TweetResponse {
    fn new_id(&self) -> Option<&str> {
        Some(&self.id)
    }
}

/// Gate a write, run it, record the outcome. The one-line hook for XApi.
pub async fn guard<T: Written>(
    ctx: &AppContext,
    intent: WriteIntent,
    write: impl Future<Output = Result<T, XmasterError>>,
) -> Result<T, XmasterError> {
    let permit = gate(ctx, &intent).await?;
    let result = write.await;
    permit.record(result.as_ref().map(|v| v.new_id())).await;
    result
}

/// Does a just-created post fail to read back within ~10 s? Only a clear
/// "not found" counts; FxTwitter being down or slow proves nothing.
async fn missing_after_post(id: &str) -> bool {
    let mut misses = 0;
    for delay in [2u64, 3, 5] {
        tokio::time::sleep(Duration::from_secs(delay)).await;
        match visible(id).await {
            Some(true) => return false,
            Some(false) => misses += 1,
            None => {}
        }
    }
    misses >= 2
}

async fn visible(id: &str) -> Option<bool> {
    static CLIENT: std::sync::OnceLock<Option<reqwest::Client>> = std::sync::OnceLock::new();
    let client = CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_millis(2500))
                .user_agent(concat!("xmaster-cli/", env!("CARGO_PKG_VERSION")))
                .build()
                .ok()
        })
        .as_ref()?;
    let resp = client.get(format!("https://api.fxtwitter.com/i/status/{id}")).send().await.ok()?;
    match resp.status().as_u16() {
        404 => Some(false),
        200 => {
            let v: serde_json::Value = resp.json().await.ok()?;
            Some(!v.get("tweet").is_none_or(serde_json::Value::is_null))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Status, pause, resume, reset (for `xmaster rails ...`)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ActionStatus {
    pub action: &'static str,
    pub used_24h: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cap_24h: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_1h: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cap_1h: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_24h: Option<u32>,
    pub next_allowed_at: String,
    pub wait_secs: u64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct RailsStatus {
    pub enabled: bool,
    pub now: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paused_until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub half_caps_until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_write_at: Option<String>,
    pub actions: Vec<ActionStatus>,
    /// Posts X accepted in the last 24 h that didn't read back.
    pub possible_silent_fails: Vec<String>,
    pub budget: Budget,
}

/// Actions shown by `rails status`, in display order.
const STATUS_ACTS: [Act; 11] = [
    Act::Post,
    Act::Reply,
    Act::OwnReply,
    Act::Quote,
    Act::Like,
    Act::Repost,
    Act::Bookmark,
    Act::Follow,
    Act::Unfollow,
    Act::Dm,
    Act::Delete,
];

fn status_from(b: &Budget, snap: &Snapshot, now: f64) -> RailsStatus {
    let future = |key: &str| Some(snap.kv_f(key)).filter(|t| *t > now);
    let half = snap.half_caps(now);
    let count = |act: Act, window: f64| snap.landed(now).filter(|r| r.act == act && r.at > now - window).count() as u32;
    let actions = STATUS_ACTS
        .into_iter()
        .map(|act| {
            let caps = act.caps(b);
            let cap_for = |w: f64| caps.iter().find(|c| c.1 == w).map(|c| effective_cap(c.0, half));
            let (cap_24h, cap_1h) = (cap_for(DAY), cap_for(HOUR));
            let used_24h = count(act, DAY);
            let bound = pacing_bound(b, snap, act, now, LIGHT_GAP_SECS, THREAD_GAP_SECS);
            RailsStatus::action(act, used_24h, cap_24h, cap_1h.map(|_| count(act, HOUR)), cap_1h, &bound, now)
        })
        .collect();
    RailsStatus {
        enabled: !rails_off(),
        now: fmt_ts(now),
        paused_until: future("paused_until").map(|t| if t > now + 365.0 * DAY { "until resumed".into() } else { fmt_ts(t) }),
        cooldown_until: future("cooldown_until").or(future("challenge_until")).map(fmt_ts),
        cooldown_reason: future("cooldown_until").and_then(|_| snap.kv.get("cooldown_reason").cloned()),
        half_caps_until: future("half_caps_until").map(fmt_ts),
        last_write_at: snap.last().map(|r| fmt_ts(r.at)),
        actions,
        possible_silent_fails: snap
            .rows
            .iter()
            .filter(|r| r.status == Status::SilentFail && r.at > now - DAY)
            .filter_map(|r| r.new_id.clone())
            .collect(),
        budget: b.clone(),
    }
}

impl RailsStatus {
    fn action(
        act: Act,
        used_24h: u32,
        cap_24h: Option<u32>,
        used_1h: Option<u32>,
        cap_1h: Option<u32>,
        bound: &Bound,
        now: f64,
    ) -> ActionStatus {
        ActionStatus {
            action: act.as_str(),
            used_24h,
            cap_24h,
            used_1h,
            cap_1h,
            remaining_24h: cap_24h.map(|c| c.saturating_sub(used_24h)),
            next_allowed_at: fmt_ts(bound.at),
            wait_secs: (bound.at - now).max(0.0).ceil() as u64,
            reason: if bound.at > now { bound.why.clone() } else { String::new() },
        }
    }
}

/// Today's writes vs caps, next allowed time per action, cooldowns.
pub fn status(ctx: &AppContext) -> Result<RailsStatus, XmasterError> {
    let b = budget(ctx);
    let now = now_secs();
    let snap = Ledger::open()?.snapshot(&b, now)?;
    Ok(status_from(&b, &snap, now))
}

/// Stop all writes for `hours` (or until resumed). Returns the end time.
pub fn pause(hours: Option<f64>) -> Result<Option<String>, XmasterError> {
    let now = now_secs();
    let until = hours.filter(|h| *h > 0.0).map(|h| now + h * HOUR);
    Ledger::open()?.set("paused_until", until.unwrap_or(now + 100.0 * 365.0 * DAY))?;
    Ok(until.map(fmt_ts))
}

/// Allow writes again. Returns whether a pause was active.
pub fn resume() -> Result<bool, XmasterError> {
    Ok(Ledger::open()?.clear("paused_until")? > 0)
}

/// Clear an X cooldown after the owner cleared the challenge on x.com.
/// Half caps stay on for `half_caps_days` from now. Returns whether one was active.
pub fn reset_cooldown(ctx: &AppContext) -> Result<bool, XmasterError> {
    let b = budget(ctx);
    let now = now_secs();
    let ledger = Ledger::open()?;
    let snap = ledger.snapshot(&b, now)?;
    let active = snap.kv_f("cooldown_until") > now || snap.kv_f("challenge_until") > now;
    ledger.clear("cooldown_until")?;
    ledger.clear("cooldown_reason")?;
    ledger.clear("challenge_until")?;
    ledger.clear("blocked_until:%")?;
    if active {
        let until = (now + b.half_caps_days as f64 * DAY).max(snap.kv_f("half_caps_until"));
        ledger.set("half_caps_until", until)?;
    }
    Ok(active)
}

// ---------------------------------------------------------------------------
// Tests (explicit clock and seeded rng; nothing sleeps)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    const T0: f64 = 1_791_849_433.0;

    fn rng() -> StdRng {
        StdRng::seed_from_u64(7)
    }

    fn row(at: f64, act: Act) -> Row {
        Row {
            at,
            act,
            status: Status::Ok,
            target_id: None,
            author: None,
            conversation: None,
            new_id: None,
            text: None,
            next_gap: 45.0,
            reply_gap: (act == Act::Reply).then_some(180.0),
        }
    }

    fn snap(rows: Vec<Row>) -> Snapshot {
        Snapshot { rows, ..Default::default() }
    }

    fn go(b: &Budget, s: &Snapshot, act: Act, t: &Target, now: f64) -> Result<Plan, XmasterError> {
        plan(b, s, act, None, t, Some("me"), now, &mut rng())
    }

    fn code(r: Result<Plan, XmasterError>) -> &'static str {
        r.err().map(|e| e.error_code()).unwrap_or("ok")
    }

    fn cv(xs: &[f64]) -> f64 {
        let m = xs.iter().sum::<f64>() / xs.len() as f64;
        let var = xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / xs.len() as f64;
        var.sqrt() / m
    }

    #[test]
    fn gap_after_last_write_and_never_a_burst() {
        let b = Budget::default();
        let mut r = row(T0, Act::Post);
        r.next_gap = 60.0;
        let p = go(&b, &snap(vec![r]), Act::Post, &Target::default(), T0 + 10.0).unwrap();
        assert!((p.go_at - (T0 + 60.0)).abs() < 1e-6, "{}", p.go_at);

        // Thread parts only keep clear of the 1 s burst window
        let p = go(&b, &snap(vec![row(T0, Act::Post)]), Act::Thread, &Target::default(), T0).unwrap();
        assert!(p.go_at >= T0 + BURST_SECS + 1.0 && p.go_at < T0 + 30.0, "{}", p.go_at - T0);
    }

    #[test]
    fn reply_gap_applies_only_to_stranger_replies() {
        let b = Budget::default();
        let s = snap(vec![row(T0, Act::Reply)]);
        let p = go(&b, &s, Act::Reply, &Target::default(), T0 + 100.0).unwrap();
        assert!(p.go_at >= T0 + 180.0);
        let p = go(&b, &s, Act::OwnReply, &Target::default(), T0 + 100.0).unwrap();
        assert!(p.go_at < T0 + 180.0);
    }

    #[test]
    fn jitter_is_irregular_and_bounded() {
        let mut g = rng();
        for floor in [45.0, 180.0] {
            let xs: Vec<f64> = (0..20_000).map(|_| jittered(floor, 1.1, &mut g)).collect();
            assert!(xs.iter().all(|x| *x >= floor && *x <= floor * 9.0));
            assert!(cv(&xs) >= 0.5, "cv {} for floor {floor}", cv(&xs));
            let median = {
                let mut s = xs.clone();
                s.sort_by(f64::total_cmp);
                s[s.len() / 2]
            };
            assert!((median / floor - 1.5).abs() < 0.05, "median {median}");
        }
        let b = Budget::default();
        let reads: Vec<f64> = (0..5_000).map(|i| read_secs(&b, i % 600, &mut g)).collect();
        assert!(reads.iter().all(|r| (8.0..=40.0).contains(r)));
        assert!(cv(&reads) > 0.1);
    }

    #[test]
    fn short_waits_sleep_long_waits_refuse_with_retry_after() {
        let b = Budget::default();
        let mut r = row(T0, Act::Like);
        r.next_gap = 200.0;
        let s = snap(vec![r.clone()]);
        assert!(go(&b, &s, Act::Like, &Target::default(), T0).is_ok());
        r.next_gap = 400.0;
        let e = go(&b, &snap(vec![r]), Act::Like, &Target::default(), T0).unwrap_err();
        assert_eq!(e.error_code(), "budget_exceeded");
        assert!(e.to_string().contains("retry_after_secs=400"), "{e}");
    }

    #[test]
    fn caps_use_rolling_windows_and_halve_after_a_challenge() {
        let b = Budget::default();
        let likes: Vec<Row> = (0..60).map(|i| row(T0 + i as f64 * 600.0, Act::Like)).collect();
        let now = T0 + 60.0 * 600.0;
        let e = go(&b, &snap(likes.clone()), Act::Like, &Target::default(), now).unwrap_err();
        assert!(e.to_string().contains("cap of 60 likes per day"), "{e}");
        // The first like leaves the window 24 h after it was made
        let free = T0 + DAY;
        assert!(e.to_string().contains(&format!("retry_after_secs={}", (free - now) as u64)), "{e}");

        let replies: Vec<Row> = (0..5).map(|i| row(T0 + i as f64 * 300.0, Act::Reply)).collect();
        let e = go(&b, &snap(replies), Act::Reply, &Target::default(), T0 + 1_500.0).unwrap_err();
        assert!(e.to_string().contains("replies per hour"), "{e}");

        let mut s = snap(likes[..30].to_vec());
        s.kv.insert("half_caps_until".into(), (T0 + 30.0 * DAY).to_string());
        let e = go(&b, &s, Act::Like, &Target::default(), T0 + 30.0 * 600.0).unwrap_err();
        assert!(e.to_string().contains("halved"), "{e}");
    }

    #[test]
    fn same_author_streak_stops_the_third_action() {
        let b = Budget::default();
        let on = |a: &str, at: f64| Row { author: Some(a.into()), ..row(at, Act::Like) };
        let s = snap(vec![on("alice", T0), on("alice", T0 + 100.0)]);
        let alice = Target { author: Some("alice".into()), ..Default::default() };
        let bob = Target { author: Some("bob".into()), ..Default::default() };
        assert_eq!(code(go(&b, &s, Act::Like, &alice, T0 + 500.0)), "rails_blocked");
        assert_eq!(code(go(&b, &s, Act::Like, &bob, T0 + 500.0)), "ok");
        let s = snap(vec![on("alice", T0), on("bob", T0 + 100.0)]);
        assert_eq!(code(go(&b, &s, Act::Like, &alice, T0 + 500.0)), "ok");
    }

    #[test]
    fn one_reply_per_strangers_conversation_per_day() {
        let b = Budget::default();
        let s = snap(vec![Row { conversation: Some("root".into()), ..row(T0, Act::Reply) }]);
        let same = Target { conversation: Some("root".into()), ..Default::default() };
        let other = Target { conversation: Some("other".into()), ..Default::default() };
        let e = go(&b, &s, Act::Reply, &same, T0 + 3_600.0).unwrap_err();
        assert!(e.to_string().contains("this conversation"), "{e}");
        assert_eq!(code(go(&b, &s, Act::Reply, &other, T0 + 3_600.0)), "ok");
        assert_eq!(code(go(&b, &s, Act::Reply, &same, T0 + DAY + 1.0)), "ok");
    }

    #[test]
    fn replies_are_classified_by_where_they_land() {
        let now = T0;
        let n = |id: &str, a: &str| Node { id: id.into(), author: Some(a.into()) };
        let s = snap(vec![]);
        let own = Some("me");
        // Stranger's thread
        let (act, conv) = classify_reply(&s, &[n("2", "bob"), n("1", "alice")], true, own, now).unwrap();
        assert_eq!((act, conv.as_deref()), (Act::Reply, Some("1")));
        // Someone replied to our post
        let (act, _) = classify_reply(&s, &[n("2", "bob"), n("1", "me")], true, own, now).unwrap();
        assert_eq!(act, Act::OwnReply);
        // Continuing our own thread
        let (act, _) = classify_reply(&s, &[n("2", "me"), n("1", "me")], true, own, now).unwrap();
        assert_eq!(act, Act::Thread);
        // Our own reply under a stranger's post: the multi-step pattern
        let e = classify_reply(&s, &[n("2", "me"), n("1", "alice")], true, own, now).unwrap_err();
        assert_eq!(e.error_code(), "rails_blocked");
        // Known conversation from an earlier reply short-circuits the walk
        let s = snap(vec![Row {
            target_id: Some("1".into()),
            new_id: Some("9".into()),
            conversation: Some("1".into()),
            ..row(now - 100.0, Act::Reply)
        }]);
        let (act, conv) = classify_reply(&s, &[n("5", "carol"), Node { id: "9".into(), author: None }], false, own, now).unwrap();
        assert_eq!((act, conv.as_deref()), (Act::Reply, Some("1")));
    }

    #[test]
    fn near_duplicate_text_is_blocked() {
        let b = Budget::default();
        let mut r = row(T0, Act::Reply);
        r.text = Some("Great point about rapamycin dosing, @bob https://t.co/x".into());
        r.new_id = Some("42".into());
        let s = snap(vec![r]);
        let p = |text: &str| plan(&b, &s, Act::Reply, Some(text), &Target::default(), None, T0 + DAY, &mut rng());
        let e = p("@alice great point about rapamycin dosing!").unwrap_err();
        assert!(e.to_string().contains("near-duplicate of your post 42"), "{e}");
        assert!(p("Rapamycin trials in dogs show a different pattern entirely").is_ok());
        assert!(jaccard(&words("a b c d"), &words("a b x y")) - 1.0 / 3.0 < 1e-9);
        assert!(words("@x hi https://t.co/a HI!").len() == 1);
    }

    #[test]
    fn no_unfollow_within_a_week_and_young_posts_wait() {
        let b = Budget::default();
        let s = snap(vec![Row { author: Some("carol".into()), ..row(T0, Act::Follow) }]);
        let carol = Target { author: Some("carol".into()), ..Default::default() };
        let e = go(&b, &s, Act::Unfollow, &carol, T0 + DAY).unwrap_err();
        assert!(e.to_string().contains("followed @carol"), "{e}");
        assert_eq!(code(go(&b, &s, Act::Unfollow, &carol, T0 + 7.0 * DAY + 1.0)), "ok");

        let young = Target { created_at: Some(T0 - 30.0), ..Default::default() };
        let p = go(&b, &snap(vec![]), Act::Reply, &young, T0).unwrap();
        assert!(p.go_at >= T0 + 60.0 && p.go_at <= T0 + 80.0, "{}", p.go_at - T0);
    }

    #[test]
    fn x_errors_trip_the_right_stop() {
        let now = T0;
        let web = |m: &str| XmasterError::Api { provider: "x-web", code: "web_error", message: m.into() };
        assert!(matches!(classify(&web("CreateTweet failed: automated [code 226]"), now), Trip::Account(_)));
        assert!(matches!(classify(&web(r#"HTTP 403: {"errors":[{"code":326}]}"#), now), Trip::Account(_)));
        assert!(matches!(classify(&web("Please verify your phone"), now), Trip::Account(_)));
        assert!(matches!(classify(&web("Arkose challenge required"), now), Trip::Account(_)));
        assert_eq!(classify(&web("duplicate [code 187]"), now), Trip::None);
        assert_eq!(classify(&web("Read-only application cannot POST"), now), Trip::None);
        assert_eq!(classify(&web("verified account"), now), Trip::None);
        let challenge = XmasterError::Api { provider: "x-web", code: "web_challenge", message: "page".into() };
        assert_eq!(classify(&challenge, now), Trip::Challenge);
        let Trip::Park(until) = classify(&web("follow limit [code 161]"), now) else { panic!() };
        assert!(until > now && until <= now + DAY && (until as i64) % 86_400 == 0);
        let rl = XmasterError::RateLimited { provider: "x", reset_at: (now + 300.0) as u64 };
        assert_eq!(classify(&rl, now), Trip::Park(now + 300.0));
        assert_eq!(classify(&web("[code 88] rate limit"), now), Trip::Park(now + PARK_SECS));
    }

    #[test]
    fn ledger_queues_parallel_writers_and_enforces_cooldowns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.db");
        let b = Budget::default();
        let intent = WriteIntent::new(WriteKind::Post).text("hello there");
        let t = Target::default();
        let mut g = rng();

        // Two writers planning at the same instant don't share a slot
        let mut a = Ledger::open_at(&path).unwrap();
        let mut c = Ledger::open_at(&path).unwrap();
        let (r1, p1) = a.reserve(&b, Act::Post, &intent, &t, None, T0, &mut g).unwrap();
        let other = WriteIntent::new(WriteKind::Post).text("something else entirely");
        let (_, p2) = c.reserve(&b, Act::Post, &other, &t, None, T0, &mut g).unwrap();
        assert!(p2.go_at >= p1.go_at + b.min_gap_secs as f64, "{} vs {}", p2.go_at, p1.go_at);
        a.finish(r1, Status::Ok, Some("100"), None).unwrap();
        let s = a.snapshot(&b, T0 + 1.0).unwrap();
        assert_eq!(s.rows[0].conversation.as_deref(), Some("100"));
        assert!(matches!(s.parent("100", T0 + 1.0), Some(Parent::Ledger(Act::Post, _))));

        // A 226 stops everything for 24 h, then caps run at half
        let err = XmasterError::Api { provider: "x-web", code: "web_error", message: "[code 226]".into() };
        a.apply_trip(&classify(&err, T0), Act::Post, &b, T0).unwrap();
        let s = a.snapshot(&b, T0 + HOUR).unwrap();
        assert_eq!(code(go(&b, &s, Act::Like, &t, T0 + HOUR)), "account_cooldown");
        let later = T0 + 25.0 * HOUR;
        let s = a.snapshot(&b, later).unwrap();
        assert_eq!(code(go(&b, &s, Act::Like, &t, later)), "ok");
        assert!(s.half_caps(later));

        a.set("paused_until", T0 + 100.0 * DAY).unwrap();
        let s = a.snapshot(&b, later).unwrap();
        assert_eq!(code(go(&b, &s, Act::Like, &t, later)), "rails_paused");
        a.clear("paused_until").unwrap();

        let st = status_from(&b, &a.snapshot(&b, T0 + 2.0).unwrap(), T0 + 2.0);
        let post = st.actions.iter().find(|x| x.action == "post").unwrap();
        assert_eq!((post.used_24h, post.cap_24h), (2, Some(5)), "half caps still on");
        assert!(post.wait_secs > 0);
    }
}
