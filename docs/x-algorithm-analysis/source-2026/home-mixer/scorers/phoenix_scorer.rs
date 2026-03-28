use crate::candidate_pipeline::candidate::{PhoenixScores, PostCandidate};
use crate::candidate_pipeline::query::ScoredPostsQuery;
use crate::clients::phoenix_prediction_client::PhoenixPredictionClient;
use crate::util::request_util;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tonic::async_trait;
use xai_candidate_pipeline::scorer::Scorer;
use xai_recsys_proto::{ActionName, ContinuousActionName};

pub struct PhoenixScorer {
    pub phoenix_client: Arc<dyn PhoenixPredictionClient + Send + Sync>,
}

#[async_trait]
impl Scorer<ScoredPostsQuery, PostCandidate> for PhoenixScorer {
    #[xai_stats_macro::receive_stats]
    async fn score(
        &self,
        query: &ScoredPostsQuery,
        candidates: &[PostCandidate],
    ) -> Result<Vec<PostCandidate>, String> {
        let user_id = query.user_id as u64;
        let prediction_request_id = request_util::generate_request_id();
        let last_scored_at_ms = Self::current_timestamp_millis();

        if let Some(sequence) = &query.user_action_sequence {
            let tweet_infos: Vec<xai_recsys_proto::TweetInfo> = candidates
                .iter()
                .map(|c| {
                    let tweet_id = c.retweeted_tweet_id.unwrap_or(c.tweet_id as u64);
                    let author_id = c.retweeted_user_id.unwrap_or(c.author_id);
                    xai_recsys_proto::TweetInfo {
                        tweet_id,
                        author_id,
                        ..Default::default()
                    }
                })
                .collect();

            let result = self
                .phoenix_client
                .predict(user_id, sequence.clone(), tweet_infos)
                .await;

            if let Ok(response) = result {
                let predictions_map = self.build_predictions_map(&response);

                let scored_candidates = candidates
                    .iter()
                    .map(|c| {
                        // For retweets, look up predictions using the original tweet id
                        let lookup_tweet_id = c.retweeted_tweet_id.unwrap_or(c.tweet_id as u64);

                        let phoenix_scores = predictions_map
                            .get(&lookup_tweet_id)
                            .map(|preds| self.extract_phoenix_scores(preds))
                            .unwrap_or_default();

                        PostCandidate {
                            phoenix_scores,
                            prediction_request_id: Some(prediction_request_id),
                            last_scored_at_ms,
                            ..Default::default()
                        }
                    })
                    .collect();

                return Ok(scored_candidates);
            }
        }

        // Return candidates unchanged if no scoring could be done
        Ok(candidates.to_vec())
    }

    fn update(&self, candidate: &mut PostCandidate, scored: PostCandidate) {
        candidate.phoenix_scores = scored.phoenix_scores;
        candidate.prediction_request_id = scored.prediction_request_id;
        candidate.last_scored_at_ms = scored.last_scored_at_ms;
    }
}

impl PhoenixScorer {
    /// Builds Map[tweet_id -> ActionPredictions]
    fn build_predictions_map(
        &self,
        response: &xai_recsys_proto::PredictNextActionsResponse,
    ) -> HashMap<u64, ActionPredictions> {
        let mut predictions_map = HashMap::new();

        let Some(distribution_set) = response.distribution_sets.first() else {
            return predictions_map;
        };

        for distribution in &distribution_set.candidate_distributions {
            let Some(candidate) = &distribution.candidate else {
                continue;
            };
            let tweet_id = candidate.tweet_id;

            let action_probs: HashMap<usize, f64> = distribution
                .top_log_probs
                .iter()
                .enumerate()
                .map(|(idx, log_prob)| (idx, (*log_prob as f64).exp()))
                .collect();

            let continuous_values: HashMap<usize, f64> = distribution
                .continuous_actions_values
                .iter()
                .enumerate()
                .map(|(idx, value)| (idx, *value as f64))
                .collect();

            predictions_map.insert(
                tweet_id,
                ActionPredictions {
                    action_probs,
                    continuous_values,
                },
            );
        }

        predictions_map
    }

    fn extract_phoenix_scores(&self, p: &ActionPredictions) -> PhoenixScores {
        PhoenixScores {
            favorite_score: p.get(ActionName::ServerTweetFav),
            reply_score: p.get(ActionName::ServerTweetReply),
            retweet_score: p.get(ActionName::ServerTweetRetweet),
            photo_expand_score: p.get(ActionName::ClientTweetPhotoExpand),
            click_score: p.get(ActionName::ClientTweetClick),
            profile_click_score: p.get(ActionName::ClientTweetClickProfile),
            vqv_score: p.get(ActionName::ClientTweetVideoQualityView),
            share_score: p.get(ActionName::ClientTweetShare),
            share_via_dm_score: p.get(ActionName::ClientTweetClickSendViaDirectMessage),
            share_via_copy_link_score: p.get(ActionName::ClientTweetShareViaCopyLink),
            dwell_score: p.get(ActionName::ClientTweetRecapDwelled),
            quote_score: p.get(ActionName::ServerTweetQuote),
            quoted_click_score: p.get(ActionName::ClientQuotedTweetClick),
            follow_author_score: p.get(ActionName::ClientTweetFollowAuthor),
            not_interested_score: p.get(ActionName::ClientTweetNotInterestedIn),
            block_author_score: p.get(ActionName::ClientTweetBlockAuthor),
            mute_author_score: p.get(ActionName::ClientTweetMuteAuthor),
            report_score: p.get(ActionName::ClientTweetReport),
            dwell_time: p.get_continuous(ContinuousActionName::DwellTime),
        }
    }

    fn current_timestamp_millis() -> Option<u64> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_millis() as u64)
    }
}

struct ActionPredictions {
    /// Map of action index -> probability (exp of log prob)
    action_probs: HashMap<usize, f64>,
    /// Map of continuous action index -> value
    continuous_values: HashMap<usize, f64>,
}

impl ActionPredictions {
    fn get(&self, action: ActionName) -> Option<f64> {
        self.action_probs.get(&(action as usize)).copied()
    }

    fn get_continuous(&self, action: ContinuousActionName) -> Option<f64> {
        self.continuous_values.get(&(action as usize)).copied()
    }
}
