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

    /// Read a post — full text, author, date, metrics, media URLs in one call
    Read {
        /// Tweet ID or URL
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
        /// Only show posts after this time (e.g. "12h", "7d", or ISO 8601)
        #[arg(long)]
        since: Option<String>,
        /// Only show posts before this time (e.g. "12h", "7d", or ISO 8601)
        #[arg(long)]
        before: Option<String>,
        /// Sort by: impressions, likes, retweets, date (default: date)
        #[arg(long)]
        sort: Option<String>,
    },

    /// View your mentions
    Mentions {
        /// Number of mentions
        #[arg(long, short, default_value = "10")]
        count: usize,
        /// Only show mentions after this tweet ID
        #[arg(long)]
        since_id: Option<String>,
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

    /// Manage bookmarks (sync, search, export, digest)
    Bookmarks {
        #[command(subcommand)]
        action: BookmarkCommands,
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

    /// Reply to a tweet (shorthand for post --reply-to)
    Reply {
        /// Tweet ID or URL to reply to
        id: String,
        /// Reply text
        text: String,
        /// Media file paths
        #[arg(long, num_args = 1..=4)]
        media: Vec<String>,
    },

    /// Get tweet engagement metrics
    Metrics {
        /// Tweet ID(s) or URL(s)
        ids: Vec<String>,
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

    /// Get replies to a tweet (uses conversation_id search)
    Replies {
        /// Tweet ID or URL
        id: String,
        /// Max replies to fetch
        #[arg(long, short, default_value = "20")]
        count: usize,
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
pub enum EngageCommands {
    /// Find high-ROI reply targets in your niche
    Recommend {
        /// Topic to discover targets (uses AI search)
        #[arg(long)]
        topic: Option<String>,
        /// Minimum follower count for targets
        #[arg(long, default_value = "1000")]
        min_followers: u32,
        /// Number of recommendations
        #[arg(long, short, default_value = "5")]
        count: usize,
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
    /// Step-by-step setup guide for X API and xAI keys
    Guide,
    /// Authorize OAuth 2.0 (required for bookmarks)
    Auth,
    /// Auto-capture X web cookies from your browser (enables reply fallback)
    WebLogin,
}

#[derive(Subcommand)]
pub enum BookmarkCommands {
    /// List recent bookmarks
    List {
        #[arg(long, short, default_value = "10")]
        count: usize,
        /// Show only unread
        #[arg(long)]
        unread: bool,
    },
    /// Sync bookmarks from X to local database (preserves deleted tweets)
    Sync {
        /// Number of bookmarks to fetch from X
        #[arg(long, short, default_value = "100")]
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
}

/// Parse a tweet ID from a URL or raw ID string.
/// Handles URLs like `.../status/12345/photo/1` by finding the segment after "status".
pub fn parse_tweet_id(input: &str) -> String {
    let input = input.trim();
    if input.contains("x.com/") || input.contains("twitter.com/") {
        let parts: Vec<&str> = input.split('/').filter(|s| !s.is_empty()).collect();
        // Find the segment immediately after "status"
        if let Some(pos) = parts.iter().position(|&p| p == "status") {
            if let Some(id_part) = parts.get(pos + 1) {
                let id = id_part.split('?').next().unwrap_or(id_part);
                if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) {
                    return id.to_string();
                }
            }
        }
    }
    input.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_id() {
        assert_eq!(parse_tweet_id("1234567890"), "1234567890");
    }

    #[test]
    fn x_url() {
        assert_eq!(
            parse_tweet_id("https://x.com/user/status/1234567890"),
            "1234567890"
        );
    }

    #[test]
    fn twitter_url() {
        assert_eq!(
            parse_tweet_id("https://twitter.com/user/status/1234567890"),
            "1234567890"
        );
    }

    #[test]
    fn url_with_query() {
        assert_eq!(
            parse_tweet_id("https://x.com/user/status/1234567890?s=20"),
            "1234567890"
        );
    }

    #[test]
    fn url_with_trailing_slash() {
        assert_eq!(
            parse_tweet_id("https://x.com/user/status/1234567890/"),
            "1234567890"
        );
    }

    #[test]
    fn url_with_multiple_query_params() {
        assert_eq!(
            parse_tweet_id("https://x.com/user/status/9876543210?s=20&t=abc"),
            "9876543210"
        );
    }

    #[test]
    fn whitespace_trimmed() {
        assert_eq!(parse_tweet_id("  1234567890  "), "1234567890");
    }

    #[test]
    fn url_with_photo_suffix() {
        assert_eq!(
            parse_tweet_id("https://x.com/user/status/1234567890/photo/1"),
            "1234567890"
        );
    }

    #[test]
    fn url_with_video_suffix() {
        assert_eq!(
            parse_tweet_id("https://x.com/user/status/9876543210/video/1"),
            "9876543210"
        );
    }
}
