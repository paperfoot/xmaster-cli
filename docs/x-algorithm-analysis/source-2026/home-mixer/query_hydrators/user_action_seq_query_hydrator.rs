use crate::candidate_pipeline::query::ScoredPostsQuery;
use crate::clients::uas_fetcher::{UserActionSequenceFetcher, UserActionSequenceOps};
use crate::params as p;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tonic::async_trait;
use xai_candidate_pipeline::query_hydrator::QueryHydrator;
use xai_recsys_aggregation::aggregation::{DefaultAggregator, UserActionAggregator};
use xai_recsys_aggregation::filters::{
    AggregatedActionFilter, DenseAggregatedActionFilter, KeepOriginalUserActionFilter,
    UserActionFilter,
};
use xai_recsys_proto::{
    AggregatedUserActionList, Mask, MaskType, UserActionSequence, UserActionSequenceDataContainer,
    UserActionSequenceMeta, user_action_sequence_data_container::Data as ProtoDataContainer,
};
use xai_uas_thrift::convert::thrift_to_proto_aggregated_user_action;
use xai_uas_thrift::user_action_sequence::{
    AggregatedUserAction as ThriftAggregatedUserAction,
    UserActionSequence as ThriftUserActionSequence,
    UserActionSequenceMeta as ThriftUserActionSequenceMeta,
};

/// Hydrate a sequence that captures the user's recent actions
pub struct UserActionSeqQueryHydrator {
    pub uas_fetcher: Arc<UserActionSequenceFetcher>,
    global_filter: Arc<dyn UserActionFilter>,
    aggregator: Arc<dyn UserActionAggregator>,
    post_filters: Vec<Arc<dyn AggregatedActionFilter>>,
}

impl UserActionSeqQueryHydrator {
    pub fn new(uas_fetcher: Arc<UserActionSequenceFetcher>) -> Self {
        Self {
            uas_fetcher,
            global_filter: Arc::new(KeepOriginalUserActionFilter::new()),
            aggregator: Arc::new(DefaultAggregator),
            post_filters: vec![Arc::new(DenseAggregatedActionFilter::new())],
        }
    }
}

#[async_trait]
impl QueryHydrator<ScoredPostsQuery> for UserActionSeqQueryHydrator {
    #[xai_stats_macro::receive_stats]
    async fn hydrate(&self, query: &ScoredPostsQuery) -> Result<ScoredPostsQuery, String> {
        let uas_thrift = self
            .uas_fetcher
            .get_by_user_id(query.user_id)
            .await
            .map_err(|e| format!("Failed to fetch user action sequence: {}", e))?;

        let aggregated_uas_proto =
            self.aggregate_user_action_sequence(query.user_id, uas_thrift)?;

        Ok(ScoredPostsQuery {
            user_action_sequence: Some(aggregated_uas_proto),
            ..Default::default()
        })
    }

    fn update(&self, query: &mut ScoredPostsQuery, hydrated: ScoredPostsQuery) {
        query.user_action_sequence = hydrated.user_action_sequence;
    }

    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}

impl UserActionSeqQueryHydrator {
    fn aggregate_user_action_sequence(
        &self,
        user_id: i64,
        uas_thrift: ThriftUserActionSequence,
    ) -> Result<UserActionSequence, String> {
        // Extract user_actions from thrift sequence
        let thrift_user_actions = uas_thrift.user_actions.clone().unwrap_or_default();
        if thrift_user_actions.is_empty() {
            return Err(format!("No user actions found for user {}", user_id));
        }

        // Pre-aggregation filter
        let filtered_actions = self.global_filter.run(thrift_user_actions);
        if filtered_actions.is_empty() {
            return Err(format!(
                "No user actions remaining after filtering for user {}",
                user_id
            ));
        }

        // Aggregate
        let mut aggregated_actions =
            self.aggregator
                .run(&filtered_actions, p::UAS_WINDOW_TIME_MS, 0);

        // Post-aggregation filters
        for filter in &self.post_filters {
            aggregated_actions = filter.run(aggregated_actions);
        }

        // Truncate to max sequence length (keep last N items)
        if aggregated_actions.len() > p::UAS_MAX_SEQUENCE_LENGTH {
            let drain_count = aggregated_actions.len() - p::UAS_MAX_SEQUENCE_LENGTH;
            aggregated_actions.drain(0..drain_count);
        }

        // Convert to proto format
        let original_metadata = uas_thrift.metadata.clone().unwrap_or_default();
        convert_to_proto_sequence(
            user_id,
            original_metadata,
            aggregated_actions,
            self.aggregator.name(),
        )
    }
}

fn convert_to_proto_sequence(
    user_id: i64,
    original_metadata: ThriftUserActionSequenceMeta,
    aggregated_actions: Vec<ThriftAggregatedUserAction>,
    aggregator_name: &str,
) -> Result<UserActionSequence, String> {
    if aggregated_actions.is_empty() {
        return Err("Cannot create sequence from empty aggregated actions".to_string());
    }

    let first_sequence_time = aggregated_actions
        .first()
        .and_then(|a| a.impressed_time_ms)
        .unwrap_or(0) as u64;
    let last_sequence_time = aggregated_actions
        .last()
        .and_then(|a| a.impressed_time_ms)
        .unwrap_or(0) as u64;

    // Preserve lastModifiedEpochMs and lastKafkaPublishEpochMs from original metadata
    let last_modified_epoch_ms = original_metadata.last_modified_epoch_ms.unwrap_or(0) as u64;
    let previous_kafka_publish_epoch_ms =
        original_metadata.last_kafka_publish_epoch_ms.unwrap_or(0) as u64;

    let proto_metadata = UserActionSequenceMeta {
        length: aggregated_actions.len() as u64,
        first_sequence_time,
        last_sequence_time,
        last_modified_epoch_ms,
        previous_kafka_publish_epoch_ms,
    };

    // Convert thrift aggregated actions to proto
    let mut proto_agg_actions = Vec::with_capacity(aggregated_actions.len());
    for action in aggregated_actions {
        proto_agg_actions.push(
            thrift_to_proto_aggregated_user_action(action)
                .map_err(|e| format!("Failed to convert aggregated action: {}", e))?,
        );
    }

    let aggregation_time_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    let agg_list = AggregatedUserActionList {
        aggregated_user_actions: proto_agg_actions,
        aggregation_provider: aggregator_name.to_string(),
        aggregation_time_ms,
    };

    let mask = Mask {
        mask_type: MaskType::NewEvent as i32,
        mask: vec![false; agg_list.aggregated_user_actions.len()],
    };

    // Build the final UserActionSequence
    Ok(UserActionSequence {
        user_id: user_id as u64,
        metadata: Some(proto_metadata),
        user_actions_data: Some(UserActionSequenceDataContainer {
            data: Some(ProtoDataContainer::OrderedAggregatedUserActionsList(
                agg_list,
            )),
        }),
        masks: vec![mask],
        ..Default::default()
    })
}
