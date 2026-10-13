use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::output::{self, OutputFormat, Tableable};
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
struct CountBucket {
    start: String,
    end: String,
    tweet_count: u64,
}

#[derive(Serialize)]
struct VolumeResult {
    query: String,
    granularity: String,
    total_tweets: u64,
    buckets: Vec<CountBucket>,
}

impl Tableable for VolumeResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["Start", "End", "Tweets"]);
        for b in &self.buckets {
            table.add_row(vec![&b.start, &b.end, &b.tweet_count.to_string()]);
        }
        table.add_row(vec!["TOTAL", "", &self.total_tweets.to_string()]);
        table
    }
}

/// Show recent tweet volume for a query, bucketed by time.
/// Uses OAuth 2.0 App-Only auth (this endpoint rejects OAuth 1.0a User Context).
pub async fn execute(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    query: &str,
    granularity: &str,
) -> Result<(), XmasterError> {
    let token = crate::providers::oauth2::get_app_only_bearer(&ctx.config).await?;
    let encoded = percent_encoding::utf8_percent_encode(
        query,
        percent_encoding::NON_ALPHANUMERIC,
    );
    let url = format!(
        "https://api.x.com/2/tweets/counts/recent?query={encoded}&granularity={granularity}"
    );
    let json = crate::providers::oauth2::oauth2_get(&url, &token).await?;

    let data = json
        .get("data")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();

    let buckets: Vec<CountBucket> = data
        .into_iter()
        .filter_map(|b| {
            Some(CountBucket {
                start: b.get("start")?.as_str()?.to_string(),
                end: b.get("end")?.as_str()?.to_string(),
                tweet_count: b.get("tweet_count")?.as_u64()?,
            })
        })
        .collect();

    let total: u64 = buckets.iter().map(|b| b.tweet_count).sum();

    let result = VolumeResult {
        query: query.to_string(),
        granularity: granularity.to_string(),
        total_tweets: total,
        buckets,
    };
    if result.buckets.is_empty() {
        output::render_hint(format, &result, "No posts matched in the last 7 days; broaden the query");
    } else {
        output::render(format, &result, None);
    }
    Ok(())
}
