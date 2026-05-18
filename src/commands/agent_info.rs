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
        table.add_row(vec!["Top Signal", "Follow from post, DM share, Reply (ordering only — magnitudes are runtime params)"]);
        table.add_row(vec!["Signals", "22 total (17 positive/continuous + 5 negative) — magnitudes all runtime-params; numbers shown are 2023-leak estimates"]);
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
            source: "xai-org/x-algorithm (May 15, 2026 release, Grok-based transformer). The signal LIST is confirmed from home-mixer/scorers/ranking_scorer.rs (22 weighted terms: 17 positive/continuous + 5 negative). EVERY MAGNITUDE BELOW IS A 2023-LEAK-ERA HISTORICAL ESTIMATE kept for relative ordering only — production weights are fetched at request time via the unpublished xai_feature_switches::Params module and are A/B-tested live with no stable public value.".into(),
            weights: vec![
                // 22 scorer terms from home-mixer/scorers/ranking_scorer.rs (May 15, 2026).
                // Magnitudes are HISTORICAL 2023-leak estimates, not live X production values.
                // The 2023 reply_engaged_by_author signal is ABSENT from the May 2026 source —
                // do not use it as a tactical anchor.
                SignalWeight { signal: "follow_author".into(), weight: 30.0, ratio_to_like: "historical 2023 estimate; live weight runtime-param".into() },
                SignalWeight { signal: "share_via_dm".into(), weight: 25.0, ratio_to_like: "historical 2023 estimate; live weight runtime-param".into() },
                SignalWeight { signal: "share_via_copy_link".into(), weight: 20.0, ratio_to_like: "historical 2023 estimate; live weight runtime-param".into() },
                SignalWeight { signal: "share".into(), weight: 15.0, ratio_to_like: "historical 2023 estimate; live weight runtime-param".into() },
                SignalWeight { signal: "reply".into(), weight: 13.5, ratio_to_like: "historical 2023 estimate; live weight runtime-param".into() },
                SignalWeight { signal: "profile_click".into(), weight: 12.0, ratio_to_like: "historical 2023 estimate; live weight runtime-param".into() },
                SignalWeight { signal: "click".into(), weight: 11.0, ratio_to_like: "historical 2023 estimate; live weight runtime-param".into() },
                SignalWeight { signal: "dwell".into(), weight: 10.0, ratio_to_like: "historical 2023 estimate; live weight runtime-param".into() },
                SignalWeight { signal: "cont_dwell_time".into(), weight: 8.0, ratio_to_like: "historical 2023 estimate; continuous in May 2026 scorer".into() },
                SignalWeight { signal: "cont_click_dwell_time".into(), weight: 6.0, ratio_to_like: "May 2026 scorer term, no 2023 anchor; runtime-param".into() },
                SignalWeight { signal: "quote".into(), weight: 8.0, ratio_to_like: "historical 2023 estimate; live weight runtime-param".into() },
                SignalWeight { signal: "quoted_click".into(), weight: 5.0, ratio_to_like: "historical 2023 estimate; live weight runtime-param".into() },
                SignalWeight { signal: "quoted_vqv".into(), weight: 3.0, ratio_to_like: "May 2026 scorer term (quoted video quality view); runtime-param".into() },
                SignalWeight { signal: "photo_expand".into(), weight: 4.0, ratio_to_like: "historical 2023 estimate; live weight runtime-param".into() },
                SignalWeight { signal: "vqv".into(), weight: 3.0, ratio_to_like: "historical 2023 estimate; gated by MIN_VIDEO_DURATION_MS".into() },
                SignalWeight { signal: "retweet".into(), weight: 1.0, ratio_to_like: "historical 2023 estimate; live weight runtime-param".into() },
                SignalWeight { signal: "favorite".into(), weight: 0.5, ratio_to_like: "1x (baseline; demo pipeline uses 1.0 in run_pipeline.py:355-360)".into() },
                SignalWeight { signal: "not_dwelled".into(), weight: -10.0, ratio_to_like: "May 2026 scorer NEGATIVE term (predicted scroll-past); runtime-param".into() },
                SignalWeight { signal: "not_interested".into(), weight: -20.0, ratio_to_like: "historical 2023 estimate; live weight runtime-param".into() },
                SignalWeight { signal: "mute_author".into(), weight: -40.0, ratio_to_like: "historical 2023 estimate; live weight runtime-param".into() },
                SignalWeight { signal: "block_author".into(), weight: -74.0, ratio_to_like: "historical 2023 estimate; live weight runtime-param".into() },
                SignalWeight { signal: "report".into(), weight: -369.0, ratio_to_like: "historical 2023 estimate; live weight runtime-param".into() },
            ],
            time_decay_halflife_minutes: 0, // No half-life function exists in the May 2026 source. home-mixer/filters/age_filter.rs is a binary Duration cutoff; phoenix/recsys_model.py:33-55,590-604 shows learned post-age buckets (60-min granularity, 4800-min max). Empirical feed decay (~6h) is not algorithmic.
            out_of_network_reply_penalty: 0.0, // Replaced by OonWeightFactor / TopicOonWeightFactor / NEW_USER_OON_WEIGHT_FACTOR (multiplicative, values unpublished) in home-mixer/scorers/oon_scorer.rs + ranking_scorer.rs:220-239
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
            "Reply to larger accounts in your niche — and REPLY BACK when people reply to you. Author reply-back is an empirically strong reciprocity loop, but the May 15 2026 ranking_scorer.rs does NOT contain a `reply_engaged_by_author` term — treat reply-back as a community/relationship heuristic, not a published weight".into(),
            "Create content people want to DM to friends — share_via_dm is one of the top scoring signals in weighted_scorer.rs".into(),
            "Never put external links in the main tweet body — non-Premium gets near-zero reach, Premium loses 30-50%. Links go in the first reply".into(),
            "Space posts 2+ hours apart — home-mixer/scorers/author_diversity_scorer.rs applies exponential decay (1-floor)*decay^position+floor for repeated authors. The decay/floor values are runtime params (unpublished); empirically this means your 3rd+ post in a feed session is heavily discounted, but the '2-3 posts per session' specific cap is not in code".into(),
            "DAILY CAP (empirical, not in code): ≤4 STANDALONE posts per 24h is xmaster's editorial heuristic — no daily-cap rule exists in the May 15 2026 source. Replies do NOT count against this heuristic. For small accounts, heavy replying is empirically better than heavy posting".into(),
            "THREADS (May 2026 mechanism): home-mixer/filters/dedup_conversation_filter.rs keeps only the HIGHEST-SCORED tweet per conversation per viewer. Long self-threads dilute their own best candidate — every extra tweet is another candidate competing for the same conversation slot. '~80% drop on tweet 5+' is an empirical pattern, not a coded per-position rule. Keep threads ≤4 or split into separate standalone posts".into(),
            "ARTICLES (empirical, not in code): no Article-specific scoring is in the open-source release. xmaster's long-form heuristics are editorial advice based on observed contest-winning posts (Jan 2026 $1M Article Contest: @beaverd $1M, @KobeissiLetter $500K, @thedankoe $250K — pattern: timely angle + named subjects + specific numbers + clear payoff)".into(),
            "LONG-FORM POSTS (>280 chars, Premium): xmaster auto-routes through CreateNoteTweet for Premium accounts. 500-2000 char sweet spot is EMPIRICAL (no length-scoring in the open source). Beyond 2000, prefer Articles (no coded boost, just contest-pattern observation). 'xmaster analyze' runs long-form preview-hook, scannability, and payoff-density checks".into(),
            "LONG-FORM STRUCTURE (2026 winners): hook in first 280 chars (that's all the feed shows before 'show more'), then short paragraphs (2-4 lines), data points or named subjects every ~500 chars, payoff/CTA at the end. Wall-of-text dies — readers scan, not read. xmaster analyze flags weak-preview, wall-of-text, and low-density issues for long drafts".into(),
            "LONG-FORM EXEMPLARS: 'xmaster inspire --long' surfaces high-impression long-form posts already in your discovered library. Seed it by running 'xmaster search-ai' or 'xmaster timeline --user' on long-form practitioners (e.g. @beaverd, @KobeissiLetter, @thedankoe, @nickshirleyy, @wolfejosh, @ryanhallyall — 2026 contest winners) — every search/timeline auto-populates discovered_posts".into(),
            "LONG-FORM COVER IMAGES: X Articles render with a preview card; covers boost click-through. Generate one via the nanaban CLI (default model is gpt-image-2): `nanaban \"<prompt>\" --ar 3:2` for the standard preview-card aspect ratio. Keep prompts editorial — title-card style, not abstract art".into(),
            "ARTICLE PREVIEW: Articles are not long posts / Note Tweets. Use `xmaster article preview draft.md --header-image cover.png --author \"Name\" --handle username -o preview.html` to render the separate X Articles surface locally. Markdown maps to the official Article feature set: header image, headings/subheadings, bold, italic, strikethrough, indentation, numbered/bulleted lists, images, video/GIF directives, embedded posts/Articles, and links".into(),
            "ARTICLE DRAFTS: `xmaster article draft draft.md --header-image cover.png` saves a native, unpublished X Article draft through the current private web Article entity endpoint (`ArticleEntityDraftCreate`). It requires browser cookies from `xmaster config web-login`; it is not the public `/2/tweets` API and it is not CreateNoteTweet".into(),
            "LONG-FORM TIMING: post during the in-network peak (Tue–Thu, 9–11 AM author-local for desk audiences; 7–9 PM for general). Long-form needs more dwell, so a slow-feed window is fine — but the first 30-min velocity rule still applies, so be ready to engage replies. Avoid Mon mornings (timeline rebuild) and Fri afternoon (drop-off)".into(),
            "QUOTE TWEETS: a quote enters the in-network candidate pool of YOUR followers via home-mixer/sources/thunder_source.rs:33-43 (Thunder serves from following_user_ids). Replies mostly travel through the OP's audience pool. Use 'xmaster post \"your take\" --quote <id>' when extending an idea — the in-network reach is mechanistic, not just empirical".into(),
            "PREMIUM (empirical, not in code): xmaster's ~2-3x reach claim has NO algorithmic anchor in the open-source release. home-mixer/filters/ineligible_subscription_filter.rs is an ACCESS filter (gates paywalled content viewers can't see), not a score multiplier. Premium reach gains are real but the mechanism is invisible in the open source".into(),
            "SMALL-ACCOUNT GROWTH (<1k followers, empirical): run replies-heavy. Grox spam classifier at grox/tasks/task_spam_detection.py:17-29 buckets reply chains by BOTH parent and root author follower count (≤100/≤500/≤1000/>1000). A reply under a small-account chain is at higher spam risk than the same reply under a large-account root — so prefer replying to LARGER accounts in your niche. Use 'xmaster engage swarm <big-post-id>' to find peer-to-peer threads, but quality-gate every reply".into(),
            "REPLY QUALITY (empirical): short generic replies ('great post', '100%', 'this') get no algorithmic push. The May 15 2026 source has no literal phrase/length rule, but Grox has an LLM-based reply ranking classifier (grox/classifiers/content/reply_ranking.py) and a spam-comment classifier — generic replies score badly on both. xmaster analyze flags these in reply mode".into(),
            "FIRST 30-60 MINUTES (empirical, not in code): no distribution-gate stage exists in the open-source pipeline. Phoenix age enters as learned post-age buckets (60-min granularity, 4800-min max per phoenix/recsys_model.py:33-55), so recency IS a model feature but no specific '1500 candidates / 30-60 min window' rule ships with the open source. Treat as empirical pacing advice".into(),
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
