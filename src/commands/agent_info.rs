use crate::config;
use crate::output::{self, OutputFormat, Tableable};
use serde::Serialize;

#[derive(Serialize)]
struct ExitCodes {
    success: &'static str,
    transient: &'static str,
    config: &'static str,
    bad_input: &'static str,
    rate_limited: &'static str,
}

#[derive(Serialize)]
struct Envelope {
    version: &'static str,
    success: &'static str,
    error: &'static str,
}

#[derive(Serialize)]
struct AgentInfo {
    name: String,
    version: String,
    description: String,
    commands: Vec<String>,
    capabilities: Vec<String>,
    env_prefix: String,
    config_path: String,
    auto_json_when_piped: bool,
    exit_codes: ExitCodes,
    envelope: Envelope,
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
            "bookmarks folders".into(), "bookmarks folder".into(),
            // Lists
            "lists create".into(), "lists delete".into(), "lists add".into(),
            "lists remove".into(), "lists timeline".into(), "lists members".into(),
            "lists mine".into(),
            // Intelligence
            "analyze".into(), "engage recommend".into(), "engage feed".into(),
            "engage swarm".into(), "engage hot-targets".into(),
            "engage watchlist add".into(), "engage watchlist list".into(), "engage watchlist remove".into(),
            "likers".into(), "retweeters".into(), "quotes".into(), "users".into(),
            "amplifiers".into(), "volume".into(),
            "track run".into(), "track status".into(),
            "track followers".into(), "track growth".into(),
            "report daily".into(), "report weekly".into(),
            "suggest best-time".into(), "suggest next-post".into(),
            // Scheduling
            "schedule add".into(), "schedule list".into(), "schedule cancel".into(),
            "schedule reschedule".into(), "schedule fire".into(), "schedule setup".into(),
            // System
            "config show".into(), "config get".into(), "config set".into(), "config check".into(),
            "config auth".into(), "config guide".into(), "config web-login".into(),
            "inspire".into(),
            "rate-limits".into(), "agent-info".into(), "update".into(),
            "skill install".into(), "skill update".into(), "skill status".into(),
            "star".into(),
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
        auto_json_when_piped: true,
        exit_codes: ExitCodes {
            success: "0: Success — continue",
            transient: "1: Transient error (IO, network) — retry with backoff",
            config: "2: Config error (missing key, bad file) — fix setup, do not retry",
            bad_input: "3: Bad input (invalid args, denied command) — fix arguments",
            rate_limited: "4: Rate limited — wait, then retry",
        },
        envelope: Envelope {
            version: "1",
            success: "{ version, status: \"success\", data, metadata }",
            error: "{ version, status: \"error\", error: { code, message, suggestion } }",
        },
        algorithm: AlgorithmInfo {
            source: "xai-org/x-algorithm (January 2026 release, Grok-based transformer in Rust). Weights are in the unpublished `params` module (excluded 'for security reasons'). Numeric values below are community estimates — the signal LIST is confirmed from home-mixer/scorers/weighted_scorer.rs but magnitudes are NOT.".into(),
            weights: vec![
                // The 19 signals from weighted_scorer.rs, ordered by estimated impact.
                // reply_engaged_by_author (+75 from empirical sources) is not a separate
                // Phoenix score but the combined effect of the reply being in conversation
                // context with the author — it amplifies both reply_score and dwell_score.
                SignalWeight { signal: "follow_author".into(), weight: 30.0, ratio_to_like: "~60x (estimated, highest positive)" .into() },
                SignalWeight { signal: "share_via_dm".into(), weight: 25.0, ratio_to_like: "~50x (estimated)".into() },
                SignalWeight { signal: "share_via_copy_link".into(), weight: 20.0, ratio_to_like: "~40x (estimated)".into() },
                SignalWeight { signal: "share".into(), weight: 15.0, ratio_to_like: "~30x (estimated)".into() },
                SignalWeight { signal: "reply".into(), weight: 13.5, ratio_to_like: "~27x (estimated, reply_engaged_by_author = ~150x)".into() },
                SignalWeight { signal: "profile_click".into(), weight: 12.0, ratio_to_like: "~24x (estimated)".into() },
                SignalWeight { signal: "click".into(), weight: 11.0, ratio_to_like: "~22x (estimated)".into() },
                SignalWeight { signal: "dwell".into(), weight: 10.0, ratio_to_like: "~20x (estimated, binary 2+ min threshold)".into() },
                SignalWeight { signal: "cont_dwell_time".into(), weight: 8.0, ratio_to_like: "~16x (estimated, continuous seconds)".into() },
                SignalWeight { signal: "quote".into(), weight: 8.0, ratio_to_like: "~16x (estimated, separate from retweet in 2026)".into() },
                SignalWeight { signal: "quoted_click".into(), weight: 5.0, ratio_to_like: "~10x (estimated, click into quoted tweet)".into() },
                SignalWeight { signal: "photo_expand".into(), weight: 4.0, ratio_to_like: "~8x (estimated)".into() },
                SignalWeight { signal: "vqv".into(), weight: 3.0, ratio_to_like: "~6x (estimated, gated by MIN_VIDEO_DURATION_MS)".into() },
                SignalWeight { signal: "retweet".into(), weight: 1.0, ratio_to_like: "~2x (estimated)".into() },
                SignalWeight { signal: "favorite".into(), weight: 0.5, ratio_to_like: "1x (baseline)".into() },
                SignalWeight { signal: "not_interested".into(), weight: -20.0, ratio_to_like: "~-40x (estimated)".into() },
                SignalWeight { signal: "mute_author".into(), weight: -40.0, ratio_to_like: "~-80x (estimated)".into() },
                SignalWeight { signal: "block_author".into(), weight: -74.0, ratio_to_like: "~-148x (estimated)".into() },
                SignalWeight { signal: "report".into(), weight: -369.0, ratio_to_like: "~-738x (estimated)".into() },
            ],
            time_decay_halflife_minutes: 360, // ~6h half-life from empirical analysis; hard cutoff in first 30-60 min via Phoenix distribution gate
            out_of_network_reply_penalty: 0.0, // Replaced by OON_WEIGHT_FACTOR (multiplicative, value unpublished) in oon_scorer.rs
            media_hierarchy: vec![
                "text (highest avg engagement, maximises dwell + reply probability)".into(),
                "native_image (triggers photo_expand_score)".into(),
                "native_video (requires MIN_VIDEO_DURATION_MS for vqv_score)".into(),
                "thread (maximises continuous cont_dwell_time_weight)".into(),
            ],
            best_posting_hours: "8-11 AM local time (empirical, aligns with engagement velocity gate)".into(),
            best_posting_days: "Tuesday, Wednesday, Thursday (empirical)".into(),
        },
        measurement_coverage: MeasurementCoverage {
            measurable: vec![
                "favorite".into(), "retweet".into(), "reply".into(),
                "quote".into(), "impressions".into(), "bookmarks".into(),
                "profile_click".into(), "url_clicks".into(),
            ],
            proxy_only: vec![
                ProxySignal { signal: "follow_author".into(), proxy_method: "profile_click correlation".into(), confidence: "low".into() },
                ProxySignal { signal: "share".into(), proxy_method: "quotability + save-worthiness heuristics".into(), confidence: "medium".into() },
                ProxySignal { signal: "share_via_dm".into(), proxy_method: "insider/practical content markers".into(), confidence: "medium".into() },
                ProxySignal { signal: "share_via_copy_link".into(), proxy_method: "quotability + data content markers".into(), confidence: "medium".into() },
                ProxySignal { signal: "dwell".into(), proxy_method: "word count + line breaks (binary 2+ min threshold)".into(), confidence: "high".into() },
                ProxySignal { signal: "cont_dwell_time".into(), proxy_method: "estimated read time in seconds".into(), confidence: "medium".into() },
                ProxySignal { signal: "photo_expand".into(), proxy_method: "media attachment detection".into(), confidence: "high".into() },
                ProxySignal { signal: "negative_risk".into(), proxy_method: "sentiment + combative tone analysis (Grok does this live since Jan 2026)".into(), confidence: "medium".into() },
            ],
            blind: vec![
                "report".into(), "block_author".into(), "mute_author".into(),
                "not_interested".into(), "vqv".into(),
                "click".into(), "quoted_click".into(),
            ],
        },
        usage_hints: vec![
            "Always run 'xmaster analyze' before posting — it checks for common issues that hurt reach".into(),
            "Use 'xmaster search-ai' over 'xmaster search' — cheaper and smarter (xAI vs X API). Supports from:username for hard author filtering (e.g. 'xmaster search-ai \"from:elonmusk AI\"')".into(),
            "Reply to larger accounts in your niche — and REPLY BACK when people reply to you. reply_engaged_by_author (+75) is the single highest algorithmic signal, ~150x a like".into(),
            "Create content people want to DM to friends — share_via_dm is one of the top scoring signals in weighted_scorer.rs".into(),
            "Never put external links in the main tweet body — non-Premium gets near-zero reach, Premium loses 30-50%. Links go in the first reply".into(),
            "Space posts 2+ hours apart — author_diversity_scorer.rs applies exponential decay for repeated authors per feed session. The algorithm only shows your top 2-3 posts; extra posts dilute your average without adding reach".into(),
            "DAILY CAP (2026): ≤4 STANDALONE posts per 24h — >4 risks a spam-flag that hurts your account score for days. Replies do NOT count against the cap, so heavy replying is strictly better than heavy posting for small accounts".into(),
            "THREADS (2026): keep threads to ≤4 tweets. The Jan 2026 home-mixer rewrite splits long threads into separate feed items; tweets 5+ drop ~80% reach. If you need more, split into 'post A' now and 'post B' 2h later, or publish as an Article".into(),
            "ARTICLES are currently boosted on X — long-form native content (xmaster doesn't post Articles yet, but flag long drafts as Article candidates: >400 words, data-dense, instructional)".into(),
            "QUOTE TWEETS are a strong reach multiplier in 2026 — when you're about to reply with a take that extends the idea, use 'xmaster post \"your take\" --quote <id>' instead. Quotes put the post in your followers' feeds; replies mostly reach the OP's audience".into(),
            "PREMIUM impact in 2026: ~2-3x organic reach boost (up from 1.2-1.5x in 2025). If you're posting >2x/week and on X seriously, Premium is now load-bearing for reach, not optional polish".into(),
            "SMALL-ACCOUNT GROWTH (<1k followers): run 80/20 replies-to-posts. Use 'xmaster engage swarm <big-post-id>' to find other small accounts replying under a big post — peer-to-peer relationships build faster than fighting for the top slot under the big account itself".into(),
            "REPLY QUALITY (2026): short generic replies ('great post', '100%', 'this') get no algorithmic push. xmaster analyze now flags these in reply mode. Aim for 1-2 sentences with a specific observation the author would want to engage back on".into(),
            "The first 30-60 minutes are critical — Phoenix makes its biggest distribution decision in this window. Each post is shown to ~1500 candidates; if it doesn't get traction it stops being served. Time your posts when your audience is online".into(),
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
            "'xmaster amplifiers' shows who reposts your content. High-frequency reposters are your audience amplifiers — add them to your watchlist to nurture the relationship".into(),
            "XMASTER_ALLOW_COMMANDS and XMASTER_DENY_COMMANDS env vars restrict which commands agents can run. Deny takes precedence. agent-info, config, rate-limits, update, skill are always allowed".into(),
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
                    "xmaster post \"your take\" --quote <id>".into(),
                    "xmaster engage swarm <id>".into(),
                    "xmaster engage watchlist list".into(),
                ],
                reason: "Reply, quote-tweet (stronger reach than reply — puts the post in your followers' feed), or swarm the replies for peer-to-peer small-account engagement".into(),
            },
            Handoff {
                after_command: "engage swarm".into(),
                next_commands: vec![
                    "xmaster reply <reply_id> \"...\"".into(),
                    "xmaster like <reply_id>".into(),
                    "xmaster follow <username>".into(),
                ],
                reason: "Swarm targets are small-to-mid accounts already engaged with the big post. Replying peer-to-peer builds reciprocal relationships faster than competing for the top slot under the big account".into(),
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
