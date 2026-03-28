use crate::candidate_pipeline::query_features::UserFeatures;
use crate::util::request_util::generate_request_id;
use xai_candidate_pipeline::candidate_pipeline::HasRequestId;
use xai_home_mixer_proto::ImpressionBloomFilterEntry;
use xai_twittercontext_proto::{GetTwitterContextViewer, TwitterContextViewer};

#[derive(Clone, Default, Debug)]
pub struct ScoredPostsQuery {
    pub user_id: i64,
    pub client_app_id: i32,
    pub country_code: String,
    pub language_code: String,
    pub seen_ids: Vec<i64>,
    pub served_ids: Vec<i64>,
    pub in_network_only: bool,
    pub is_bottom_request: bool,
    pub bloom_filter_entries: Vec<ImpressionBloomFilterEntry>,
    pub user_action_sequence: Option<xai_recsys_proto::UserActionSequence>,
    pub user_features: UserFeatures,
    pub request_id: String,
}

impl ScoredPostsQuery {
    pub fn new(
        user_id: i64,
        client_app_id: i32,
        country_code: String,
        language_code: String,
        seen_ids: Vec<i64>,
        served_ids: Vec<i64>,
        in_network_only: bool,
        is_bottom_request: bool,
        bloom_filter_entries: Vec<ImpressionBloomFilterEntry>,
    ) -> Self {
        let request_id = format!("{}-{}", generate_request_id(), user_id);
        Self {
            user_id,
            client_app_id,
            country_code,
            language_code,
            seen_ids,
            served_ids,
            in_network_only,
            is_bottom_request,
            bloom_filter_entries,
            user_action_sequence: None,
            user_features: UserFeatures::default(),
            request_id,
        }
    }
}

impl GetTwitterContextViewer for ScoredPostsQuery {
    fn get_viewer(&self) -> Option<TwitterContextViewer> {
        Some(TwitterContextViewer {
            user_id: self.user_id,
            client_application_id: self.client_app_id as i64,
            request_country_code: self.country_code.clone(),
            request_language_code: self.language_code.clone(),
            ..Default::default()
        })
    }
}

impl HasRequestId for ScoredPostsQuery {
    fn request_id(&self) -> &str {
        &self.request_id
    }
}
