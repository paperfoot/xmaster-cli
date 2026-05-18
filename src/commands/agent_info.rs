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
    source: &'static str,
    signals: Vec<AlgorithmSignal>,
    media_hierarchy: Vec<&'static str>,
    best_posting_hours: &'static str,
    best_posting_days: &'static str,
}

#[derive(Serialize)]
struct AlgorithmSignal {
    signal: &'static str,
    polarity: &'static str, // "positive" | "continuous" | "negative"
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
        table.add_row(vec!["Algorithm Source", self.algorithm.source]);
        let pos = self.algorithm.signals.iter().filter(|s| s.polarity == "positive").count();
        let cont = self.algorithm.signals.iter().filter(|s| s.polarity == "continuous").count();
        let neg = self.algorithm.signals.iter().filter(|s| s.polarity == "negative").count();
        table.add_row(vec!["Signals", &format!("{} total ({pos} positive, {cont} continuous, {neg} negative)", self.algorithm.signals.len())]);
        table.add_row(vec!["Best Times", self.algorithm.best_posting_hours]);
        table.add_row(vec!["Best Days", self.algorithm.best_posting_days]);
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
            "post".into(), "reply".into(), "thread".into(), "article preview".into(), "article draft".into(), "delete".into(),
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
            "analyze".into(), "engage inbox".into(), "engage recommend".into(), "engage feed".into(),
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
            "conversation_inbox".into(),
            "article_preview".into(),
            "article_draft".into(),
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
            source: "xai-org/x-algorithm (May 15, 2026, home-mixer/scorers/ranking_scorer.rs). Weight magnitudes are runtime params, not public.",
            signals: vec![
                AlgorithmSignal { signal: "follow_author",         polarity: "positive" },
                AlgorithmSignal { signal: "share_via_dm",          polarity: "positive" },
                AlgorithmSignal { signal: "share_via_copy_link",   polarity: "positive" },
                AlgorithmSignal { signal: "share",                 polarity: "positive" },
                AlgorithmSignal { signal: "reply",                 polarity: "positive" },
                AlgorithmSignal { signal: "profile_click",         polarity: "positive" },
                AlgorithmSignal { signal: "click",                 polarity: "positive" },
                AlgorithmSignal { signal: "quote",                 polarity: "positive" },
                AlgorithmSignal { signal: "quoted_click",          polarity: "positive" },
                AlgorithmSignal { signal: "photo_expand",          polarity: "positive" },
                AlgorithmSignal { signal: "vqv",                   polarity: "positive" },
                AlgorithmSignal { signal: "retweet",               polarity: "positive" },
                AlgorithmSignal { signal: "favorite",              polarity: "positive" },
                AlgorithmSignal { signal: "dwell",                 polarity: "continuous" },
                AlgorithmSignal { signal: "cont_dwell_time",       polarity: "continuous" },
                AlgorithmSignal { signal: "cont_click_dwell_time", polarity: "continuous" },
                AlgorithmSignal { signal: "quoted_vqv",            polarity: "continuous" },
                AlgorithmSignal { signal: "not_dwelled",           polarity: "negative" },
                AlgorithmSignal { signal: "not_interested",        polarity: "negative" },
                AlgorithmSignal { signal: "mute_author",           polarity: "negative" },
                AlgorithmSignal { signal: "block_author",          polarity: "negative" },
                AlgorithmSignal { signal: "report",                polarity: "negative" },
            ],
            media_hierarchy: vec![
                "text",
                "native_image",
                "native_video (>= MIN_VIDEO_DURATION_MS for vqv)",
                "thread",
            ],
            best_posting_hours: "8-11 AM local time",
            best_posting_days: "Tuesday, Wednesday, Thursday",
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
                ProxySignal { signal: "dwell".into(), proxy_method: "word count + line breaks".into(), confidence: "high".into() },
                ProxySignal { signal: "cont_dwell_time".into(), proxy_method: "estimated read time in seconds".into(), confidence: "medium".into() },
                ProxySignal { signal: "photo_expand".into(), proxy_method: "media attachment detection".into(), confidence: "high".into() },
                ProxySignal { signal: "negative_risk".into(), proxy_method: "sentiment + combative tone analysis".into(), confidence: "medium".into() },
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
            "Reply to larger accounts in your niche, and reply back when people reply to you — author reply-back is a strong reciprocity loop".into(),
            "Create content people want to DM to friends — share_via_dm is a top scoring signal".into(),
            "Never put external links in the main tweet body — links reduce reach. Put them in the first reply".into(),
            "Space posts 2+ hours apart — author_diversity_scorer.rs decays repeated authors per feed pass".into(),
            "DAILY CAP: ≤4 standalone posts per 24h. Replies don't count — heavy replying beats heavy posting for small accounts".into(),
            "THREADS: dedup_conversation_filter.rs keeps only the highest-scored tweet per conversation. Long threads dilute their own hook. Keep to ≤4, or split into standalones".into(),
            "ARTICLES: a strong long-form format. Winners (Jan 2026 $1M Article Contest: @beaverd, @KobeissiLetter, @thedankoe) all hit: timely angle + named subjects + specific numbers + clear payoff".into(),
            "LONG-FORM POSTS (>280 chars, Premium): xmaster auto-routes through CreateNoteTweet for Premium accounts. 500-2000 char sweet spot. Beyond 2000, prefer Articles. 'xmaster analyze' runs long-form preview-hook, scannability, and payoff-density checks".into(),
            "LONG-FORM STRUCTURE: hook in first 280 chars (only this surfaces in the feed before 'show more'), then short paragraphs (2-4 lines), data points or named subjects every ~500 chars, payoff/CTA at end. xmaster analyze flags weak-preview, wall-of-text, low-density issues for long drafts".into(),
            "LONG-FORM EXEMPLARS: 'xmaster inspire --long' surfaces high-impression long-form posts from your local library. Seed it by running 'xmaster timeline --user @beaverd' (etc.) on long-form practitioners — every search/timeline auto-populates discovered_posts".into(),
            "LONG-FORM COVER IMAGES: generate via the nanaban CLI: `nanaban \"<title-card prompt>\" --ar 3:2`".into(),
            "ARTICLE PREVIEW: Articles are NOT long posts / Note Tweets. Render locally with `xmaster article preview draft.md --header-image cover.png --author \"Name\" --handle username -o preview.html`. Markdown maps to the official Article feature set (headings, bold, italic, lists, images, video/GIF, embedded posts/Articles, links)".into(),
            "ARTICLE DRAFTS: `xmaster article draft draft.md --header-image cover.png` saves an unpublished Article draft via the private web endpoint (`ArticleEntityDraftCreate`). Requires `xmaster config web-login` for cookies; NOT `/2/tweets` and NOT CreateNoteTweet".into(),
            "LONG-FORM TIMING: post in the in-network peak (Tue–Thu, 9–11 AM author-local for desk audiences; 7–9 PM for general). Be ready to engage replies in the first 30 min. Avoid Mon mornings (timeline rebuild) and Fri afternoon (drop-off)".into(),
            "QUOTE TWEETS: quotes enter the in-network candidate pool of YOUR followers (thunder_source.rs serves from following_user_ids). Replies travel through the OP's audience pool. Use 'xmaster post \"your take\" --quote <id>' when extending an idea".into(),
            "PREMIUM: ~2-3x organic reach in practice. The mechanism is not in the open source but the effect is real".into(),
            "SMALL-ACCOUNT GROWTH (<1k followers): replies-heavy. Grox's spam classifier (grox/tasks/task_spam_detection.py) buckets reply chains by BOTH parent and root author follower count (≤100/≤500/≤1000/>1000) — replies under small-account chains carry higher spam risk. Prefer replying to LARGER accounts in your niche; quality-gate every reply".into(),
            "REPLY QUALITY: short generic replies ('great post', '100%', 'this') score poorly on Grok's reply-ranking classifier and rarely earn a reply-back. xmaster analyze flags these in reply mode".into(),
            "FIRST 30-60 MINUTES: initial traction window matters — Phoenix age enters as learned post-age buckets. Time posts when your audience is online".into(),
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
            "'xmaster track run' also surfaces CROSS-POST CANDIDATES — standalone posts that crossed 5k impressions in the last 14d. Check 'cross_post_candidates' in track-run metadata. Pattern from @jackmoses777 (Apr 2026): screenshot a high-perf X text post, upload to Instagram as a still. 198K X views → 325K IG views / 8K followers in one case. Use the clinstagram skill to automate the upload".into(),
            "'xmaster engage hot-targets --days 7 --json' ranks the accounts you've replied to by avg impressions, profile clicks, and reply-back rate. Use it to find which targets reward your reply effort the most, then prioritise re-engaging them".into(),
            "Use 'xmaster likers <id>' / 'xmaster retweeters <id>' / 'xmaster quotes <id>' to inspect who engaged with a specific post — each returns a clean user/tweet list. 'quotes' also caches the quote tweets into your discovered_posts library for later 'xmaster inspire' browsing".into(),
            "Use 'xmaster engage inbox <id>' after publishing or when a post is moving. It checks root replies, quote tweets, and replies under quote tweets in one pass, then ranks where an author reply-back is likely to matter most. This catches the 'comments in reposts' surface that plain metrics/replies checks miss".into(),
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
                    "xmaster engage inbox <id>".into(),
                    "xmaster track run".into(),
                    "xmaster metrics <id>".into(),
                ],
                reason: "Check the conversation inbox first when a post is moving, then batch-snapshot recent posts. Use metrics <id> only for a deep dive on one specific tweet".into(),
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
                    "xmaster engage inbox <id>".into(),
                    "xmaster metrics ID1 ID2 ID3 ...".into(),
                    "xmaster track run".into(),
                ],
                reason: "If engagement is moving, inspect replies/quotes/quote-comments with engage inbox. For more post metrics, batch IDs in one metrics call".into(),
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
