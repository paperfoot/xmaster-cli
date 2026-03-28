use crate::candidate_pipeline::candidate::PostCandidate;
use crate::candidate_pipeline::query::ScoredPostsQuery;
use futures::future::join;
use std::collections::HashMap;
use std::sync::Arc;
use tonic::async_trait;
use xai_candidate_pipeline::hydrator::Hydrator;
use xai_twittercontext_proto::GetTwitterContextViewer;
use xai_twittercontext_proto::TwitterContextViewer;
use xai_visibility_filtering::models::FilteredReason;
use xai_visibility_filtering::vf_client::SafetyLevel;
use xai_visibility_filtering::vf_client::SafetyLevel::{TimelineHome, TimelineHomeRecommendations};
use xai_visibility_filtering::vf_client::VisibilityFilteringClient;

pub struct VFCandidateHydrator {
    pub vf_client: Arc<dyn VisibilityFilteringClient + Send + Sync>,
}

impl VFCandidateHydrator {
    pub async fn new(vf_client: Arc<dyn VisibilityFilteringClient + Send + Sync>) -> Self {
        Self { vf_client }
    }

    async fn fetch_vf_results(
        client: &Arc<dyn VisibilityFilteringClient + Send + Sync>,
        tweet_ids: Vec<i64>,
        safety_level: SafetyLevel,
        for_user_id: i64,
        context: Option<TwitterContextViewer>,
    ) -> Result<HashMap<i64, Option<FilteredReason>>, String> {
        if tweet_ids.is_empty() {
            return Ok(HashMap::new());
        }

        client
            .get_result(tweet_ids, safety_level, for_user_id, context)
            .await
            .map_err(|e| e.to_string())
    }
}

#[async_trait]
impl Hydrator<ScoredPostsQuery, PostCandidate> for VFCandidateHydrator {
    #[xai_stats_macro::receive_stats]
    async fn hydrate(
        &self,
        query: &ScoredPostsQuery,
        candidates: &[PostCandidate],
    ) -> Result<Vec<PostCandidate>, String> {
        let context = query.get_viewer();
        let user_id = query.user_id;
        let client = &self.vf_client;

        let mut in_network_ids = Vec::new();
        let mut oon_ids = Vec::new();
        for candidate in candidates.iter() {
            if candidate.in_network.unwrap_or(false) {
                in_network_ids.push(candidate.tweet_id);
            } else {
                oon_ids.push(candidate.tweet_id);
            }
        }

        let in_network_future = Self::fetch_vf_results(
            client,
            in_network_ids,
            TimelineHome,
            user_id,
            context.clone(),
        );

        let oon_future = Self::fetch_vf_results(
            client,
            oon_ids,
            TimelineHomeRecommendations,
            user_id,
            context,
        );

        let (in_network_result, oon_result) = join(in_network_future, oon_future).await;
        let mut result: HashMap<i64, Option<FilteredReason>> = HashMap::new();
        result.extend(in_network_result?);
        result.extend(oon_result?);

        let mut hydrated_candidates = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let visibility_reason = result.get(&candidate.tweet_id);
            let visibility_reason = visibility_reason.unwrap_or(&None);
            let hydrated = PostCandidate {
                visibility_reason: visibility_reason.clone(),
                ..Default::default()
            };
            hydrated_candidates.push(hydrated);
        }
        Ok(hydrated_candidates)
    }

    fn update(&self, candidate: &mut PostCandidate, hydrated: PostCandidate) {
        candidate.visibility_reason = hydrated.visibility_reason;
    }
}
