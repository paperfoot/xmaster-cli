use crate::candidate_pipeline::candidate::PostCandidate;
use crate::candidate_pipeline::query::ScoredPostsQuery;
use tonic::async_trait;
use xai_candidate_pipeline::filter::{Filter, FilterResult};

// Remove candidates that are blocked or muted by the viewer
pub struct AuthorSocialgraphFilter;

#[async_trait]
impl Filter<ScoredPostsQuery, PostCandidate> for AuthorSocialgraphFilter {
    async fn filter(
        &self,
        query: &ScoredPostsQuery,
        candidates: Vec<PostCandidate>,
    ) -> Result<FilterResult<PostCandidate>, String> {
        let viewer_blocked_user_ids = query.user_features.blocked_user_ids.clone();
        let viewer_muted_user_ids = query.user_features.muted_user_ids.clone();

        if viewer_blocked_user_ids.is_empty() && viewer_muted_user_ids.is_empty() {
            return Ok(FilterResult {
                kept: candidates,
                removed: Vec::new(),
            });
        }

        let mut kept: Vec<PostCandidate> = Vec::new();
        let mut removed: Vec<PostCandidate> = Vec::new();

        for candidate in candidates {
            let author_id = candidate.author_id as i64;
            let muted = viewer_muted_user_ids.contains(&author_id);
            let blocked = viewer_blocked_user_ids.contains(&author_id);
            if muted || blocked {
                removed.push(candidate);
            } else {
                kept.push(candidate);
            }
        }

        Ok(FilterResult { kept, removed })
    }
}
