use crate::config;
use crate::output::{self, OutputFormat, Tableable};
use serde::Serialize;

#[derive(Serialize)]
struct AgentInfo {
    name: String,
    version: String,
    description: String,
    commands: Vec<String>,
    capabilities: Vec<String>,
    env_prefix: String,
    config_path: String,
    /// Algorithm intelligence — agents read this to understand how to optimise.
    /// Source: xai-org/x-algorithm open-source code (January 2026).
    algorithm: AlgorithmInfo,
    /// Which signals xmaster can measure, which it can only proxy, and which are blind.
    measurement_coverage: MeasurementCoverage,
    /// Hints for optimal usage — the CLI tells agents how to use it well.
    usage_hints: Vec<String>,
    /// Workflow handoff hints — tells agents what command to run after each action.
    handoffs: Vec<Handoff>,
    /// User's writing style for X posts (only present when configured).
    #[serde(skip_serializing_if = "Option::is_none")]
    writing_style: Option<String>,
}

#[derive(Serialize)]
struct MeasurementCoverage {
    /// Signals that X API returns directly — xmaster can track these.
    measurable: Vec<String>,
    /// Signals that X API doesn't expose — xmaster uses heuristic proxies.
    proxy_only: Vec<ProxySignal>,
    /// Signals with no API or proxy — completely invisible to xmaster.
    blind: Vec<String>,
}

#[derive(Serialize)]
struct ProxySignal {
    signal: String,
    proxy_method: String,
    confidence: String,
}

#[derive(Serialize)]
struct Handoff {
    after_command: String,
    next_commands: Vec<String>,
    reason: String,
}

#[derive(Serialize)]
struct AlgorithmInfo {
    source: String,
    weights: Vec<SignalWeight>,
    time_decay_halflife_minutes: u32,
    out_of_network_reply_penalty: f64,
    media_hierarchy: Vec<String>,
    best_posting_hours: String,
    best_posting_days: String,
}

#[derive(Serialize)]
struct SignalWeight {
    signal: String,
    weight: f64,
    ratio_to_like: String,
}

impl Tableable for AgentInfo {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Field", "Value"]);
        table.add_row(vec!["Name", &self.name]);
        table.add_row(vec!["Version", &self.version]);
        table.add_row(vec!["Description", &self.description]);
        table.add_row(vec!["Commands", &format!("{} commands", self.commands.len())]);
        table.add_row(vec!["Capabilities", &self.capabilities.join(", ")]);
        table.add_row(vec!["Algorithm Source", &self.algorithm.source]);
        table.add_row(vec!["Top Signal", "Follow from post (~30x), DM share (~25x), Reply (~20x)"]);
        table.add_row(vec!["Signals", "19 total (15 positive, 4 negative) — weights unpublished"]);
        table.add_row(vec!["Best Times", &self.algorithm.best_posting_hours]);
        table.add_row(vec!["Best Days", &self.algorithm.best_posting_days]);
        table.add_row(vec![
            "Measurable Signals",
            &self.measurement_coverage.measurable.join(", "),
        ]);
        table.add_row(vec![
            "Proxy Signals",
            &self.measurement_coverage.proxy_only
                .iter()
                .map(|p| format!("{} ({})", p.signal, p.confidence))
                .collect::<Vec<_>>()
                .join(", "),
        ]);
        table.add_row(vec![
            "Blind Signals",
            &format!("{} signals (no API/proxy)", self.measurement_coverage.blind.len()),
        ]);
        table.add_row(vec!["Hint", &self.usage_hints.first().cloned().unwrap_or_default()]);
        if let Some(ref style) = self.writing_style {
            table.add_row(vec!["Writing Style", style]);
        }
        table
    }
}

pub fn execute(format: OutputFormat) {
    let style = config::load_config()
        .ok()
        .and_then(|c| {
            if c.style.voice.is_empty() {
                None
            } else {
                Some(c.style.voice)
            }
        });

    let info = AgentInfo {
        name: "xmaster".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        description: "Enterprise-grade X/Twitter CLI with built-in algorithm intelligence".into(),
        commands: vec![
            // Reading posts (use 'read' as the primary single-post lookup)
            "read".into(), "replies".into(), "metrics".into(),
            "timeline".into(), "mentions".into(), "search".into(),
            "search-ai".into(), "trending".into(), "user".into(), "me".into(),
            "followers".into(), "following".into(),
            // Posting
            "post".into(), "reply".into(), "thread".into(), "delete".into(),
            // Engagement
            "like".into(), "unlike".into(),
            "retweet".into(), "unretweet".into(), "bookmark".into(), "unbookmark".into(),
            "follow".into(), "unfollow".into(),
            // Moderation
            "hide-reply".into(), "unhide-reply".into(), "block".into(), "unblock".into(),
            "mute".into(), "unmute".into(),
            // DMs
            "dm send".into(), "dm inbox".into(), "dm thread".into(),
            // Bookmarks
            "bookmarks list".into(), "bookmarks sync".into(), "bookmarks search".into(),
            "bookmarks export".into(), "bookmarks digest".into(), "bookmarks stats".into(),
            // Lists
            "lists".into(),
            // Intelligence
            "analyze".into(), "engage recommend".into(), "engage feed".into(),
            "engage hot-targets".into(),
            "likers".into(), "retweeters".into(), "quotes".into(), "users".into(),
            "track run".into(), "track status".into(),
            "track followers".into(), "track growth".into(),
            "report daily".into(), "report weekly".into(),
            "suggest best-time".into(), "suggest next-post".into(),
            // Scheduling
            "schedule add".into(), "schedule list".into(), "schedule cancel".into(),
            "schedule reschedule".into(), "schedule fire".into(), "schedule setup".into(),
            // System
            "config show".into(), "config get".into(), "config set".into(), "config check".into(),
            "config web-login".into(),
            "inspire".into(),
            "rate-limits".into(), "agent-info".into(), "update".into(),
        ],
        capabilities: vec![
            "tweet_crud".into(), "engagement".into(), "social_graph".into(),
            "direct_messages".into(), "search".into(), "ai_search".into(),
            "media_upload".into(), "user_lookup".into(), "lists".into(),
            "moderation".into(), "analytics".into(), "preflight_scoring".into(),
            "performance_tracking".into(), "timing_intelligence".into(),
            "scheduling".into(),
            "bookmark_intelligence".into(),
            "engagement_intelligence".into(),
            "self_update".into(),
        ],
        env_prefix: "XMASTER_".into(),
        config_path: config::config_path().to_string_lossy().to_string(),
        algorithm: AlgorithmInfo {
            source: "xai-org/x-algorithm (January 2026, Grok-based transformer). Exact weights unpublished — estimates below from code structure + empirical data.".into(),
            weights: vec![
                SignalWeight { signal: "follow_author".into(), weight: 30.0, ratio_to_like: "~30x (estimated)".into() },
                SignalWeight { signal: "share_via_dm".into(), weight: 25.0, ratio_to_like: "~25x (estimated)".into() },
                SignalWeight { signal: "reply".into(), weight: 20.0, ratio_to_like: "~20x (estimated)".into() },
                SignalWeight { signal: "share_via_copy_link".into(), weight: 20.0, ratio_to_like: "~20x (estimated)".into() },
                SignalWeight { signal: "quote".into(), weight: 18.0, ratio_to_like: "~18x (estimated)".into() },
                SignalWeight { signal: "profile_click".into(), weight: 12.0, ratio_to_like: "~12x (estimated)".into() },
                SignalWeight { signal: "click".into(), weight: 10.0, ratio_to_like: "~10x (estimated)".into() },
                SignalWeight { signal: "share".into(), weight: 10.0, ratio_to_like: "~10x (estimated)".into() },
                SignalWeight { signal: "dwell".into(), weight: 8.0, ratio_to_like: "~8x (estimated)".into() },
                SignalWeight { signal: "retweet".into(), weight: 3.0, ratio_to_like: "~3x (estimated)".into() },
                SignalWeight { signal: "favorite".into(), weight: 1.0, ratio_to_like: "1x (baseline)".into() },
                SignalWeight { signal: "not_interested".into(), weight: -20.0, ratio_to_like: "~-20x (estimated)".into() },
                SignalWeight { signal: "mute_author".into(), weight: -40.0, ratio_to_like: "~-40x (estimated)".into() },
                SignalWeight { signal: "block_author".into(), weight: -74.0, ratio_to_like: "~-74x (estimated)".into() },
                SignalWeight { signal: "report".into(), weight: -369.0, ratio_to_like: "~-369x (estimated)".into() },
            ],
            time_decay_halflife_minutes: 0, // Not published in 2026 code — removed from agent-info
            out_of_network_reply_penalty: 0.0, // Replaced by OON_WEIGHT_FACTOR (multiplicative, value unpublished)
            media_hierarchy: vec![
                "text (highest avg engagement)".into(),
                "native_image (triggers photo_expand_score)".into(),
                "native_video (requires MIN_VIDEO_DURATION_MS for vqv_score)".into(),
                "thread (maximises continuous dwell_time)".into(),
            ],
            best_posting_hours: "9-11 AM local time (empirical)".into(),
            best_posting_days: "Tuesday, Wednesday, Thursday (empirical)".into(),
        },
        measurement_coverage: MeasurementCoverage {
            measurable: vec![
                "favorite".into(), "retweet".into(), "reply".into(),
                "quote".into(), "impressions".into(), "bookmarks".into(),
                "profile_click".into(),
            ],
            proxy_only: vec![
                ProxySignal { signal: "follow_author".into(), proxy_method: "profile_click correlation".into(), confidence: "low".into() },
                ProxySignal { signal: "share_via_dm".into(), proxy_method: "save-worthy content heuristics".into(), confidence: "medium".into() },
                ProxySignal { signal: "share_via_copy_link".into(), proxy_method: "quotability heuristics".into(), confidence: "medium".into() },
                ProxySignal { signal: "dwell".into(), proxy_method: "word count + line breaks".into(), confidence: "high".into() },
                ProxySignal { signal: "photo_expand".into(), proxy_method: "media attachment detection".into(), confidence: "high".into() },
                ProxySignal { signal: "negative_risk".into(), proxy_method: "sentiment + combative tone analysis".into(), confidence: "medium".into() },
            ],
            blind: vec![
                "report".into(), "block_author".into(), "mute_author".into(),
                "not_interested".into(), "vqv".into(),
                "click".into(), "quoted_click".into(),
                "good_click".into(), "cont_dwell_time".into(),
            ],
        },
        usage_hints: vec![
            "Always run 'xmaster analyze' before posting — it checks for common issues that hurt reach".into(),
            "Use 'xmaster search-ai' over 'xmaster search' — cheaper and smarter (xAI vs X API). Supports from:username for hard author filtering (e.g. 'xmaster search-ai \"from:elonmusk AI\"')".into(),
            "Reply to larger accounts in your niche — replies are a high-value signal (estimated ~20x a like)".into(),
            "Create content people want to DM to friends — DM shares are estimated ~25x a like".into(),
            "Never put external links in the main tweet body — put them in the first reply".into(),
            "Space posts 2+ hours apart — the feed diversifies repeated authors".into(),
            "Use 'xmaster timeline --sort impressions' to find your best-performing posts".into(),
            "Use 'xmaster timeline --since 24h' to check recent post performance".into(),
            "Use 'xmaster engage recommend --topic \"your niche\"' to find high-ROI reply targets".into(),
            "Use 'xmaster config get style.voice' to read the current voice before updating it — adapt, don't replace".into(),
            "Every search/timeline/read automatically builds your local post library — use 'xmaster inspire --topic X' to browse it".into(),
            "Set account.premium to true if you have X Premium — unlocks 25k char limit instead of 280".into(),
            // ── metrics & tracking — agent-friendly batch patterns ──
            "BATCH METRICS: 'xmaster metrics ID1 ID2 ID3 ...' fetches multiple tweets in ONE HTTP call (up to 100 IDs). NEVER loop per-id — it wastes tool calls and rate limits".into(),
            "Every metrics call returns pre-computed age_seconds, age_human ('9 min'), delta vs previous snapshot (+imps since last check), and velocity (imps/min). Top-level 'now' field gives server time. The agent should NOT do clock arithmetic — the CLI already did it".into(),
            "Each metrics call auto-saves a snapshot to the local DB. The NEXT call gets a delta for free — so running metrics twice with a gap is the cheapest way to see a trend".into(),
            "Use 'xmaster track run' to batch-snapshot all your recent posts and check for reply-backs on pending replies — one command replaces N metrics calls when auditing recent activity".into(),
            // ── engagement: watchlist auto-tracking ──
            "Use 'xmaster engage watchlist add <username> --topic \"your niche\"' to pin important accounts without following. 'xmaster engage watchlist list' shows them. Watchlist accounts are prioritised by 'engage feed'".into(),
            "'xmaster engage feed \"your niche\"' checks watchlist accounts first (saves API calls) and silently auto-adds high-follower discovered authors (>=10k) back into the watchlist — so the watchlist grows itself as you discover targets".into(),
            "After replying to targets, run 'xmaster track run' to capture reply-backs (when the target replies to you). The reply-back signal is tracked in engagement_actions and surfaces in 'engage recommend'".into(),
            "'xmaster track run' now AUTO-PROMOTES hot reply targets into the watchlist — any account where your reply got >=100 imps, >=1 profile click, or a reply-back (last 14d, >=1k followers) is added automatically. Check 'watchlist_auto_promoted' in the track run metadata to see who got added".into(),
            "'xmaster engage hot-targets --days 7 --json' ranks the accounts you've replied to by avg impressions, profile clicks, and reply-back rate. Use it to find which targets reward your reply effort the most, then prioritise re-engaging them".into(),
            "Use 'xmaster likers <id>' / 'xmaster retweeters <id>' / 'xmaster quotes <id>' to inspect who engaged with a specific post — each returns a clean user/tweet list. 'quotes' also caches the quote tweets into your discovered_posts library for later 'xmaster inspire' browsing".into(),
            "Batch-lookup many users at once with 'xmaster users alice bob carol' — one HTTP call for up to 100 usernames. Use this whenever you need to hydrate a list of accounts; never loop per-user".into(),
            "'xmaster lists members <list_id>' returns the users in a given list (max 100 per call). Useful for auditing community membership or extracting target sets from curated lists".into(),
        ],
        handoffs: vec![
            Handoff {
                after_command: "post".into(),
                // track run FIRST — batch-snapshots all your recent posts + checks reply-backs.
                // metrics <id> is the fallback for spot-checking one specific tweet.
                next_commands: vec![
                    "xmaster track run".into(),
                    "xmaster metrics <id>".into(),
                ],
                reason: "Batch-snapshot recent posts and catch reply-backs in one command. Use metrics <id> only for a deep dive on one specific tweet".into(),
            },
            Handoff {
                after_command: "reply".into(),
                next_commands: vec![
                    "xmaster track run".into(),
                    "xmaster metrics <reply_id>".into(),
                ],
                reason: "track run snapshots the new reply AND polls the target for a reply-back. metrics <reply_id> gives delta+velocity on this specific reply".into(),
            },
            Handoff {
                after_command: "metrics".into(),
                // Hint agents to batch next time: if they want to check more posts, use one call.
                next_commands: vec![
                    "xmaster metrics ID1 ID2 ID3 ...".into(),
                    "xmaster track run".into(),
                ],
                reason: "metrics accepts multiple IDs in one call — never loop per-id. For auditing recent activity, track run is faster still".into(),
            },
            Handoff {
                after_command: "analyze".into(),
                next_commands: vec!["xmaster post \"...\"".into(), "xmaster schedule add \"...\" --at auto".into()],
                reason: "Post the optimized content or schedule it for the best time".into(),
            },
            Handoff {
                after_command: "schedule add".into(),
                next_commands: vec!["xmaster schedule list".into()],
                reason: "Confirm the post is queued at the right time".into(),
            },
            Handoff {
                after_command: "engage recommend".into(),
                next_commands: vec![
                    "xmaster reply <id> \"...\"".into(),
                    "xmaster like <id>".into(),
                    "xmaster engage watchlist add <username>".into(),
                ],
                reason: "Act on the recommended engagement targets — add proven reciprocators to your watchlist".into(),
            },
            Handoff {
                after_command: "engage feed".into(),
                next_commands: vec![
                    "xmaster reply <id> \"...\"".into(),
                    "xmaster like <id>".into(),
                    "xmaster engage watchlist list".into(),
                ],
                reason: "Engage with the curated feed, then check which accounts feed silently auto-added to your watchlist".into(),
            },
            Handoff {
                after_command: "track run".into(),
                next_commands: vec![
                    "xmaster engage hot-targets --days 7".into(),
                    "xmaster engage watchlist list".into(),
                ],
                reason: "track run auto-promotes hot reply targets. Check hot-targets to see the full ranking and watchlist list to confirm the new entries".into(),
            },
            Handoff {
                after_command: "engage hot-targets".into(),
                next_commands: vec![
                    "xmaster engage feed".into(),
                    "xmaster reply <id> \"...\"".into(),
                ],
                reason: "Once you know which targets reward your replies most, fetch fresh posts from them via engage feed and reply immediately".into(),
            },
        ],
        writing_style: style,
    };
    output::render(format, &info, None);
}
