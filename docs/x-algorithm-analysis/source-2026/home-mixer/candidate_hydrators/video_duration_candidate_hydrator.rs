use crate::candidate_pipeline::candidate::PostCandidate;
use crate::candidate_pipeline::candidate_features::MediaInfo;
use crate::candidate_pipeline::query::ScoredPostsQuery;
use crate::clients::tweet_entity_service_client::TESClient;
use std::sync::Arc;
use tonic::async_trait;
use xai_candidate_pipeline::hydrator::Hydrator;

pub struct VideoDurationCandidateHydrator {
    pub tes_client: Arc<dyn TESClient + Send + Sync>,
}

impl VideoDurationCandidateHydrator {
    pub async fn new(tes_client: Arc<dyn TESClient + Send + Sync>) -> Self {
        Self { tes_client }
    }
}

#[async_trait]
impl Hydrator<ScoredPostsQuery, PostCandidate> for VideoDurationCandidateHydrator {
    #[xai_stats_macro::receive_stats]
    async fn hydrate(
        &self,
        _query: &ScoredPostsQuery,
        candidates: &[PostCandidate],
    ) -> Result<Vec<PostCandidate>, String> {
        let client = &self.tes_client;

        let tweet_ids = candidates.iter().map(|c| c.tweet_id).collect::<Vec<_>>();

        let post_features = client.get_tweet_media_entities(tweet_ids.clone()).await;
        let post_features = post_features.map_err(|e| e.to_string())?;

        let mut hydrated_candidates = Vec::with_capacity(candidates.len());
        for tweet_id in tweet_ids {
            let post_features = post_features.get(&tweet_id);
            let media_entities = post_features.and_then(|x| x.as_ref());

            let video_duration_ms = media_entities.and_then(|entities| {
                entities.iter().find_map(|entity| {
                    if let Some(MediaInfo::VideoInfo(video_info)) = &entity.media_info {
                        Some(video_info.duration_millis)
                    } else {
                        None
                    }
                })
            });

            let hydrated = PostCandidate {
                video_duration_ms,
                ..Default::default()
            };
            hydrated_candidates.push(hydrated);
        }

        Ok(hydrated_candidates)
    }

    fn update(&self, candidate: &mut PostCandidate, hydrated: PostCandidate) {
        candidate.video_duration_ms = hydrated.video_duration_ms;
    }
}
