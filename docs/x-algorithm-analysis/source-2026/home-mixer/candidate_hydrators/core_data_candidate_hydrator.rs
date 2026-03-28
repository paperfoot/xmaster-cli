use crate::candidate_pipeline::candidate::PostCandidate;
use crate::candidate_pipeline::query::ScoredPostsQuery;
use crate::clients::tweet_entity_service_client::TESClient;
use std::sync::Arc;
use tonic::async_trait;
use xai_candidate_pipeline::hydrator::Hydrator;

pub struct CoreDataCandidateHydrator {
    pub tes_client: Arc<dyn TESClient + Send + Sync>,
}

impl CoreDataCandidateHydrator {
    pub async fn new(tes_client: Arc<dyn TESClient + Send + Sync>) -> Self {
        Self { tes_client }
    }
}

#[async_trait]
impl Hydrator<ScoredPostsQuery, PostCandidate> for CoreDataCandidateHydrator {
    #[xai_stats_macro::receive_stats]
    async fn hydrate(
        &self,
        _query: &ScoredPostsQuery,
        candidates: &[PostCandidate],
    ) -> Result<Vec<PostCandidate>, String> {
        let client = &self.tes_client;

        let tweet_ids = candidates.iter().map(|c| c.tweet_id).collect::<Vec<_>>();

        let post_features = client.get_tweet_core_datas(tweet_ids.clone()).await;
        let post_features = post_features.map_err(|e| e.to_string())?;

        let mut hydrated_candidates = Vec::with_capacity(candidates.len());
        for tweet_id in tweet_ids {
            let post_features = post_features.get(&tweet_id);
            let core_data = post_features.and_then(|x| x.as_ref());
            let text = core_data.map(|x| x.text.clone());
            let hydrated = PostCandidate {
                author_id: core_data.map(|x| x.author_id).unwrap_or_default(),
                retweeted_user_id: core_data.and_then(|x| x.source_user_id),
                retweeted_tweet_id: core_data.and_then(|x| x.source_tweet_id),
                in_reply_to_tweet_id: core_data.and_then(|x| x.in_reply_to_tweet_id),
                tweet_text: text.unwrap_or_default(),
                ..Default::default()
            };
            hydrated_candidates.push(hydrated);
        }

        Ok(hydrated_candidates)
    }

    fn update(&self, candidate: &mut PostCandidate, hydrated: PostCandidate) {
        candidate.retweeted_user_id = hydrated.retweeted_user_id;
        candidate.retweeted_tweet_id = hydrated.retweeted_tweet_id;
        candidate.in_reply_to_tweet_id = hydrated.in_reply_to_tweet_id;
        candidate.tweet_text = hydrated.tweet_text;
    }
}
