use crate::candidate_pipeline::candidate::PostCandidate;
use crate::candidate_pipeline::query::ScoredPostsQuery;
use tonic::async_trait;
use xai_candidate_pipeline::filter::{Filter, FilterResult};

pub struct CoreDataHydrationFilter;

#[async_trait]
impl Filter<ScoredPostsQuery, PostCandidate> for CoreDataHydrationFilter {
    async fn filter(
        &self,
        _query: &ScoredPostsQuery,
        candidates: Vec<PostCandidate>,
    ) -> Result<FilterResult<PostCandidate>, String> {
        let (kept, removed) = candidates
            .into_iter()
            .partition(|c| c.author_id != 0 && !c.tweet_text.trim().is_empty());
        Ok(FilterResult { kept, removed })
    }
}
