use anyhow::{Context, Result};
use std::sync::Arc;
use xai_kafka::KafkaProducerConfig;
use xai_kafka::config::{KafkaConfig, KafkaConsumerConfig, SslConfig};
use xai_wily::WilyConfig;

use crate::{
    args,
    kafka::{
        tweet_events_listener::start_tweet_event_processing,
        tweet_events_listener_v2::start_tweet_event_processing_v2,
    },
};

const TWEET_EVENT_TOPIC: &str = "";
const TWEET_EVENT_DEST: &str = "";

const IN_NETWORK_EVENTS_DEST: &str = "";
const IN_NETWORK_EVENTS_TOPIC: &str = "";

pub async fn start_kafka(
    args: &args::Args,
    post_store: Arc<crate::posts::post_store::PostStore>,
    user: &str,
    tx: tokio::sync::mpsc::Sender<i64>,
) -> Result<()> {
    let sasl_password = std::env::var("")
        .ok()
        .or(args.sasl_password.clone())?;

    let producer_sasl_password = std::env::var("")
        .ok()
        .or(args.producer_sasl_password.clone());

    if args.is_serving {
        let unique_id = uuid::Uuid::new_v4().to_string();

        let v2_tweet_events_consumer_config = KafkaConsumerConfig {
            base_config: KafkaConfig {
                dest: args.in_network_events_consumer_dest.clone(),
                topic: IN_NETWORK_EVENTS_TOPIC.to_string(),
                wily_config: Some(WilyConfig::default()),
                ssl: Some(SslConfig {
                    security_protocol: args.security_protocol.clone(),
                    sasl_mechanism: Some(args.producer_sasl_mechanism.clone()),
                    sasl_username: Some(args.producer_sasl_username.clone()),
                    sasl_password: producer_sasl_password.clone(),
                }),
                ..Default::default()
            },
            group_id: format!("{}-{}", args.kafka_group_id, unique_id),
            auto_offset_reset: args.auto_offset_reset.clone(),
            fetch_timeout_ms: args.fetch_timeout_ms,
            max_partition_fetch_bytes: Some(1024 * 1024 * 100),
            skip_to_latest: args.skip_to_latest,
            ..Default::default()
        };

        // Start Kafka background tasks
        start_tweet_event_processing_v2(
            v2_tweet_events_consumer_config,
            Arc::clone(&post_store),
            args,
            tx,
        )
        .await;
    }

    // Only start Kafka processing and background tasks if not in serving mode
    if !args.is_serving {
        // Create Kafka consumer config
        let tweet_events_consumer_config = KafkaConsumerConfig {
            base_config: KafkaConfig {
                dest: TWEET_EVENT_DEST.to_string(),
                topic: TWEET_EVENT_TOPIC.to_string(),
                wily_config: Some(WilyConfig::default()),
                ssl: Some(SslConfig {
                    security_protocol: args.security_protocol.clone(),
                    sasl_mechanism: Some(args.sasl_mechanism.clone()),
                    sasl_username: Some(args.sasl_username.clone()),
                    sasl_password: Some(sasl_password.clone()),
                }),
                ..Default::default()
            },
            group_id: format!("{}-{}", args.kafka_group_id, user),
            auto_offset_reset: args.auto_offset_reset.clone(),
            enable_auto_commit: false,
            fetch_timeout_ms: args.fetch_timeout_ms,
            max_partition_fetch_bytes: Some(1024 * 1024 * 10),
            partitions: None,
            skip_to_latest: args.skip_to_latest,
            ..Default::default()
        };

        let producer_config = KafkaProducerConfig {
            base_config: KafkaConfig {
                dest: IN_NETWORK_EVENTS_DEST.to_string(),
                topic: IN_NETWORK_EVENTS_TOPIC.to_string(),
                wily_config: Some(WilyConfig::default()),
                ssl: Some(SslConfig {
                    security_protocol: args.security_protocol.clone(),
                    sasl_mechanism: Some(args.producer_sasl_mechanism.clone()),
                    sasl_username: Some(args.producer_sasl_username.clone()),
                    sasl_password: producer_sasl_password.clone(),
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        start_tweet_event_processing(tweet_events_consumer_config, producer_config, args).await;
    }

    Ok(())
}
