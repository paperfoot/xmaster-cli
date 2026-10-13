pub mod post;
pub mod engage;
pub mod social;
pub mod timeline;
pub mod search;
pub mod search_ai;
pub mod dm;
pub mod config_cmd;
pub mod agent_info;
pub mod update;
pub mod user;
pub mod thread;
pub mod metrics;
pub mod lists;
pub mod moderation;
pub mod rate_limits;
pub mod analyze;
pub mod track;
pub mod report;
pub mod suggest;
pub mod schedule;
pub mod bookmarks_cmd;
pub mod engage_recommend;
pub mod engage_inbox;
pub mod skill_cmd;
pub mod replies;
pub mod read_post;
pub mod inspire;
pub mod tweet_engagement;
pub mod quotes;
pub mod amplifiers;
pub mod volume;
pub mod article;
pub mod conversation;
pub mod rails_cmd;

use crate::cli::{ArticleCommands, Cli, Commands, ConfigCommands, DmCommands, EngageCommands, WatchlistCommands, ListCommands, RailsCommands, TrackCommands, ReportCommands, SuggestCommands, ScheduleCommands, BookmarkCommands, SkillCommands};
use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::output::OutputFormat;
use std::sync::Arc;

/// Extract the top-level command name as a lowercase string for allowlisting.
fn command_name(cmd: &Commands) -> &'static str {
    match cmd {
        Commands::Post { .. } => "post",
        Commands::Delete { .. } => "delete",
        Commands::Like { .. } => "like",
        Commands::Unlike { .. } => "unlike",
        Commands::Retweet { .. } => "retweet",
        Commands::Unretweet { .. } => "unretweet",
        Commands::Bookmark { .. } => "bookmark",
        Commands::Unbookmark { .. } => "unbookmark",
        Commands::Follow { .. } => "follow",
        Commands::Unfollow { .. } => "unfollow",
        Commands::Dm { .. } => "dm",
        Commands::Timeline { .. } => "timeline",
        Commands::Mentions { .. } => "mentions",
        Commands::Search { .. } => "search",
        Commands::SearchAi { .. } => "search-ai",
        Commands::Trending { .. } => "trending",
        Commands::User { .. } => "user",
        Commands::Me => "me",
        Commands::Bookmarks { .. } => "bookmarks",
        Commands::Followers { .. } => "followers",
        Commands::Following { .. } => "following",
        Commands::Config { .. } => "config",
        Commands::AgentInfo => "agent-info",
        Commands::Engage { .. } => "engage",
        Commands::Update { .. } => "update",
        Commands::Star => "star",
        Commands::Thread { .. } => "thread",
        Commands::Article { .. } => "article",
        Commands::Reply { .. } => "reply",
        Commands::Metrics { .. } => "metrics",
        Commands::Lists { .. } => "lists",
        Commands::HideReply { .. } => "hide-reply",
        Commands::UnhideReply { .. } => "unhide-reply",
        Commands::Read { .. } => "read",
        Commands::Conversation { .. } => "conversation",
        Commands::Quote { .. } => "quote",
        Commands::Replies { .. } => "replies",
        Commands::Likers { .. } => "likers",
        Commands::Retweeters { .. } => "retweeters",
        Commands::Quotes { .. } => "quotes",
        Commands::Users { .. } => "users",
        Commands::Volume { .. } => "volume",
        Commands::Amplifiers { .. } => "amplifiers",
        Commands::RateLimits => "rate-limits",
        Commands::Block { .. } => "block",
        Commands::Unblock { .. } => "unblock",
        Commands::Mute { .. } => "mute",
        Commands::Unmute { .. } => "unmute",
        Commands::Analyze { .. } => "analyze",
        Commands::Inspire { .. } => "inspire",
        Commands::Track { .. } => "track",
        Commands::Report { .. } => "report",
        Commands::Suggest { .. } => "suggest",
        Commands::Schedule { .. } => "schedule",
        Commands::Skill { .. } => "skill",
        Commands::Rails { .. } => "rails",
    }
}

/// Other names a deny rule may use for a command: its X-term alias, and
/// `post` for every command that publishes a post.
fn deny_aliases(cmd_name: &str) -> &'static [&'static str] {
    match cmd_name {
        "reply" | "quote" | "thread" => &["post"],
        "retweet" => &["repost"],
        "unretweet" => &["unrepost"],
        "retweeters" => &["reposters"],
        "conversation" => &["convo"],
        _ => &[],
    }
}

/// Check XMASTER_ALLOW_COMMANDS / XMASTER_DENY_COMMANDS env vars.
/// Deny takes precedence over allow. Agent-info and config are always
/// allowed so operators can still introspect and reconfigure.
fn check_command_access(cmd_name: &str) -> Result<(), XmasterError> {
    // Always allow introspection + configuration commands
    if matches!(cmd_name, "agent-info" | "config" | "rate-limits" | "update" | "skill") {
        return Ok(());
    }

    if let Ok(deny) = std::env::var("XMASTER_DENY_COMMANDS") {
        let names: Vec<&str> = std::iter::once(cmd_name).chain(deny_aliases(cmd_name).iter().copied()).collect();
        if let Some(rule) = deny
            .split(',')
            .map(str::trim)
            .find(|d| names.iter().any(|n| d.eq_ignore_ascii_case(n)))
        {
            return Err(XmasterError::CommandDenied(format!(
                "'{cmd_name}' is blocked by XMASTER_DENY_COMMANDS ({rule})"
            )));
        }
    }

    if let Ok(allow) = std::env::var("XMASTER_ALLOW_COMMANDS") {
        let allowed: Vec<&str> = allow.split(',').map(|s| s.trim()).collect();
        if !allowed.iter().any(|a| a.eq_ignore_ascii_case(cmd_name)) {
            return Err(XmasterError::CommandDenied(format!(
                "'{cmd_name}' is not in XMASTER_ALLOW_COMMANDS"
            )));
        }
    }

    Ok(())
}

/// A link to any post that needs no lookup of its author's handle.
pub(crate) fn post_url(id: &str) -> String {
    format!("https://x.com/i/status/{id}")
}

/// The route of a write whose path is fixed up front (`via` in results).
pub(crate) struct WriteRoute;

impl WriteRoute {
    /// Likes and follows: the web session when it's set up, else the API
    /// (no fallback either way, so this is the path taken).
    pub fn web_only(ctx: &AppContext) -> Option<&'static str> {
        Some(if ctx.config.web_first(true) { "web" } else { "api" })
    }
}

/// TRANSITIONAL until xapi.rs builds its errors with
/// `XmasterError::from_x_error` (lane transport): today every API 403 arrives
/// as `AuthMissing` ("HTTP 403: <body>"), telling agents to change keys.
/// Re-classify it from X's body: duplicate text, reply limits, protected
/// posts and removed endpoints become `Forbidden`/`ReplyRestricted` (exit 5)
/// or `NotFound`; token-permission 403s stay credentials problems.
pub(crate) fn reclassify_403(err: XmasterError) -> XmasterError {
    match err {
        XmasterError::AuthMissing { provider, message } if message.starts_with("HTTP 403: ") => {
            let body = &message["HTTP 403: ".len()..];
            let body = body.rsplit_once(" [txn: ").map_or(body, |(b, _)| b);
            XmasterError::from_x_error(provider, 403, body)
        }
        other => other,
    }
}

pub async fn dispatch(
    ctx: Arc<AppContext>,
    cli: &Cli,
    format: OutputFormat,
) -> Result<(), XmasterError> {
    // Command access guard — check allowlist/denylist before any work.
    let cmd_name = command_name(&cli.command);
    check_command_access(cmd_name)?;

    match &cli.command {
        Commands::Post { text, reply_to, quote, media, poll, poll_duration, dry_run } => {
            let draft = post::Draft {
                text,
                reply_to: reply_to.as_deref(),
                quote: quote.as_deref(),
                media,
                poll: poll.as_deref(),
                poll_duration: *poll_duration,
            };
            post::execute(ctx, format, draft, *dry_run).await
        }
        Commands::Delete { id } => engage::delete(ctx, format, id).await,
        Commands::Like { id } => engage::like(ctx, format, id).await,
        Commands::Unlike { id } => engage::unlike(ctx, format, id).await,
        Commands::Retweet { id } => engage::retweet(ctx, format, id).await,
        Commands::Unretweet { id } => engage::unretweet(ctx, format, id).await,
        Commands::Bookmark { id } => engage::bookmark(ctx, format, id).await,
        Commands::Unbookmark { id } => engage::unbookmark(ctx, format, id).await,
        Commands::Follow { username } => social::follow(ctx, format, username).await,
        Commands::Unfollow { username } => social::unfollow(ctx, format, username).await,
        Commands::Dm { action } => match action {
            DmCommands::Send { username, text } => dm::send(ctx, format, username, text).await,
            DmCommands::Inbox { count } => dm::inbox(ctx, format, *count).await,
            DmCommands::Thread { id, count } => dm::thread(ctx, format, id, *count).await,
        },
        Commands::Timeline { user, count, since, before, sort } => timeline::timeline(ctx, format, user.as_deref(), *count, since.as_deref(), before.as_deref(), sort.as_deref()).await,
        Commands::Mentions { count, since_id } => timeline::mentions(ctx, format, *count, since_id.as_deref()).await,
        Commands::Search { query, mode, count, since, before } => search::execute(ctx, format, query, mode, *count, since.as_deref(), before.as_deref()).await,
        Commands::SearchAi { query, count, from_date, to_date } => {
            search_ai::execute(ctx, format, query, *count, from_date.as_deref(), to_date.as_deref()).await
        }
        Commands::Trending { region, category, personalized } => {
            if *personalized {
                search_ai::personalized_trends(ctx, format).await
            } else {
                search_ai::trending(ctx, format, region.as_deref(), category.as_deref()).await
            }
        }
        Commands::User { username } => user::info(ctx, format, username).await,
        Commands::Me => user::me(ctx, format).await,
        Commands::Bookmarks { action } => match action {
            BookmarkCommands::List { count, unread } => bookmarks_cmd::list(ctx, format, *count, *unread).await,
            BookmarkCommands::Sync { count } => bookmarks_cmd::sync(ctx, format, *count).await,
            BookmarkCommands::Search { query } => bookmarks_cmd::search(format, query).await,
            BookmarkCommands::Export { output, unread } => bookmarks_cmd::export(format, output.as_deref(), *unread).await,
            BookmarkCommands::Digest { days } => bookmarks_cmd::digest(format, *days).await,
            BookmarkCommands::Stats => bookmarks_cmd::stats(format).await,
            BookmarkCommands::Folders => bookmarks_cmd::folders(ctx, format).await,
            BookmarkCommands::Folder { id, count } => bookmarks_cmd::folder(ctx, format, id, *count).await,
        },
        Commands::Followers { username, count } => social::followers(ctx, format, username, *count).await,
        Commands::Following { username, count } => social::following(ctx, format, username, *count).await,
        Commands::Config { action } => match action {
            ConfigCommands::Show => config_cmd::show(ctx, format).await,
            ConfigCommands::Get { key } => config_cmd::get(format, key).await,
            ConfigCommands::Set { key, value } => config_cmd::set(format, key, value).await,
            ConfigCommands::Check => config_cmd::check(ctx, format).await,
            ConfigCommands::Guide => { config_cmd::guide(format).await }
            ConfigCommands::Auth => { config_cmd::auth(ctx, format).await }
            ConfigCommands::WebLogin => { config_cmd::web_login(format).await }
        },
        Commands::AgentInfo => {
            agent_info::execute(format);
            Ok(())
        }
        Commands::Engage { action } => match action {
            EngageCommands::Inbox { id, count, quote_reply_count } => {
                engage_inbox::execute(ctx, format, id, *count, *quote_reply_count).await
            }
            EngageCommands::Recommend { topic, min_followers, count } => {
                engage_recommend::recommend(ctx, format, topic.as_deref(), *min_followers, *count).await
            }
            EngageCommands::Feed { topics, min_followers, max_age_mins, count } => {
                engage_recommend::feed(ctx, format, topics, *min_followers, *max_age_mins, *count).await
            }
            EngageCommands::Watchlist { action } => match action {
                WatchlistCommands::Add { username, topic } => {
                    engage_recommend::watchlist_add(ctx, format, username, topic.as_deref()).await
                }
                WatchlistCommands::List => engage_recommend::watchlist_list(format).await,
                WatchlistCommands::Remove { username } => engage_recommend::watchlist_remove(format, username).await,
            },
            EngageCommands::HotTargets { days, min_imps, min_profile_clicks, min_samples, count, sort } => {
                engage_recommend::hot_targets(format, *days, *min_imps, *min_profile_clicks, *min_samples, *count, sort).await
            }
            EngageCommands::Swarm { id, min_followers, max_followers, count } => {
                engage_recommend::swarm(ctx, format, id, *min_followers, *max_followers, *count).await
            }
        },
        Commands::Update { check } => update::execute(format, *check).await,
        Commands::Star => {
            crate::star_nudge::open_star_page();
            Ok(())
        }
        Commands::Thread { texts, media } => thread::execute(ctx, format, texts, media).await,
        Commands::Article { action } => match action {
            ArticleCommands::Preview {
                input,
                output,
                title,
                subtitle,
                header_image,
                author,
                handle,
                avatar,
                audience,
                open,
            } => {
                article::preview(
                    format,
                    input,
                    output.as_deref(),
                    title.as_deref(),
                    subtitle.as_deref(),
                    header_image.as_deref(),
                    author,
                    handle,
                    avatar.as_deref(),
                    audience,
                    *open,
                )
                .await
            }
            ArticleCommands::Draft {
                input,
                title,
                header_image,
                api,
            } => {
                article::draft(
                    ctx,
                    format,
                    input,
                    title.as_deref(),
                    header_image.as_deref(),
                    *api,
                )
                .await
            }
            ArticleCommands::Publish { article_id } => {
                article::publish(ctx, format, article_id).await
            }
        },
        Commands::Reply { id, text, media, dry_run } => {
            let draft = post::Draft { text, reply_to: Some(id), quote: None, media, poll: None, poll_duration: 1440 };
            post::execute(ctx, format, draft, *dry_run).await
        }
        Commands::Quote { id, text, media, dry_run } => {
            let draft = post::Draft { text, reply_to: None, quote: Some(id), media, poll: None, poll_duration: 1440 };
            post::execute(ctx, format, draft, *dry_run).await
        }
        Commands::Metrics { ids } => metrics::execute_batch(ctx, format, ids).await,
        Commands::Lists { action } => match action {
            ListCommands::Create { name, description } => {
                lists::create(ctx, format, name, description.as_deref()).await
            }
            ListCommands::Delete { id } => lists::delete(ctx, format, id).await,
            ListCommands::Add { list_id, username } => {
                lists::add_member(ctx, format, list_id, username).await
            }
            ListCommands::Remove { list_id, username } => {
                lists::remove_member(ctx, format, list_id, username).await
            }
            ListCommands::Timeline { list_id, count } => {
                lists::timeline(ctx, format, list_id, *count).await
            }
            ListCommands::Members { list_id, count } => {
                lists::members(ctx, format, list_id, *count).await
            }
            ListCommands::Mine { count } => lists::mine(ctx, format, *count).await,
        },
        Commands::HideReply { id } => moderation::hide_reply(ctx, format, id).await,
        Commands::UnhideReply { id } => moderation::unhide_reply(ctx, format, id).await,
        Commands::Read { id } => read_post::execute(ctx, format, id).await,
        Commands::Conversation { id, count } => conversation::execute(ctx, format, id, *count).await,
        Commands::Replies { id, count, all } => replies::execute(ctx, format, id, *count, *all).await,
        Commands::Likers { id, count } => tweet_engagement::likers(ctx, format, id, *count).await,
        Commands::Retweeters { id, count } => tweet_engagement::retweeters(ctx, format, id, *count).await,
        Commands::Quotes { id, count } => quotes::execute(ctx, format, id, *count).await,
        Commands::Users { usernames } => user::bulk(ctx, format, usernames).await,
        Commands::Volume { query, granularity } => volume::execute(ctx, format, query, granularity).await,
        Commands::Amplifiers { count } => amplifiers::execute(ctx, format, *count).await,
        Commands::RateLimits => rate_limits::execute(ctx, format).await,
        Commands::Block { username } => moderation::block(ctx, format, username).await,
        Commands::Unblock { username } => moderation::unblock(ctx, format, username).await,
        Commands::Mute { username } => moderation::mute(ctx, format, username).await,
        Commands::Unmute { username } => moderation::unmute(ctx, format, username).await,
        Commands::Analyze { text, goal, reply_to } => analyze::execute(ctx, format, text, goal.as_deref(), reply_to.as_deref()).await,
        Commands::Inspire { topic, author, min_likes, min_chars, long, count } => {
            // --long is sugar for --min-chars 500. Explicit --min-chars wins if both passed.
            let mc = min_chars.or(if *long { Some(500) } else { None });
            inspire::execute(ctx, format, topic.as_deref(), author.as_deref(), *min_likes, mc, *count).await
        },
        Commands::Track { action } => match action {
            TrackCommands::Run => track::track_run(ctx, format).await,
            TrackCommands::Status => track::track_status(ctx, format).await,
            TrackCommands::Followers => track::track_followers(ctx, format).await,
            TrackCommands::Growth { days } => track::follower_growth(ctx, format, *days).await,
        },
        Commands::Report { action } => match action {
            ReportCommands::Daily => report::daily(ctx, format).await,
            ReportCommands::Weekly => report::weekly(ctx, format).await,
        },
        Commands::Suggest { action } => match action {
            SuggestCommands::BestTime => suggest::best_time(ctx, format).await,
            SuggestCommands::NextPost => suggest::next_post(ctx, format).await,
        },
        Commands::Schedule { action } => match action {
            ScheduleCommands::Add { content, at, reply_to, quote, media } => {
                schedule::add(ctx, format, content, at, reply_to.as_deref(), quote.as_deref(), media).await
            }
            ScheduleCommands::List { status } => schedule::list(format, status.as_deref()).await,
            ScheduleCommands::Cancel { id } => schedule::cancel(format, id).await,
            ScheduleCommands::Reschedule { id, at } => schedule::reschedule(format, id, at).await,
            ScheduleCommands::Fire => schedule::fire(ctx, format).await,
            ScheduleCommands::Setup => schedule::setup(format).await,
        },
        Commands::Rails { action } => match action {
            RailsCommands::Status => rails_cmd::status(ctx, format).await,
            RailsCommands::Pause { hours } => rails_cmd::pause(format, *hours).await,
            RailsCommands::Resume => rails_cmd::resume(format).await,
            RailsCommands::ResetCooldown { yes } => rails_cmd::reset_cooldown(ctx, format, *yes).await,
        },
        Commands::Skill { action } => match action {
            SkillCommands::Install => skill_cmd::install(format).await,
            SkillCommands::Update => skill_cmd::update(format).await,
            SkillCommands::Status => skill_cmd::status(format).await,
        },
    }
}

#[cfg(test)]
mod access_tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn clear_access_env() {
        std::env::remove_var("XMASTER_ALLOW_COMMANDS");
        std::env::remove_var("XMASTER_DENY_COMMANDS");
    }

    #[test]
    fn deny_blocks_command() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_access_env();
        std::env::set_var("XMASTER_DENY_COMMANDS", "post,delete");
        let r = check_command_access("post");
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("blocked"));
        clear_access_env();
    }

    #[test]
    fn deny_allows_unlisted() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_access_env();
        std::env::set_var("XMASTER_DENY_COMMANDS", "post,delete");
        assert!(check_command_access("search").is_ok());
        clear_access_env();
    }

    #[test]
    fn allow_restricts_to_listed() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_access_env();
        std::env::set_var("XMASTER_ALLOW_COMMANDS", "search,read,metrics");
        assert!(check_command_access("search").is_ok());
        let r = check_command_access("post");
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("not in"));
        clear_access_env();
    }

    #[test]
    fn introspection_always_allowed() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_access_env();
        std::env::set_var("XMASTER_DENY_COMMANDS", "agent-info,config");
        assert!(check_command_access("agent-info").is_ok());
        assert!(check_command_access("config").is_ok());
        clear_access_env();
    }

    #[test]
    fn denying_post_covers_reply_quote_thread_and_x_terms_match() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_access_env();
        std::env::set_var("XMASTER_DENY_COMMANDS", "post,repost");
        for cmd in ["post", "reply", "quote", "thread", "retweet"] {
            assert!(check_command_access(cmd).is_err(), "{cmd} should be denied");
        }
        assert!(check_command_access("like").is_ok());
        clear_access_env();
    }

    #[test]
    fn api_403_is_reclassified_from_the_body() {
        let dup = XmasterError::AuthMissing {
            provider: "x",
            message: r#"HTTP 403: {"detail":"You are not allowed to create a Tweet with duplicate content.","type":"about:blank","title":"Forbidden","status":403} [txn: abc]"#.into(),
        };
        let e = reclassify_403(dup);
        assert_eq!(e.exit_code(), 5, "{e}");
        let keys = XmasterError::AuthMissing { provider: "x", message: "HTTP 401: Unauthorized".into() };
        assert_eq!(reclassify_403(keys).exit_code(), 2);
    }

    #[test]
    fn deny_takes_precedence_over_allow() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_access_env();
        std::env::set_var("XMASTER_ALLOW_COMMANDS", "post,search");
        std::env::set_var("XMASTER_DENY_COMMANDS", "post");
        let r = check_command_access("post");
        assert!(r.is_err());
        clear_access_env();
    }
}
