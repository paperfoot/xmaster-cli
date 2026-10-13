use clap::{Parser, Subcommand};

/// Root `--help` footer: the few facts that save an agent a wrong first call.
const HELP_FOOTER: &str = "\
Tips:
  - Piped output is compact JSON {version,status,data}; errors go to stderr as
    {error:{code,message,suggestion,retryable}} with exit codes 0-5
  - `xmaster agent-info`: every command on one line. `agent-info --command \"engage feed\"`:
    that command's arguments, flags, defaults and allowed values
  - Posts take an ID or URL; users take a handle, @handle or profile URL
  - Collections take -n N (also -c, --count, --limit)
  - post, reply and quote take --dry-run; XMASTER_READ_ONLY=1 blocks every write to X
  - `xmaster config web-login` once: free reads, and replies to any thread

Examples:
  xmaster engage feed \"longevity\" -n 5
  xmaster conversation https://x.com/user/status/1234567890
  xmaster reply 1234567890 \"The dose-response point is the key one\" --dry-run
  xmaster metrics 1234567890 1234567891";

#[derive(Parser)]
#[command(
    name = "xmaster",
    version,
    about = "X CLI for AI agents and people: read, post, reply anywhere, like, repost, search, DM",
    long_about = "X CLI for AI agents and people: read, post, reply anywhere, like, repost, search, DM.\nBuilt by 199 Biotechnologies.",
    after_long_help = HELP_FOOTER
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Force JSON (the default when piped)
    #[arg(long, global = true)]
    pub json: bool,

    /// CSV for list commands (timeline, search, replies, metrics, ...)
    #[arg(long, global = true)]
    pub csv: bool,

    /// Indented JSON (compact when piped, to save tokens)
    #[arg(long, global = true)]
    pub pretty: bool,

    /// Hide notices for people; data and errors still print
    #[arg(long, global = true)]
    pub quiet: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Post (text, media, reply, quote, poll)
    Post {
        /// Post text
        text: String,
        /// Reply to this post (ID or URL)
        #[arg(long, value_parser = post_id)]
        reply_to: Option<String>,
        /// Quote this post (ID or URL)
        #[arg(long, value_parser = post_id)]
        quote: Option<String>,
        /// Media file paths to attach
        #[arg(long, num_args = 1..=4)]
        media: Vec<String>,
        /// Poll options (comma-separated)
        #[arg(long)]
        poll: Option<String>,
        /// Poll duration in minutes (default 1440 = 24h)
        #[arg(long, default_value = "1440")]
        poll_duration: u64,
        /// Check the draft and show the route and cost; post nothing
        #[arg(long)]
        dry_run: bool,
    },

    /// Delete a post
    Delete {
        /// Post ID or URL
        #[arg(value_parser = post_id)]
        id: String,
    },

    /// Read a post: text, author, date, metrics and media in one call
    Read {
        /// Post ID or URL
        #[arg(value_parser = post_id)]
        id: String,
    },

    /// A post with its parent chain and top replies
    #[command(visible_alias = "convo")]
    Conversation {
        /// Post ID or URL
        #[arg(value_parser = post_id)]
        id: String,
        /// Max replies to include (0 for none)
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "10")]
        count: usize,
    },

    /// Like a post
    Like {
        /// Post ID or URL
        #[arg(value_parser = post_id)]
        id: String,
    },

    /// Unlike a post
    Unlike {
        /// Post ID or URL
        #[arg(value_parser = post_id)]
        id: String,
    },

    /// Repost a post
    #[command(visible_alias = "repost")]
    Retweet {
        /// Post ID or URL
        #[arg(value_parser = post_id)]
        id: String,
    },

    /// Undo a repost
    #[command(visible_alias = "unrepost")]
    Unretweet {
        /// Post ID or URL
        #[arg(value_parser = post_id)]
        id: String,
    },

    /// Bookmark a post
    Bookmark {
        /// Post ID or URL
        #[arg(value_parser = post_id)]
        id: String,
    },

    /// Remove a bookmark
    Unbookmark {
        /// Post ID or URL
        #[arg(value_parser = post_id)]
        id: String,
    },

    /// Follow a user
    Follow {
        /// Handle, @handle or profile URL
        #[arg(value_parser = handle)]
        username: String,
    },

    /// Unfollow a user
    Unfollow {
        /// Handle, @handle or profile URL
        #[arg(value_parser = handle)]
        username: String,
    },

    /// Direct messages
    Dm {
        #[command(subcommand)]
        action: DmCommands,
    },

    /// Home or user timeline
    Timeline {
        /// Handle (omit for home timeline)
        #[arg(long, value_parser = handle)]
        user: Option<String>,
        /// Number of posts
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "10")]
        count: usize,
        /// Only show posts after this time (e.g. "12h", "7d", or ISO 8601)
        #[arg(long)]
        since: Option<String>,
        /// Only show posts before this time (e.g. "12h", "7d", or ISO 8601)
        #[arg(long)]
        before: Option<String>,
        /// Sort by: impressions, likes, reposts, date (default: date)
        #[arg(long, value_parser = ["impressions", "views", "likes", "reposts", "retweets", "rts", "date"])]
        sort: Option<String>,
    },

    /// View your mentions
    Mentions {
        /// Number of mentions
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "10")]
        count: usize,
        /// Only show mentions after this post ID
        #[arg(long, value_parser = post_id)]
        since_id: Option<String>,
    },

    /// Search posts (X search operators work)
    Search {
        /// Search query. Supports X operators, incl. min_likes:/min_replies:/
        /// min_reposts: (since May 2026). Keyword search no longer returns reposts
        query: String,
        /// Search mode: recent (latest first) or top
        #[arg(long, default_value = "recent", value_parser = ["recent", "latest", "top", "relevancy", "relevant", "popular"])]
        mode: String,
        /// Number of results
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "10")]
        count: usize,
        /// Only show posts after this time (e.g. "12h", "7d", or ISO 8601)
        #[arg(long)]
        since: Option<String>,
        /// Only show posts before this time (e.g. "12h", "7d", or ISO 8601)
        #[arg(long)]
        before: Option<String>,
    },

    /// AI-powered search (xAI/Grok)
    SearchAi {
        /// Search query
        query: String,
        /// Number of results
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "10")]
        count: usize,
        /// Filter by date (from)
        #[arg(long)]
        from_date: Option<String>,
        /// Filter by date (to)
        #[arg(long)]
        to_date: Option<String>,
    },

    /// Get trending topics
    Trending {
        /// Region filter
        #[arg(long)]
        region: Option<String>,
        /// Category filter
        #[arg(long)]
        category: Option<String>,
        /// Use personalized trends (requires X Premium)
        #[arg(long)]
        personalized: bool,
    },

    /// A user's profile
    User {
        /// Handle, @handle or profile URL
        #[arg(value_parser = handle)]
        username: String,
    },

    /// Your own profile
    Me,

    /// Bookmarks and a local searchable archive
    Bookmarks {
        #[command(subcommand)]
        action: BookmarkCommands,
    },

    /// List followers
    Followers {
        /// Handle, @handle or profile URL
        #[arg(value_parser = handle)]
        username: String,
        /// Number of results
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "20")]
        count: usize,
    },

    /// List following
    Following {
        /// Handle, @handle or profile URL
        #[arg(value_parser = handle)]
        username: String,
        /// Number of results
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "20")]
        count: usize,
    },

    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: ConfigCommands,
    },

    /// Commands for agents, as raw JSON: one line each, or one in full with --command
    #[command(visible_alias = "info")]
    AgentInfo {
        /// One command or group in full: arguments, flags, defaults, allowed values
        /// (e.g. "engage feed", "engage", "repost")
        #[arg(long, value_name = "PATH", conflicts_with = "tips")]
        command: Option<String>,
        /// Strategy notes: all, or one topic (algorithm, replies, posting, growth, metrics, avoid)
        #[arg(
            long,
            num_args = 0..=1,
            default_missing_value = "all",
            value_parser = clap::builder::PossibleValuesParser::new(crate::commands::agent_info::TIP_TOPICS.iter().copied())
        )]
        tips: Option<String>,
    },

    /// Post a thread (one argument per post)
    Thread {
        /// Post texts, in order
        #[arg(required = true)]
        texts: Vec<String>,
        /// Media file paths to attach to the first post
        #[arg(long, num_args = 1..=4)]
        media: Vec<String>,
    },

    /// X Articles (separate from long posts)
    Article {
        #[command(subcommand)]
        action: ArticleCommands,
    },

    /// Reply to a post (shorthand for post --reply-to)
    Reply {
        /// Post ID or URL to reply to
        #[arg(value_parser = post_id)]
        id: String,
        /// Reply text
        text: String,
        /// Media file paths
        #[arg(long, num_args = 1..=4)]
        media: Vec<String>,
        /// Check the draft and show the route and cost; post nothing
        #[arg(long)]
        dry_run: bool,
    },

    /// Quote a post (shorthand for post --quote)
    Quote {
        /// Post ID or URL to quote
        #[arg(value_parser = post_id)]
        id: String,
        /// Your text above the quoted post
        text: String,
        /// Media file paths
        #[arg(long, num_args = 1..=4)]
        media: Vec<String>,
        /// Check the draft and show the route and cost; post nothing
        #[arg(long)]
        dry_run: bool,
    },

    /// Get post engagement metrics
    Metrics {
        /// Post ID(s) or URL(s)
        #[arg(required = true, value_parser = post_id)]
        ids: Vec<String>,
    },

    /// Manage X lists
    Lists {
        #[command(subcommand)]
        action: ListCommands,
    },

    /// Hide a reply to your post
    HideReply {
        /// Post ID or URL
        #[arg(value_parser = post_id)]
        id: String,
    },

    /// Unhide a reply to your post
    UnhideReply {
        /// Post ID or URL
        #[arg(value_parser = post_id)]
        id: String,
    },

    /// Replies to a post, any age (free through the web session)
    ///
    /// With JEV, replies not worth reading (thanks, emoji, spam, no
    /// substance) are left out; --all shows every reply.
    Replies {
        /// Post ID or URL
        #[arg(value_parser = post_id)]
        id: String,
        /// Max replies to fetch
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "20")]
        count: usize,
        /// Show every reply, without JEV trimming
        #[arg(long)]
        all: bool,
    },

    /// Users who liked a post
    Likers {
        /// Post ID or URL
        #[arg(value_parser = post_id)]
        id: String,
        /// Max users to fetch (1-100)
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "50")]
        count: usize,
    },

    /// Users who reposted a post
    #[command(visible_alias = "reposters")]
    Retweeters {
        /// Post ID or URL
        #[arg(value_parser = post_id)]
        id: String,
        /// Max users to fetch (1-100)
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "50")]
        count: usize,
    },

    /// Quote posts of a post (saved to your local library)
    Quotes {
        /// Post ID or URL
        #[arg(value_parser = post_id)]
        id: String,
        /// Max quotes to fetch (10-100)
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "50")]
        count: usize,
    },

    /// Look up many users in one call
    Users {
        /// Handles, @handles or profile URLs
        #[arg(required = true, value_parser = handle)]
        usernames: Vec<String>,
    },

    /// Post volume over time for a search query
    Volume {
        /// Search query
        query: String,
        /// Time granularity: minute, hour, day
        #[arg(long, default_value = "hour", value_parser = ["minute", "hour", "day"])]
        granularity: String,
    },

    /// Who reposts your posts most
    Amplifiers {
        /// Max reposts to scan (10-100)
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "50")]
        count: usize,
    },

    /// X API rate-limit status
    RateLimits,

    /// Block a user
    Block {
        /// Handle, @handle or profile URL
        #[arg(value_parser = handle)]
        username: String,
    },

    /// Unblock a user
    Unblock {
        /// Handle, @handle or profile URL
        #[arg(value_parser = handle)]
        username: String,
    },

    /// Mute a user
    Mute {
        /// Handle, @handle or profile URL
        #[arg(value_parser = handle)]
        username: String,
    },

    /// Unmute a user
    Unmute {
        /// Handle, @handle or profile URL
        #[arg(value_parser = handle)]
        username: String,
    },

    /// Score a draft before posting (preflight checks, JEV flags)
    Analyze {
        /// Draft text
        text: String,
        /// Optimization goal (replies, impressions)
        #[arg(long)]
        goal: Option<String>,
        /// Analyze as a REPLY to this post (ID or URL; applies reply-quality
        /// rules: catches generic agreement, emoji-only, and too-short replies)
        #[arg(long, value_parser = post_id)]
        reply_to: Option<String>,
    },

    /// Browse posts saved from your reads
    ///
    /// Use --long to surface long-form / Article-candidate exemplars (>=500 chars).
    Inspire {
        /// Filter by topic (searches post text)
        #[arg(long)]
        topic: Option<String>,
        /// Filter by author handle
        #[arg(long, value_parser = handle)]
        author: Option<String>,
        /// Minimum like count
        #[arg(long)]
        min_likes: Option<i64>,
        /// Minimum character count (for surfacing long-form examples)
        #[arg(long)]
        min_chars: Option<i64>,
        /// Shortcut for --min-chars 500 (long-form / Article-candidate band)
        #[arg(long, default_value = "false")]
        long: bool,
        /// Number of results
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "20")]
        count: usize,
    },

    /// Metric snapshots, reply-backs and follower tracking
    Track {
        #[command(subcommand)]
        action: TrackCommands,
    },

    /// Performance reports
    Report {
        #[command(subcommand)]
        action: ReportCommands,
    },

    /// Timing and posting suggestions
    Suggest {
        #[command(subcommand)]
        action: SuggestCommands,
    },

    /// Schedule posts for later
    Schedule {
        #[command(subcommand)]
        action: ScheduleCommands,
    },

    /// Engagement intelligence
    Engage {
        #[command(subcommand)]
        action: EngageCommands,
    },

    /// Install or update the xmaster agent skill (Claude, Codex, Gemini, ...)
    Skill {
        #[command(subcommand)]
        action: SkillCommands,
    },

    /// Self-update from GitHub releases
    Update {
        /// Check for updates without installing
        #[arg(long)]
        check: bool,
    },

    /// Open the xmaster GitHub repo to star it
    Star,

    /// Pacing rails: write budget, cooldowns, pause
    Rails {
        #[command(subcommand)]
        action: RailsCommands,
    },

    /// Exit with a given code (0-5) and its envelope, for contract tests
    #[command(hide = true)]
    Contract {
        code: i32,
    },
}

#[derive(Subcommand)]
pub enum RailsCommands {
    /// Today's write counts vs caps, next allowed time, cooldown
    Status,
    /// Pause all writes (optionally for N hours)
    Pause {
        #[arg(long)]
        hours: Option<f64>,
    },
    /// Resume writes after a pause
    Resume,
    /// Clear an X-challenge cooldown (only after clearing it on x.com)
    ResetCooldown {
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub enum SkillCommands {
    /// Install skill to all detected agent platforms (Claude, Codex, Gemini, etc.)
    Install,
    /// Update skill to latest version bundled in this binary
    Update,
    /// Show where the skill is installed
    Status,
}

#[derive(Subcommand)]
pub enum ArticleCommands {
    /// Generate an X Articles-style HTML preview from a Markdown draft
    Preview {
        /// Markdown draft path, or "-" to read from stdin
        input: String,
        /// Output HTML path (default: <input>.preview.html)
        #[arg(long, short)]
        output: Option<String>,
        /// Article title (default: first H1 in the Markdown draft)
        #[arg(long)]
        title: Option<String>,
        /// Optional article subtitle/deck
        #[arg(long)]
        subtitle: Option<String>,
        /// Header/cover image path or URL (default: first image block, if it appears first)
        #[arg(long)]
        header_image: Option<String>,
        /// Display author name
        #[arg(long, default_value = "Author")]
        author: String,
        /// Display handle, with or without @
        #[arg(long, default_value = "handle")]
        handle: String,
        /// Optional avatar image path or URL
        #[arg(long)]
        avatar: Option<String>,
        /// Article audience badge: public or subscribers
        #[arg(long, default_value = "public")]
        audience: String,
        /// Open the generated HTML preview after writing it
        #[arg(long, default_value = "false")]
        open: bool,
    },
    /// Create a native X Article draft from Markdown without publishing it
    Draft {
        /// Markdown draft path, or "-" to read from stdin
        input: String,
        /// Article title (default: first H1 in the Markdown draft)
        #[arg(long)]
        title: Option<String>,
        /// Header/cover image path (default: first image block, if it appears first)
        #[arg(long)]
        header_image: Option<String>,
        /// Use the official X API v2 Articles endpoint (OAuth keys, no browser
        /// cookies) instead of the private web session path
        #[arg(long)]
        api: bool,
    },
    /// Publish a draft Article via the official X API v2 (makes it PUBLIC)
    Publish {
        /// Article ID (from 'article draft --api')
        article_id: String,
    },
}

#[derive(Subcommand)]
pub enum EngageCommands {
    /// Triage replies and quotes on your post
    Inbox {
        /// Post ID or URL of your post
        #[arg(value_parser = post_id)]
        id: String,
        /// Max direct replies and quote posts to fetch
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "20")]
        count: usize,
        /// Max replies to inspect under each quote post (0 disables quote-comment scan)
        #[arg(long, default_value = "5")]
        quote_reply_count: usize,
    },
    /// Accounts in your niche worth replying to
    Recommend {
        /// Topic to discover targets (uses AI search)
        #[arg(long)]
        topic: Option<String>,
        /// Minimum follower count for targets
        #[arg(long, default_value = "1000")]
        min_followers: u32,
        /// Number of recommendations
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "5")]
        count: usize,
    },
    /// Accounts you track without following
    Watchlist {
        #[command(subcommand)]
        action: WatchlistCommands,
    },
    /// Fresh posts in your niche worth a reply now
    ///
    /// Accepts multiple topics positionally AND as comma-separated lists
    /// (they are merged + deduped). If no topics are passed, falls back to
    /// `niche.topics` from config, enabling zero-arg multi-topic fanout
    /// in a single call. Results are unioned across all topics and ranked
    /// together, so the agent never needs to loop per-topic.
    ///
    /// Examples:
    ///   xmaster engage feed "AI science"
    ///   xmaster engage feed "AI science" "biotech longevity" "gene therapy"
    ///   xmaster engage feed "AI,biotech,gene therapy"
    ///   xmaster engage feed                       # uses niche.topics
    Feed {
        /// Zero or more topics/niches to scan. Comma-separated values are
        /// split and merged with positional entries.
        topics: Vec<String>,
        /// Minimum follower count for authors
        #[arg(long, default_value = "5000")]
        min_followers: u64,
        /// Maximum age in minutes (default: 30 — reply momentum dies after ~30-60 min)
        #[arg(long, default_value = "30")]
        max_age_mins: u64,
        /// Number of posts to return (default 10; with JEV, its best 5 worth a reply)
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n')]
        count: Option<usize>,
    },
    /// Small accounts replying under a big post, for peer replies
    ///
    /// Small accounts grow fastest by replying to OTHER SMALL ACCOUNTS under
    /// big-account posts (community consensus, Q1 2026). This command fetches
    /// the replies to any post and filters to accounts inside your target
    /// follower band so you can engage peer-to-peer instead of competing for
    /// the top-of-thread slot under the big account.
    Swarm {
        /// Post ID or URL of the big post to swarm under
        #[arg(value_parser = post_id)]
        id: String,
        /// Minimum follower count to include (default 50 — filters brand-new accounts)
        #[arg(long, default_value = "50")]
        min_followers: u64,
        /// Maximum follower count to include (default 5000 — small/mid only)
        #[arg(long, default_value = "5000")]
        max_followers: u64,
        /// Number of swarm targets to return
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "10")]
        count: usize,
    },
    /// Accounts where your past replies did best
    ///
    /// Aggregates your sent replies from the last N days, groups by target
    /// username, and computes avg impressions, avg profile clicks, reply-back
    /// rate, and a composite score. Use this to find which accounts reward
    /// your reply effort with the most reach — then prioritize re-engaging.
    HotTargets {
        /// Look back this many days
        #[arg(long, default_value = "7")]
        days: i64,
        /// Minimum avg impressions across samples (HAVING filter)
        #[arg(long, default_value = "100")]
        min_imps: i64,
        /// Minimum avg profile clicks across samples (HAVING filter)
        #[arg(long, default_value = "0")]
        min_profile_clicks: i64,
        /// Minimum reply count per target (HAVING filter)
        #[arg(long, default_value = "1")]
        min_samples: i64,
        /// Number of results
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "10")]
        count: usize,
        /// Sort order: score | avg-impressions | avg-profile-clicks | reply-back-rate
        #[arg(long, default_value = "score")]
        sort: String,
    },
}

#[derive(Subcommand)]
pub enum WatchlistCommands {
    /// Add an account to your watchlist
    Add {
        /// Handle, @handle or profile URL
        #[arg(value_parser = handle)]
        username: String,
        /// Topic/niche tag
        #[arg(long)]
        topic: Option<String>,
    },
    /// List watched accounts
    List,
    /// Remove an account from your watchlist
    Remove {
        /// Handle, @handle or profile URL
        #[arg(value_parser = handle)]
        username: String,
    },
}

#[derive(Subcommand)]
pub enum ScheduleCommands {
    /// Schedule a new post
    Add {
        /// Post text
        content: String,
        /// When to post: ISO datetime "2026-03-24 09:00" or "auto" for best time
        #[arg(long)]
        at: String,
        /// Reply to this post (ID or URL)
        #[arg(long, value_parser = post_id)]
        reply_to: Option<String>,
        /// Quote this post (ID or URL)
        #[arg(long, value_parser = post_id)]
        quote: Option<String>,
        /// Media file paths
        #[arg(long, num_args = 1..=4)]
        media: Vec<String>,
    },
    /// List scheduled posts
    List {
        /// Filter by status: pending, sent, failed, cancelled
        #[arg(long)]
        status: Option<String>,
    },
    /// Cancel a scheduled post
    Cancel {
        /// Schedule ID
        id: String,
    },
    /// Reschedule a post
    Reschedule {
        /// Schedule ID
        id: String,
        /// New time: ISO datetime or "auto"
        #[arg(long)]
        at: String,
    },
    /// Fire all due scheduled posts (run via cron/launchd)
    Fire,
    /// Set up launchd for automatic scheduling (macOS)
    Setup,
}

#[derive(Subcommand)]
pub enum TrackCommands {
    /// Snapshot metrics for all recent posts (run via cron)
    Run,
    /// Show tracking status for recent posts
    Status,
    /// Snapshot follower count and detect new/lost followers
    Followers,
    /// Show follower growth history
    Growth {
        /// Number of days of history [default: 30]
        #[arg(long, short, default_value = "30")]
        days: i64,
    },
}

#[derive(Subcommand)]
pub enum ReportCommands {
    /// Daily performance report
    Daily,
    /// Weekly performance report
    Weekly,
}

#[derive(Subcommand)]
pub enum SuggestCommands {
    /// Show best posting times from your history
    BestTime,
    /// Check if it's safe to post now (cannibalization guard)
    NextPost,
}

#[derive(Subcommand)]
pub enum ListCommands {
    /// Create a new list
    Create {
        /// List name
        name: String,
        /// List description
        #[arg(long)]
        description: Option<String>,
    },
    /// Delete a list
    Delete {
        /// List ID
        #[arg(value_parser = numeric_id)]
        id: String,
    },
    /// Add a user to a list
    Add {
        /// List ID
        #[arg(value_parser = numeric_id)]
        list_id: String,
        /// Handle, @handle or profile URL
        #[arg(value_parser = handle)]
        username: String,
    },
    /// Remove a user from a list
    Remove {
        /// List ID
        #[arg(value_parser = numeric_id)]
        list_id: String,
        /// Handle, @handle or profile URL
        #[arg(value_parser = handle)]
        username: String,
    },
    /// View list timeline
    Timeline {
        /// List ID
        #[arg(value_parser = numeric_id)]
        list_id: String,
        /// Number of posts
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "10")]
        count: usize,
    },
    /// List the members of a list
    Members {
        /// List ID
        #[arg(value_parser = numeric_id)]
        list_id: String,
        /// Number of members to fetch (1-100)
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "50")]
        count: usize,
    },
    /// List your owned lists
    Mine {
        /// Number of results
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "20")]
        count: usize,
    },
}

#[derive(Subcommand)]
pub enum DmCommands {
    /// Send a direct message
    Send {
        /// Handle, @handle or profile URL
        #[arg(value_parser = handle)]
        username: String,
        /// Message text
        text: String,
    },
    /// View DM inbox
    Inbox {
        /// Number of conversations
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "10")]
        count: usize,
    },
    /// View a DM thread
    Thread {
        /// Conversation ID
        id: String,
        /// Number of messages
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "20")]
        count: usize,
    },
}

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Show current configuration (keys masked)
    Show,
    /// Print the config file path (it may not exist yet)
    Path,
    /// Get a single configuration value (raw, no masking for non-secret keys)
    Get {
        /// Key path (e.g., style.voice, account.premium)
        key: String,
    },
    /// Set a configuration value
    Set {
        /// Key path (e.g., keys.api_key)
        key: String,
        /// Value to set
        value: String,
    },
    /// Live read-only test of the web session and keys
    Check,
    /// Step-by-step setup guide for X API and xAI keys
    Guide,
    /// Authorize OAuth 2.0 (required for bookmarks)
    Auth,
    /// Copy X cookies from your browser: free reads and writes, replies anywhere
    WebLogin,
}

#[derive(Subcommand)]
pub enum BookmarkCommands {
    /// List recent bookmarks
    List {
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "10")]
        count: usize,
        /// Show only unread
        #[arg(long)]
        unread: bool,
    },
    /// Sync bookmarks to the local archive (keeps posts deleted on X)
    Sync {
        /// Number of bookmarks to fetch from X
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "100")]
        count: usize,
    },
    /// Search saved bookmarks locally
    Search {
        /// Search query
        query: String,
    },
    /// Export bookmarks as markdown
    Export {
        /// Output file path
        #[arg(long, short)]
        output: Option<String>,
        /// Only export unread
        #[arg(long)]
        unread: bool,
    },
    /// Get bookmark digest (summary of recent saves)
    Digest {
        /// Number of days to cover
        #[arg(long, short, default_value = "7")]
        days: u32,
    },
    /// Show bookmark statistics
    Stats,
    /// List bookmark folders
    Folders,
    /// List bookmarks in a specific folder
    Folder {
        /// Folder ID
        id: String,
        /// Number of bookmarks to fetch
        #[arg(long, short, visible_alias = "limit", visible_short_alias = 'n', default_value = "20")]
        count: usize,
    },
}

/// A clap parse error as `(message, suggestion)` for the error envelope:
/// clap's text without its usage block, and a fix scoped to the command the
/// arguments name (`argv` without the program name).
pub fn explain_parse_error(e: &clap::Error, argv: &[String]) -> (String, String) {
    use clap::error::{ContextKind, ContextValue, ErrorKind};
    use clap::CommandFactory;

    let text = e.to_string();
    let message = text
        .lines()
        .map(str::trim)
        .take_while(|l| !l.starts_with("Usage:"))
        .filter(|l| !l.is_empty() && !l.starts_with("tip:"))
        .collect::<Vec<_>>()
        .join(" ");
    let message = message.strip_prefix("error: ").unwrap_or(&message).to_string();
    let usage = text.lines().find_map(|l| l.trim().strip_prefix("Usage: ")).map(str::to_string);

    // The deepest command the arguments name: `timeline`, `engage feed`.
    let mut root = Cli::command();
    root.build();
    let mut cmd = &root;
    for word in argv.iter().filter(|w| !w.starts_with('-')) {
        match cmd.find_subcommand(word) {
            Some(sub) => cmd = sub,
            None => break,
        }
    }
    let shown = cmd.get_bin_name().unwrap_or("xmaster").to_string();
    let first = |kind| match e.get(kind) {
        Some(ContextValue::String(s)) => Some(s.clone()),
        Some(ContextValue::Strings(v)) => v.first().cloned(),
        _ => None,
    };

    let mut fix = Vec::new();
    if let Some(arg) = first(ContextKind::SuggestedArg) {
        fix.push(format!("Did you mean {arg}?"));
    } else if let Some(sub) = first(ContextKind::SuggestedSubcommand) {
        fix.push(format!("Did you mean `{shown} {sub}`?"));
    } else if e.kind() == ErrorKind::UnknownArgument {
        let flags: Vec<String> = cmd
            .get_arguments()
            .filter(|a| !a.is_positional() && !a.is_hide_set() && !a.is_global_set() && a.get_id() != "help")
            .filter_map(|a| {
                let long = a.get_long()?;
                Some(match a.get_short() {
                    Some(s) => format!("-{s}/--{long}"),
                    None => format!("--{long}"),
                })
            })
            .collect();
        fix.push(if flags.is_empty() {
            format!("`{shown}` takes no flags.")
        } else {
            format!("Flags for `{shown}`: {}.", flags.join(", "))
        });
    } else if matches!(e.kind(), ErrorKind::InvalidSubcommand | ErrorKind::MissingSubcommand | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand) {
        if std::ptr::eq(cmd, &root) {
            fix.push("`xmaster agent-info` lists every command.".into());
        } else {
            let subs: Vec<&str> = cmd.get_subcommands().map(|s| s.get_name()).filter(|n| *n != "help").collect();
            fix.push(format!("Commands under `{shown}`: {}.", subs.join(", ")));
        }
    }
    match usage {
        Some(usage) if !std::ptr::eq(cmd, &root) => fix.push(format!("Usage: {usage}; `{shown} --help` for details")),
        _ => fix.push(format!("`{shown} --help` shows the usage")),
    }
    (message, fix.join(" "))
}

/// clap parser: a post ID or post URL (x.com, twitter.com, mirrors),
/// normalized to the ID. Garbage fails at parse time (exit 3), before any
/// paid call.
fn post_id(input: &str) -> Result<String, String> {
    crate::utils::tweet_id(input)
        .ok_or_else(|| "expected a post ID or post URL, e.g. 1234567890 or https://x.com/user/status/1234567890".into())
}

/// clap parser: `name`, `@name` or a profile URL, normalized to the handle.
fn handle(input: &str) -> Result<String, String> {
    crate::utils::handle(input)
        .ok_or_else(|| "expected an X handle (1-15 letters, digits or _), @handle or profile URL".into())
}

/// clap parser: a numeric X ID (lists).
fn numeric_id(input: &str) -> Result<String, String> {
    let id = input.trim();
    if (1..=20).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit()) {
        Ok(id.to_string())
    } else {
        Err("expected a numeric ID".into())
    }
}

/// TRANSITIONAL for files outside lane `commands` (read_post.rs,
/// moderation.rs, analyze.rs, engage_recommend.rs): their IDs arrive already
/// normalized by [`post_id`], so this is the identity on them. Delete once
/// those callers use the ID as given.
pub fn parse_tweet_id(input: &str) -> String {
    crate::utils::tweet_id(input).unwrap_or_else(|| input.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("xmaster").chain(args.iter().copied()))
    }

    #[test]
    fn post_ids_normalize_at_parse_time() {
        let cli = parse(&["read", "https://x.com/user/status/1234567890/photo/1?s=20"]).unwrap();
        assert!(matches!(cli.command, Commands::Read { ref id } if id == "1234567890"));
        let cli = parse(&["reply", "https://twitter.com/a/status/42", "hi"]).unwrap();
        assert!(matches!(cli.command, Commands::Reply { ref id, .. } if id == "42"));
    }

    #[test]
    fn garbage_ids_fail_at_parse_time() {
        for args in [
            &["read", "notanid"][..],
            &["like", "https://example.com/status/1"],
            &["metrics", "1", "x"],
            &["post", "hi", "--reply-to", "abc"],
            &["lists", "timeline", "abc"],
        ] {
            assert!(parse(args).is_err(), "{args:?} should fail");
        }
    }

    #[test]
    fn handles_strip_at_and_urls() {
        let cli = parse(&["follow", "@jack"]).unwrap();
        assert!(matches!(cli.command, Commands::Follow { ref username } if username == "jack"));
        let cli = parse(&["user", "https://x.com/sama"]).unwrap();
        assert!(matches!(cli.command, Commands::User { ref username } if username == "sama"));
        assert!(parse(&["follow", "not a handle"]).is_err());
    }

    #[test]
    fn x_terms_are_aliases() {
        assert!(matches!(parse(&["repost", "1"]).unwrap().command, Commands::Retweet { .. }));
        assert!(matches!(parse(&["unrepost", "1"]).unwrap().command, Commands::Unretweet { .. }));
        assert!(matches!(parse(&["reposters", "1"]).unwrap().command, Commands::Retweeters { .. }));
        assert!(matches!(parse(&["convo", "1"]).unwrap().command, Commands::Conversation { count: 10, .. }));
    }

    #[test]
    fn transitional_parse_tweet_id_is_identity_on_ids() {
        assert_eq!(parse_tweet_id("1234567890"), "1234567890");
        assert_eq!(parse_tweet_id("https://x.com/u/status/1234567890"), "1234567890");
    }
}
