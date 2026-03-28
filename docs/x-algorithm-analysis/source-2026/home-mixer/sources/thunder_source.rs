use crate::candidate_pipeline::candidate::PostCandidate;
use crate::candidate_pipeline::query::ScoredPostsQuery;
use crate::clients::thunder_client::{ThunderClient, ThunderCluster};
use crate::params as p;
use std::sync::Arc;
use tonic::async_trait;
use xai_candidate_pipeline::source::Source;
use xai_home_mixer_proto as pb;
use xai_thunder_proto::GetInNetworkPostsRequest;
use xai_thunder_proto::in_network_posts_service_client::InNetworkPostsServiceClient;

pub struct ThunderSource {
    pub thunder_client: Arc<ThunderClient>,
}

#[async_trait]
impl Source<ScoredPostsQuery, PostCandidate> for ThunderSource {
    #[xai_stats_macro::receive_stats]
    async fn get_candidates(&self, query: &ScoredPostsQuery) -> Result<Vec<PostCandidate>, String> {
        let cluster = ThunderCluster::Amp;
        let channel = self
            .thunder_client
            .get_random_channel(cluster)
            .ok_or_else(|| "ThunderSource: no available channel".to_string())?;

        let mut client = InNetworkPostsServiceClient::new(channel.clone());
        let following_list = &query.user_features.followed_user_ids;
        let request = GetInNetworkPostsRequest {
            user_id: query.user_id as u64,
            following_user_ids: following_list.iter().map(|&id| id as u64).collect(),
            max_results: p::THUNDER_MAX_RESULTS,
            exclude_tweet_ids: vec![],
            algorithm: "default".to_string(),
            debug: false,
            is_video_request: false,
        };

        let response = client
            .get_in_network_posts(request)
            .await
            .map_err(|e| format!("ThunderSource: {}", e))?;

        let candidates: Vec<PostCandidate> = response
            .into_inner()
            .posts
            .into_iter()
            .map(|post| {
                let in_reply_to_tweet_id = post
                    .in_reply_to_post_id
                    .and_then(|id| u64::try_from(id).ok());
                let conversation_id = post.conversation_id.and_then(|id| u64::try_from(id).ok());

                let mut ancestors = Vec::new();
                if let Some(reply_to) = in_reply_to_tweet_id {
                    ancestors.push(reply_to);
                    if let Some(root) = conversation_id.filter(|&root| root != reply_to) {
                        ancestors.push(root);
                    }
                }

                PostCandidate {
                    tweet_id: post.post_id,
                    author_id: post.author_id as u64,
                    in_reply_to_tweet_id,
                    ancestors,
                    served_type: Some(pb::ServedType::ForYouInNetwork),
                    ..Default::default()
                }
            })
            .collect();

        Ok(candidates)
    }
}
