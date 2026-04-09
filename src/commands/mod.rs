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
pub mod skill_cmd;
pub mod replies;
pub mod read_post;
pub mod inspire;
pub mod tweet_engagement;
pub mod quotes;

use crate::cli::{Cli, Commands, ConfigCommands, DmCommands, EngageCommands, WatchlistCommands, ListCommands, TrackCommands, ReportCommands, SuggestCommands, ScheduleCommands, BookmarkCommands, SkillCommands};
use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::output::OutputFormat;
use std::sync::Arc;

pub async fn dispatch(
    ctx: Arc<AppContext>,
    cli: &Cli,
    format: OutputFormat,
) -> Result<(), XmasterError> {
    match &cli.command {
        Commands::Post { text, reply_to, quote, media, poll, poll_duration } => {
            post::execute(ctx, format, text, reply_to.as_deref(), quote.as_deref(), media, poll.as_deref(), *poll_duration).await
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
        Commands::Trending { region, category } => search_ai::trending(ctx, format, region.as_deref(), category.as_deref()).await,
        Commands::User { username } => user::info(ctx, format, username).await,
        Commands::Me => user::me(ctx, format).await,
        Commands::Bookmarks { action } => match action {
            BookmarkCommands::List { count, unread } => bookmarks_cmd::list(ctx, format, *count, *unread).await,
            BookmarkCommands::Sync { count } => bookmarks_cmd::sync(ctx, format, *count).await,
            BookmarkCommands::Search { query } => bookmarks_cmd::search(format, query).await,
            BookmarkCommands::Export { output, unread } => bookmarks_cmd::export(format, output.as_deref(), *unread).await,
            BookmarkCommands::Digest { days } => bookmarks_cmd::digest(format, *days).await,
            BookmarkCommands::Stats => bookmarks_cmd::stats(format).await,
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
        },
        Commands::Update { check } => update::execute(*check).await,
        Commands::Star => {
            crate::star_nudge::open_star_page();
            Ok(())
        }
        Commands::Thread { texts, media } => thread::execute(ctx, format, texts, media).await,
        Commands::Reply { id, text, media } => {
            post::execute(ctx, format, text, Some(id.as_str()), None, media, None, 1440).await
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
        Commands::Replies { id, count } => replies::execute(ctx, format, id, *count).await,
        Commands::Likers { id, count } => tweet_engagement::likers(ctx, format, id, *count).await,
        Commands::Retweeters { id, count } => tweet_engagement::retweeters(ctx, format, id, *count).await,
        Commands::Quotes { id, count } => quotes::execute(ctx, format, id, *count).await,
        Commands::Users { usernames } => user::bulk(ctx, format, usernames).await,
        Commands::RateLimits => rate_limits::execute(ctx, format).await,
        Commands::Block { username } => moderation::block(ctx, format, username).await,
        Commands::Unblock { username } => moderation::unblock(ctx, format, username).await,
        Commands::Mute { username } => moderation::mute(ctx, format, username).await,
        Commands::Unmute { username } => moderation::unmute(ctx, format, username).await,
        Commands::Analyze { text, goal } => analyze::execute(ctx, format, text, goal.as_deref()).await,
        Commands::Inspire { topic, author, min_likes, count } =>
            inspire::execute(ctx, format, topic.as_deref(), author.as_deref(), *min_likes, *count).await,
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
        Commands::Skill { action } => match action {
            SkillCommands::Install => skill_cmd::install(format).await,
            SkillCommands::Update => skill_cmd::update(format).await,
            SkillCommands::Status => skill_cmd::status(format).await,
        },
    }
}
