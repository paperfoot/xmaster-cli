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

use crate::cli::{Cli, Commands, ConfigCommands, DmCommands, ListCommands, TrackCommands, ReportCommands, SuggestCommands, ScheduleCommands};
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
        Commands::Timeline { user, count } => timeline::timeline(ctx, format, user.as_deref(), *count).await,
        Commands::Mentions { count } => timeline::mentions(ctx, format, *count).await,
        Commands::Search { query, mode, count } => search::execute(ctx, format, query, mode, *count).await,
        Commands::SearchAi { query, count, from_date, to_date } => {
            search_ai::execute(ctx, format, query, *count, from_date.as_deref(), to_date.as_deref()).await
        }
        Commands::Trending { region, category } => search_ai::trending(ctx, format, region.as_deref(), category.as_deref()).await,
        Commands::User { username } => user::info(ctx, format, username).await,
        Commands::Me => user::me(ctx, format).await,
        Commands::Bookmarks { count } => timeline::bookmarks(ctx, format, *count).await,
        Commands::Followers { username, count } => social::followers(ctx, format, username, *count).await,
        Commands::Following { username, count } => social::following(ctx, format, username, *count).await,
        Commands::Config { action } => match action {
            ConfigCommands::Show => config_cmd::show(ctx, format).await,
            ConfigCommands::Set { key, value } => config_cmd::set(format, key, value).await,
            ConfigCommands::Check => config_cmd::check(ctx, format).await,
        },
        Commands::AgentInfo => {
            agent_info::execute(format);
            Ok(())
        }
        Commands::Update { check } => update::execute(*check).await,
        Commands::Thread { texts, media } => thread::execute(ctx, format, texts, media).await,
        Commands::Metrics { id } => metrics::execute(ctx, format, id).await,
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
            ListCommands::Mine { count } => lists::mine(ctx, format, *count).await,
        },
        Commands::HideReply { id } => moderation::hide_reply(ctx, format, id).await,
        Commands::UnhideReply { id } => moderation::unhide_reply(ctx, format, id).await,
        Commands::RateLimits => rate_limits::execute(ctx, format).await,
        Commands::Block { username } => moderation::block(ctx, format, username).await,
        Commands::Unblock { username } => moderation::unblock(ctx, format, username).await,
        Commands::Mute { username } => moderation::mute(ctx, format, username).await,
        Commands::Unmute { username } => moderation::unmute(ctx, format, username).await,
        Commands::Analyze { text, goal } => analyze::execute(ctx, format, text, goal.as_deref()).await,
        Commands::Track { action } => match action {
            TrackCommands::Run => track::track_run(ctx, format).await,
            TrackCommands::Status => track::track_status(ctx, format).await,
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
    }
}
