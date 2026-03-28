use crate::candidate_pipeline::candidate::PostCandidate;
use crate::candidate_pipeline::query::ScoredPostsQuery;
use crate::clients::gizmoduck_client::GizmoduckClient;
use std::sync::Arc;
use tonic::async_trait;
use xai_candidate_pipeline::hydrator::Hydrator;

pub struct GizmoduckCandidateHydrator {
    pub gizmoduck_client: Arc<dyn GizmoduckClient + Send + Sync>,
}

impl GizmoduckCandidateHydrator {
    pub async fn new(gizmoduck_client: Arc<dyn GizmoduckClient + Send + Sync>) -> Self {
        Self { gizmoduck_client }
    }
}

#[async_trait]
impl Hydrator<ScoredPostsQuery, PostCandidate> for GizmoduckCandidateHydrator {
    #[xai_stats_macro::receive_stats]
    async fn hydrate(
        &self,
        _query: &ScoredPostsQuery,
        candidates: &[PostCandidate],
    ) -> Result<Vec<PostCandidate>, String> {
        let client = &self.gizmoduck_client;

        let author_ids: Vec<_> = candidates.iter().map(|c| c.author_id).collect();
        let author_ids: Vec<_> = author_ids.iter().map(|&x| x as i64).collect();
        let retweet_user_ids: Vec<_> = candidates.iter().map(|c| c.retweeted_user_id).collect();
        let retweet_user_ids: Vec<_> = retweet_user_ids.iter().flatten().collect();
        let retweet_user_ids: Vec<_> = retweet_user_ids.iter().map(|&&x| x as i64).collect();

        let mut user_ids_to_fetch = Vec::with_capacity(author_ids.len() + retweet_user_ids.len());
        user_ids_to_fetch.extend(author_ids);
        user_ids_to_fetch.extend(retweet_user_ids);
        user_ids_to_fetch.dedup();

        let users = client.get_users(user_ids_to_fetch).await;
        let users = users.map_err(|e| e.to_string())?;

        let mut hydrated_candidates = Vec::with_capacity(candidates.len());

        for candidate in candidates {
            let user = users
                .get(&(candidate.author_id as i64))
                .and_then(|user| user.as_ref());
            let user_counts = user.and_then(|user| user.user.as_ref().map(|u| &u.counts));
            let user_profile = user.and_then(|user| user.user.as_ref().map(|u| &u.profile));

            let author_followers_count: Option<i32> =
                user_counts.map(|x| x.followers_count).map(|x| x as i32);
            let author_screen_name: Option<String> = user_profile.map(|x| x.screen_name.clone());

            let retweet_user = candidate
                .retweeted_user_id
                .and_then(|retweeted_user_id| users.get(&(retweeted_user_id as i64)))
                .and_then(|user| user.as_ref());
            let retweet_profile =
                retweet_user.and_then(|user| user.user.as_ref().map(|u| &u.profile));
            let retweeted_screen_name: Option<String> =
                retweet_profile.map(|x| x.screen_name.clone());

            let hydrated = PostCandidate {
                author_followers_count,
                author_screen_name,
                retweeted_screen_name,
                ..Default::default()
            };
            hydrated_candidates.push(hydrated);
        }

        Ok(hydrated_candidates)
    }

    fn update(&self, candidate: &mut PostCandidate, hydrated: PostCandidate) {
        candidate.author_followers_count = hydrated.author_followers_count;
        candidate.author_screen_name = hydrated.author_screen_name;
        candidate.retweeted_screen_name = hydrated.retweeted_screen_name;
    }
}
