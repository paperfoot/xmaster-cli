use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::sync::RwLock;
use xai_kafka::{KafkaMessage, config::KafkaConsumerConfig, consumer::KafkaConsumer};

use crate::metrics;

/// Create and start a Kafka consumer with the given configuration
pub async fn create_kafka_consumer(
    config: KafkaConsumerConfig,
) -> Result<Arc<RwLock<KafkaConsumer>>> {
    let mut consumer = KafkaConsumer::new(config);
    consumer
        .start()
        .await
        .context("Failed to start Kafka consumer")?;

    Ok(Arc::new(RwLock::new(consumer)))
}

/// Process a batch of Kafka messages and deserialize them using the provided deserializer function
pub fn deserialize_kafka_messages<T, F>(
    messages: Vec<KafkaMessage>,
    deserializer: F,
) -> Result<Vec<T>>
where
    F: Fn(&[u8]) -> Result<T>,
{
    let _timer = metrics::Timer::new(metrics::BATCH_PROCESSING_TIME.clone());

    let mut kafka_data = Vec::with_capacity(messages.len());

    for msg in messages.iter() {
        if let Some(payload) = &msg.payload {
            match deserializer(payload) {
                Ok(deserialized_msg) => {
                    kafka_data.push(deserialized_msg);
                }
                Err(e) => {
                    log::error!("Failed to parse Kafka message: {}", e);
                    metrics::KAFKA_MESSAGES_FAILED_PARSE.inc();
                }
            }
        }
    }

    Ok(kafka_data)
}
