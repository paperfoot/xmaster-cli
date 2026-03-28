use crate::candidate_pipeline::candidate::PostCandidate;
use crate::candidate_pipeline::query::ScoredPostsQuery;
use std::collections::HashSet;
use tonic::async_trait;
use xai_candidate_pipeline::filter::{Filter, FilterResult};

/// Filters out subscription-only posts from authors the viewer is not subscribed to.
pub struct IneligibleSubscriptionFilter;

#[async_trait]
impl Filter<ScoredPostsQuery, PostCandidate> for IneligibleSubscriptionFilter {
    async fn filter(
        &self,
        query: &ScoredPostsQuery,
        candidates: Vec<PostCandidate>,
    ) -> Result<FilterResult<PostCandidate>, String> {
        let subscribed_user_ids: HashSet<u64> = query
            .user_features
            .subscribed_user_ids
            .iter()
            .map(|id| *id as u64)
            .collect();

        let (kept, removed): (Vec<_>, Vec<_>) =
            candidates
                .into_iter()
                .partition(|candidate| match candidate.subscription_author_id {
                    Some(author_id) => subscribed_user_ids.contains(&author_id),
                    None => true,
                });

        Ok(FilterResult { kept, removed })
    }
}
