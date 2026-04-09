use crate::context::AppContext;
use crate::errors::XmasterError;
use crate::output::{self, OutputFormat, Tableable};
use crate::providers::xai::XaiSearch;
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
struct AiSearchResult {
    text: String,
    citations: Vec<String>,
}

impl Tableable for AiSearchResult {
    fn to_table(&self) -> comfy_table::Table {
        let mut table = comfy_table::Table::new();
        table.set_header(vec!["AI Search Results"]);
        table.add_row(vec![&self.text]);
        if !self.citations.is_empty() {
            table.add_row(vec![""]);
            table.add_row(vec!["--- Citations ---"]);
            for url in &self.citations {
                table.add_row(vec![url.as_str()]);
            }
        }
        table
    }
}

pub async fn execute(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    query: &str,
    count: usize,
    from_date: Option<&str>,
    to_date: Option<&str>,
) -> Result<(), XmasterError> {
    let xai = XaiSearch::new(ctx.clone());
    let result = xai.search_posts(query, count, None, from_date, to_date).await?;
    let display = AiSearchResult {
        text: result.text,
        citations: result.citations,
    };
    output::render(format, &display, None);
    Ok(())
}

pub async fn trending(
    ctx: Arc<AppContext>,
    format: OutputFormat,
    region: Option<&str>,
    category: Option<&str>,
) -> Result<(), XmasterError> {
    let xai = XaiSearch::new(ctx.clone());
    let result = xai.get_trending(region, category).await?;
    let display = AiSearchResult {
        text: result.text,
        citations: result.citations,
    };
    output::render(format, &display, None);
    Ok(())
}

/// Personalized trends via the X API (not xAI). Requires X Premium.
pub async fn personalized_trends(
    ctx: Arc<AppContext>,
    format: OutputFormat,
) -> Result<(), XmasterError> {
    let api = crate::providers::xapi::XApi::new(ctx.clone());
    let trends = api.get_personalized_trends().await?;

    if trends.is_empty() {
        return Err(XmasterError::NotFound(
            "No personalized trends returned. Ensure your account has X Premium.".into(),
        ));
    }

    #[derive(Serialize)]
    struct TrendsResult {
        source: String,
        count: usize,
        trends: Vec<crate::providers::xapi::TrendData>,
    }
    impl Tableable for TrendsResult {
        fn to_table(&self) -> comfy_table::Table {
            let mut table = comfy_table::Table::new();
            table.set_header(vec!["Trend", "Tweets", "Description"]);
            for t in &self.trends {
                table.add_row(vec![
                    t.name.clone(),
                    t.tweet_count.map(|c| c.to_string()).unwrap_or_else(|| "—".into()),
                    t.description.clone().unwrap_or_default(),
                ]);
            }
            table
        }
    }

    let display = TrendsResult {
        source: "personalized".into(),
        count: trends.len(),
        trends,
    };
    output::render(format, &display, None);
    Ok(())
}
