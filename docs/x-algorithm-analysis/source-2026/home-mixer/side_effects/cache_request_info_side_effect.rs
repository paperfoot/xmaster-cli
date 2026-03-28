use crate::candidate_pipeline::candidate::PostCandidate;
use crate::candidate_pipeline::query::ScoredPostsQuery;
use crate::clients::strato_client::StratoClient;
use std::env;
use std::sync::Arc;
use tonic::async_trait;
use xai_candidate_pipeline::side_effect::{SideEffect, SideEffectInput};
use xai_strato::{StratoResult, StratoValue, decode};

pub struct CacheRequestInfoSideEffect {
    pub strato_client: Arc<dyn StratoClient + Send + Sync>,
}

#[async_trait]
impl SideEffect<ScoredPostsQuery, PostCandidate> for CacheRequestInfoSideEffect {
    fn enable(&self, query: Arc<ScoredPostsQuery>) -> bool {
        env::var("APP_ENV").unwrap_or_default() == "prod" && !query.in_network_only
    }

    async fn run(
        &self,
        input: Arc<SideEffectInput<ScoredPostsQuery, PostCandidate>>,
    ) -> Result<(), String> {
        let user_id: i64 = input.query.user_id;

        let post_ids: Vec<i64> = input
            .selected_candidates
            .iter()
            .map(|c| c.tweet_id)
            .collect();
        let client = &self.strato_client;
        let res = client
            .store_request_info(user_id, post_ids)
            .await
            .map_err(|e| e.to_string())?;
        let decoded: StratoResult<StratoValue<()>> = decode(&res);
        match decoded {
            StratoResult::Ok(_) => Ok(()),
            StratoResult::Err(_) => Err("error received from strato".to_string()),
        }
    }
}
