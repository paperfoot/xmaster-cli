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
    /// Source: twitter/the-algorithm-ml open-source code.
    algorithm: AlgorithmInfo,
    /// Hints for optimal usage — the CLI tells agents how to use it well.
    usage_hints: Vec<String>,
    /// User's writing style for X posts (only present when configured).
    #[serde(skip_serializing_if = "Option::is_none")]
    writing_style: Option<String>,
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
        table.add_row(vec!["Top Signal", "Reply + author reply (150x a like)"]);
        table.add_row(vec!["Time Decay", "50% visibility loss every 6 hours"]);
        table.add_row(vec!["Best Times", &self.algorithm.best_posting_hours]);
        table.add_row(vec!["Best Days", &self.algorithm.best_posting_days]);
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
            "post".into(), "delete".into(), "like".into(), "unlike".into(),
            "retweet".into(), "unretweet".into(), "bookmark".into(), "unbookmark".into(),
            "follow".into(), "unfollow".into(), "dm send".into(), "dm inbox".into(),
            "dm thread".into(), "timeline".into(), "mentions".into(), "search".into(),
            "search-ai".into(), "trending".into(), "user".into(), "me".into(),
            "bookmarks list".into(), "bookmarks sync".into(), "bookmarks search".into(),
            "bookmarks export".into(), "bookmarks digest".into(), "bookmarks stats".into(),
            "followers".into(), "following".into(),
            "thread".into(), "metrics".into(), "lists".into(),
            "hide-reply".into(), "unhide-reply".into(), "block".into(), "unblock".into(),
            "mute".into(), "unmute".into(), "rate-limits".into(),
            "analyze".into(), "track run".into(), "track status".into(),
            "report daily".into(), "report weekly".into(),
            "suggest best-time".into(), "suggest next-post".into(),
            "schedule add".into(), "schedule list".into(), "schedule cancel".into(),
            "schedule reschedule".into(), "schedule fire".into(), "schedule setup".into(),
            "engage recommend".into(),
            "config show".into(), "config set".into(), "config check".into(),
            "agent-info".into(), "update".into(),
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
            source: "twitter/the-algorithm-ml (open-source, verified Apr 2023, updated Sep 2025)".into(),
            weights: vec![
                SignalWeight { signal: "reply_author_engaged".into(), weight: 75.0, ratio_to_like: "150x".into() },
                SignalWeight { signal: "reply".into(), weight: 13.5, ratio_to_like: "27x".into() },
                SignalWeight { signal: "profile_click".into(), weight: 12.0, ratio_to_like: "24x".into() },
                SignalWeight { signal: "good_click".into(), weight: 11.0, ratio_to_like: "22x".into() },
                SignalWeight { signal: "retweet".into(), weight: 1.0, ratio_to_like: "2x".into() },
                SignalWeight { signal: "like".into(), weight: 0.5, ratio_to_like: "1x (baseline)".into() },
                SignalWeight { signal: "video_playback_50pct".into(), weight: 0.005, ratio_to_like: "~0".into() },
                SignalWeight { signal: "negative_feedback".into(), weight: -74.0, ratio_to_like: "-148x".into() },
                SignalWeight { signal: "report".into(), weight: -369.0, ratio_to_like: "-738x".into() },
            ],
            time_decay_halflife_minutes: 360,
            out_of_network_reply_penalty: -10.0,
            media_hierarchy: vec![
                "native_video".into(), "multiple_images".into(), "single_image".into(),
                "gif".into(), "external_link (lowest)".into(),
            ],
            best_posting_hours: "9-11 AM local time".into(),
            best_posting_days: "Tuesday, Wednesday, Thursday".into(),
        },
        usage_hints: vec![
            "Always run 'xmaster analyze' before posting — it scores your tweet against the algorithm".into(),
            "Use 'xmaster search-ai' over 'xmaster search' — cheaper and smarter (xAI vs X API)".into(),
            "Reply to your own commenters — conversations are worth 150x a like".into(),
            "Never put external links in the main tweet body — put them in the first reply".into(),
            "Check 'xmaster suggest next-post' before posting — avoid cannibalizing your own reach".into(),
            "Post threads for maximum growth — they get bookmarked and shared heavily".into(),
            "Check 'xmaster report weekly' to learn what's working".into(),
            "Use 'xmaster schedule add --at auto' to schedule posts at your historically best time".into(),
            "Run 'xmaster bookmarks sync' regularly to archive bookmarks — local copies survive tweet deletion".into(),
            "Use 'xmaster engage recommend --topic \"your niche\"' to find high-ROI reply targets — conversations are 150x a like".into(),
        ],
        writing_style: style,
    };
    output::render(format, &info, None);
}
