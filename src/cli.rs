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

    /// Suppress non-essential output
    #[arg(long, global = true)]
    pub quiet: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Post a tweet (text, media, reply, quote, poll)
    Post {
        /// Tweet text
        text: String,
        /// Reply to a tweet ID
        #[arg(long)]
        reply_to: Option<String>,
        /// Quote a tweet ID
        #[arg(long)]
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

    /// Delete a tweet
    Delete {
        /// Tweet ID to delete
        id: String,
    },

    /// Like a tweet
    Like {
        /// Tweet ID or URL
        id: String,
    },

    /// Unlike a tweet
    Unlike {
        /// Tweet ID or URL
        id: String,
    },

    /// Retweet a tweet
    Retweet {
        /// Tweet ID or URL
        id: String,
    },

    /// Undo a retweet
    Unretweet {
        /// Tweet ID or URL
        id: String,
    },

    /// Bookmark a tweet
    Bookmark {
        /// Tweet ID or URL
        id: String,
    },

    /// Remove a bookmark
    Unbookmark {
        /// Tweet ID or URL
        id: String,
    },

    /// Follow a user
    Follow {
        /// Username (without @)
        username: String,
    },

    /// Unfollow a user
    Unfollow {
        /// Username (without @)
        username: String,
    },

    /// Direct messages
    Dm {
        #[command(subcommand)]
        action: DmCommands,
    },

    /// View timeline
    Timeline {
        /// Username (omit for home timeline)
        #[arg(long)]
        user: Option<String>,
        /// Number of tweets
        #[arg(long, short, default_value = "10")]
        count: usize,
    },

    /// View your mentions
    Mentions {
        /// Number of mentions
        #[arg(long, short, default_value = "10")]
        count: usize,
    },

    /// Search tweets (X API v2)
    Search {
        /// Search query
        query: String,
        /// Search mode
        #[arg(long, default_value = "recent")]
        mode: String,
        /// Number of results
        #[arg(long, short, default_value = "10")]
        count: usize,
    },

    /// AI-powered search (xAI/Grok)
    SearchAi {
        /// Search query
        query: String,
        /// Number of results
        #[arg(long, short, default_value = "10")]
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
    },

    /// Get user info
    User {
        /// Username (without @)
        username: String,
    },

    /// Get authenticated user info
    Me,

    /// List bookmarks
    Bookmarks {
        /// Number of bookmarks
        #[arg(long, short, default_value = "10")]
        count: usize,
    },

    /// List followers
    Followers {
        /// Username (without @)
        username: String,
        /// Number of results
        #[arg(long, short, default_value = "20")]
        count: usize,
    },

    /// List following
    Following {
        /// Username (without @)
        username: String,
        /// Number of results
        #[arg(long, short, default_value = "20")]
        count: usize,
    },

    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: ConfigCommands,
    },

    /// Show agent-readable capabilities
    AgentInfo,

    /// Post a multi-tweet thread
    Thread {
        /// Tweet texts (one per thread tweet)
        texts: Vec<String>,
        /// Media file paths to attach to the first tweet
        #[arg(long, num_args = 1..=4)]
        media: Vec<String>,
    },

    /// Get tweet engagement metrics
    Metrics {
        /// Tweet ID or URL
        id: String,
    },

    /// Manage X lists
    Lists {
        #[command(subcommand)]
        action: ListCommands,
    },

    /// Hide a reply to your tweet
    HideReply {
        /// Tweet ID or URL
        id: String,
    },

    /// Unhide a reply to your tweet
    UnhideReply {
        /// Tweet ID or URL
        id: String,
    },

    /// Show API rate limit status
    RateLimits,

    /// Block a user
    Block {
        /// Username (without @)
        username: String,
    },

    /// Unblock a user
    Unblock {
        /// Username (without @)
        username: String,
    },

    /// Mute a user
    Mute {
        /// Username (without @)
        username: String,
    },

    /// Unmute a user
    Unmute {
        /// Username (without @)
        username: String,
    },

    /// Analyze a tweet before posting (pre-flight check)
    Analyze {
        /// Tweet text to analyze
        text: String,
        /// Optimization goal (replies, impressions, bookmarks)
        #[arg(long)]
        goal: Option<String>,
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

    /// Self-update from GitHub releases
    Update {
        /// Check for updates without installing
        #[arg(long)]
        check: bool,
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
        /// Reply to tweet ID
        #[arg(long)]
        reply_to: Option<String>,
        /// Quote tweet ID
        #[arg(long)]
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
        id: String,
    },
    /// Add a user to a list
    Add {
        /// List ID
        list_id: String,
        /// Username (without @)
        username: String,
    },
    /// Remove a user from a list
    Remove {
        /// List ID
        list_id: String,
        /// Username (without @)
        username: String,
    },
    /// View list timeline
    Timeline {
        /// List ID
        list_id: String,
        /// Number of tweets
        #[arg(long, short, default_value = "10")]
        count: usize,
    },
    /// List your owned lists
    Mine {
        /// Number of results
        #[arg(long, short, default_value = "20")]
        count: usize,
    },
}

#[derive(Subcommand)]
pub enum DmCommands {
    /// Send a direct message
    Send {
        /// Username (without @)
        username: String,
        /// Message text
        text: String,
    },
    /// View DM inbox
    Inbox {
        /// Number of conversations
        #[arg(long, short, default_value = "10")]
        count: usize,
    },
    /// View a DM thread
    Thread {
        /// Conversation ID
        id: String,
        /// Number of messages
        #[arg(long, short, default_value = "20")]
        count: usize,
    },
}

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Show current configuration (keys masked)
    Show,
    /// Set a configuration value
    Set {
        /// Key path (e.g., keys.api_key)
        key: String,
        /// Value to set
        value: String,
    },
    /// Validate configured credentials
    Check,
}

/// Parse a tweet ID from a URL or raw ID string
pub fn parse_tweet_id(input: &str) -> String {
    if input.contains("x.com/") || input.contains("twitter.com/") {
        if let Some(id) = input.split('/').last() {
            let id = id.split('?').next().unwrap_or(id);
            return id.to_string();
        }
    }
    input.to_string()
}
