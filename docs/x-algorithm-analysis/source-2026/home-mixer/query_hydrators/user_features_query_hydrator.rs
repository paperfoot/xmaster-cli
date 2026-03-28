use crate::candidate_pipeline::query::ScoredPostsQuery;
use crate::candidate_pipeline::query_features::UserFeatures;
use crate::clients::strato_client::StratoClient;
use std::sync::Arc;
use tonic::async_trait;
use xai_candidate_pipeline::query_hydrator::QueryHydrator;
use xai_strato::{StratoResult, StratoValue, decode};

pub struct UserFeaturesQueryHydrator {
    pub strato_client: Arc<dyn StratoClient + Send + Sync>,
}

#[async_trait]
impl QueryHydrator<ScoredPostsQuery> for UserFeaturesQueryHydrator {
    #[xai_stats_macro::receive_stats]
    async fn hydrate(&self, query: &ScoredPostsQuery) -> Result<ScoredPostsQuery, String> {
        let user_id = query.user_id;
        let client = &self.strato_client;
        let result = client.get_user_features(user_id);
        let result = result.await.map_err(|e| e.to_string())?;
        let decoded: StratoResult<StratoValue<UserFeatures>> = decode(&result);
        match decoded {
            StratoResult::Ok(v) => {
                let user_features = v.v.unwrap_or_default();
                Ok(ScoredPostsQuery {
                    user_features,
                    ..Default::default()
                })
            }
            StratoResult::Err(_) => Err("Error received from strato".to_string()),
        }
    }

    fn update(&self, query: &mut ScoredPostsQuery, hydrated: ScoredPostsQuery) {
        query.user_features = hydrated.user_features;
    }

    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}
