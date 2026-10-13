use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "xmaster",
    version,
    about = "Enterprise-grade X/Twitter CLI — post, reply, like, retweet, DM, search, and more",
    long_about = "Built by 199 Biotechnologies for AI agents and humans.\n\nAgent-friendly: auto-JSON when piped, semantic exit codes, structured errors."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Output as JSON (auto-enabled when piped)
    #[arg(long, global = true)]
    pub json: bool,

    /// Output as CSV (list commands: timeline, search, replies, metrics, ...)
    #[arg(long, global = true)]
    pub csv: bool,

    /// Pretty-print JSON (compact by default when piped, to save agent tokens)
    #[arg(long, global = true)]
    pub pretty: bool,
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
    },

    /// Delete a post
    Delete {
        /// Post ID or URL
        #[arg(value_parser = post_id)]
        id: String,
    },

    /// Read a post — full text, author, date, metrics, media URLs in one call
    Read {
        /// Post ID or URL
        #[arg(value_parser = post_id)]
        id: String,
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
    Retweet {
        /// Post ID or URL
        #[arg(value_parser = post_id)]
        id: String,
    },

    /// Undo a repost
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

    /// View timeline
    Timeline {
        /// Handle (omit for home timeline)
        #[arg(long, value_parser = handle)]
        user: Option<String>,
        /// Number of tweets
        #[arg(long, short, alias = "limit", default_value = "10")]
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
        #[arg(long, short, alias = "limit", default_value = "10")]
        count: usize,
        /// Only show mentions after this post ID
        #[arg(long, value_parser = post_id)]
        since_id: Option<String>,
    },

    /// Search tweets (X API v2)
    Search {
        /// Search query. Supports X operators, incl. min_likes:/min_replies:/
        /// min_reposts: (since May 2026). Keyword search no longer returns reposts
        query: String,
        /// Search mode: recent (latest first) or top
        #[arg(long, default_value = "recent", value_parser = ["recent", "latest", "top", "relevancy", "relevant", "popular"])]
        mode: String,
        /// Number of results
        #[arg(long, short, alias = "limit", default_value = "10")]
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
        #[arg(long, short, alias = "limit", default_value = "10")]
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

    /// Get user info
    User {
        /// Handle, @handle or profile URL
        #[arg(value_parser = handle)]
        username: String,
    },

    /// Get authenticated user info
    Me,

    /// Manage bookmarks (sync, search, export, digest)
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
        #[arg(long, short, alias = "limit", default_value = "20")]
        count: usize,
    },

    /// List following
    Following {
        /// Handle, @handle or profile URL
        #[arg(value_parser = handle)]
        username: String,
        /// Number of results
        #[arg(long, short, alias = "limit", default_value = "20")]
        count: usize,
    },

    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: ConfigCommands,
    },

    /// Show agent-readable capabilities
    AgentInfo,

    /// Post a thread (one argument per post)
    Thread {
        /// Post texts, in order
        #[arg(required = true)]
        texts: Vec<String>,
        /// Media file paths to attach to the first tweet
        #[arg(long, num_args = 1..=4)]
        media: Vec<String>,
    },

    /// X Articles tools (separate from long posts / Note Tweets)
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

    /// Hide a reply to your tweet
    HideReply {
        /// Post ID or URL
        #[arg(value_parser = post_id)]
        id: String,
    },

    /// Unhide a reply to your tweet
    UnhideReply {
        /// Post ID or URL
        #[arg(value_parser = post_id)]
        id: String,
    },

    /// Get replies to a tweet (uses conversation_id search). With JEV, replies
    /// not worth reading (thanks, emoji, spam, no substance) are left out.
    Replies {
        /// Post ID or URL
        #[arg(value_parser = post_id)]
        id: String,
        /// Max replies to fetch
        #[arg(long, short, alias = "limit", default_value = "20")]
        count: usize,
        /// Show every reply, without JEV trimming
        #[arg(long)]
        all: bool,
    },

    /// List users who liked a tweet
    Likers {
        /// Post ID or URL
        #[arg(value_parser = post_id)]
        id: String,
        /// Max users to fetch (1-100)
        #[arg(long, short, alias = "limit", default_value = "50")]
        count: usize,
    },

    /// List users who reposted a post
    Retweeters {
        /// Post ID or URL
        #[arg(value_parser = post_id)]
        id: String,
        /// Max users to fetch (1-100)
        #[arg(long, short, alias = "limit", default_value = "50")]
        count: usize,
    },

    /// List quote tweets of a tweet (persisted into discovered_posts)
    Quotes {
        /// Post ID or URL
        #[arg(value_parser = post_id)]
        id: String,
        /// Max quotes to fetch (10-100)
        #[arg(long, short, alias = "limit", default_value = "50")]
        count: usize,
    },

    /// Batch-lookup multiple users by username in a single call
    Users {
        /// Handles, @handles or profile URLs
        #[arg(required = true, value_parser = handle)]
        usernames: Vec<String>,
    },

    /// Show recent tweet volume for a search query over time
    Volume {
        /// Search query
        query: String,
        /// Time granularity: minute, hour, day
        #[arg(long, default_value = "hour", value_parser = ["minute", "hour", "day"])]
        granularity: String,
    },

    /// Show who amplifies your content (users who repost your tweets)
    Amplifiers {
        /// Max reposts to scan (10-100)
        #[arg(long, short, alias = "limit", default_value = "50")]
        count: usize,
    },

    /// Show API rate limit status
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

    /// Analyze a tweet before posting (pre-flight check)
    Analyze {
        /// Tweet text to analyze
        text: String,
        /// Optimization goal (replies, impressions)
        #[arg(long)]
        goal: Option<String>,
        /// Analyze as a REPLY to this post (ID or URL; applies reply-quality
        /// rules: catches generic agreement, emoji-only, and too-short replies)
        #[arg(long, value_parser = post_id)]
        reply_to: Option<String>,
    },

    /// Browse your discovered posts library for content inspiration.
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
        #[arg(long, alias = "limit", default_value = "20")]
        count: usize,
    },

    /// Track metric snapshots for recent posts
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

    /// Install or update the xmaster agent skill for all AI platforms
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
    /// Audit a post's conversation inbox: replies, quote tweets, and comments under quotes
    Inbox {
        /// Post ID or URL of your post
        #[arg(value_parser = post_id)]
        id: String,
        /// Max direct replies and quote tweets to fetch
        #[arg(long, short, alias = "limit", default_value = "20")]
        count: usize,
        /// Max replies to inspect under each quote tweet (0 disables quote-comment scan)
        #[arg(long, default_value = "5")]
        quote_reply_count: usize,
    },
    /// Find high-ROI reply targets in your niche
    Recommend {
        /// Topic to discover targets (uses AI search)
        #[arg(long)]
        topic: Option<String>,
        /// Minimum follower count for targets
        #[arg(long, default_value = "1000")]
        min_followers: u32,
        /// Number of recommendations
        #[arg(long, short, alias = "limit", default_value = "5")]
        count: usize,
    },
    /// Manage your niche watchlist (accounts you track but don't follow)
    Watchlist {
        #[command(subcommand)]
        action: WatchlistCommands,
    },
    /// Find fresh posts from big accounts to reply to NOW.
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
        #[arg(long, short, alias = "limit")]
        count: Option<usize>,
    },
    /// Find small-to-mid accounts replying under a big post — the 2026
    /// peer-to-peer growth lane.
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
        #[arg(long, short, alias = "limit", default_value = "10")]
        count: usize,
    },
    /// Rank accounts you've already replied to by downstream reply performance.
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
        #[arg(long, short, alias = "limit", default_value = "10")]
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
        /// Tweet text
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
        /// Number of tweets
        #[arg(long, short, alias = "limit", default_value = "10")]
        count: usize,
    },
    /// List the members of a list
    Members {
        /// List ID
        #[arg(value_parser = numeric_id)]
        list_id: String,
        /// Number of members to fetch (1-100)
        #[arg(long, short, alias = "limit", default_value = "50")]
        count: usize,
    },
    /// List your owned lists
    Mine {
        /// Number of results
        #[arg(long, short, alias = "limit", default_value = "20")]
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
        #[arg(long, short, alias = "limit", default_value = "10")]
        count: usize,
    },
    /// View a DM thread
    Thread {
        /// Conversation ID
        id: String,
        /// Number of messages
        #[arg(long, short, alias = "limit", default_value = "20")]
        count: usize,
    },
}

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Show current configuration (keys masked)
    Show,
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
    /// Validate configured credentials
    Check,
    /// Step-by-step setup guide for X API and xAI keys
    Guide,
    /// Authorize OAuth 2.0 (required for bookmarks)
    Auth,
    /// Auto-capture X web cookies from your browser (enables replies anywhere, likes, follows, quotes via your browser session)
    WebLogin,
}

#[derive(Subcommand)]
pub enum BookmarkCommands {
    /// List recent bookmarks
    List {
        #[arg(long, short, alias = "limit", default_value = "10")]
        count: usize,
        /// Show only unread
        #[arg(long)]
        unread: bool,
    },
    /// Sync bookmarks from X to local database (preserves deleted tweets)
    Sync {
        /// Number of bookmarks to fetch from X
        #[arg(long, short, alias = "limit", default_value = "100")]
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
        #[arg(long, short, alias = "limit", default_value = "20")]
        count: usize,
    },
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
    fn transitional_parse_tweet_id_is_identity_on_ids() {
        assert_eq!(parse_tweet_id("1234567890"), "1234567890");
        assert_eq!(parse_tweet_id("https://x.com/u/status/1234567890"), "1234567890");
    }
}
