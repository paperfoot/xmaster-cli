use anyhow::{Context, Result};
use log::{error, info, warn};
use prost::Message;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::RwLock;
use xai_kafka::{KafkaMessage, config::KafkaConsumerConfig, consumer::KafkaConsumer};
use xai_kafka::{KafkaProducer, KafkaProducerConfig};
use xai_thunder_proto::{
    InNetworkEvent, LightPost, TweetCreateEvent, TweetDeleteEvent, in_network_event,
};

use crate::{
    args::Args,
    crate::config::MIN_VIDEO_DURATION_MS,
    deserializer::deserialize_tweet_event,
    kafka::utils::{create_kafka_consumer, deserialize_kafka_messages},
    metrics,
    schema::{tweet::Tweet, tweet_events::TweetEventData},
};

/// Counter for logging batch processing every Nth time
static BATCH_LOG_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Monitor Kafka partition lag and update metrics
async fn monitor_partition_lag(
    consumer: Arc<RwLock<KafkaConsumer>>,
    topic: String,
    interval_secs: u64,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));

    loop {
        interval.tick().await;

        let consumer = consumer.read().await;
        match consumer.get_partition_lags().await {
            Ok(lag_info) => {
                for partition_lag in lag_info {
                    let partition_str = partition_lag.partition_id.to_string();

                    metrics::KAFKA_PARTITION_LAG
                        .with_label_values(&[&topic, &partition_str])
                        .set(partition_lag.lag as f64);
                }
            }
            Err(e) => {
                warn!("Failed to get partition lag info: {}", e);
            }
        }
    }
}

fn is_eligible_video(tweet: &Tweet) -> bool {
    let Some(media) = tweet.media.as_ref() else {
        return false;
    };

    let [first_media] = media.as_slice() else {
        return false;
    };

    let Some(crate::schema::tweet_media::MediaInfo::VideoInfo(video_info)) =
        first_media.media_info.as_ref()
    else {
        return false;
    };

    video_info
        .duration_millis
        .map(|d| d >= MIN_VIDEO_DURATION_MS)
        .unwrap_or(false)
}

/// Start the partition lag monitoring task in the background
pub fn start_partition_lag_monitor(
    consumer: Arc<RwLock<KafkaConsumer>>,
    topic: String,
    interval_secs: u64,
) {
    tokio::spawn(async move {
        info!(
            "Starting partition lag monitoring task for topic '{}' (interval: {}s)",
            topic, interval_secs
        );
        monitor_partition_lag(consumer, topic, interval_secs).await;
    });
}

/// Start the tweet event processing loop in the background with configurable number of threads
pub async fn start_tweet_event_processing(
    base_config: KafkaConsumerConfig,
    producer_config: KafkaProducerConfig,
    args: &Args,
) {
    let num_partitions = args.tweet_events_num_partitions as usize;
    let kafka_num_threads = args.kafka_num_threads;

    // Use all available partitions
    let partitions_to_use: Vec<i32> = (0..num_partitions as i32).collect();
    let partitions_per_thread = num_partitions.div_ceil(kafka_num_threads);

    info!(
        "Starting {} message processing threads for {} partitions ({} partitions per thread)",
        kafka_num_threads, num_partitions, partitions_per_thread
    );

    let producer = if !args.is_serving {
        info!("Kafka producer enabled, starting producer...");
        let producer = Arc::new(RwLock::new(KafkaProducer::new(producer_config)));
        if let Err(e) = producer.write().await.start().await {
            panic!("Failed to start Kafka producer: {:#}", e);
        }
        Some(producer)
    } else {
        info!("Kafka producer disabled, skipping producer initialization");
        None
    };

    spawn_processing_threads(base_config, partitions_to_use, producer, args);
}

/// Spawn multiple processing threads, each handling a subset of partitions
fn spawn_processing_threads(
    base_config: KafkaConsumerConfig,
    partitions_to_use: Vec<i32>,
    producer: Option<Arc<RwLock<KafkaProducer>>>,
    args: &Args,
) {
    let total_partitions = partitions_to_use.len();
    let partitions_per_thread = total_partitions.div_ceil(args.kafka_num_threads);

    for thread_id in 0..args.kafka_num_threads {
        let start_idx = thread_id * partitions_per_thread;
        let end_idx = ((thread_id + 1) * partitions_per_thread).min(total_partitions);

        if start_idx >= total_partitions {
            break;
        }

        let thread_partitions = partitions_to_use[start_idx..end_idx].to_vec();
        let mut thread_config = base_config.clone();
        thread_config.partitions = Some(thread_partitions.clone());

        let producer_clone = producer.as_ref().map(Arc::clone);
        let topic = thread_config.base_config.topic.clone();
        let lag_monitor_interval_secs = args.lag_monitor_interval_secs;
        let batch_size = args.kafka_batch_size;
        let post_retention_sec = args.post_retention_seconds;

        tokio::spawn(async move {
            info!(
                "Starting message processing thread {} for partitions {:?}",
                thread_id, thread_partitions
            );

            match create_kafka_consumer(thread_config).await {
                Ok(consumer) => {
                    // Start partition lag monitoring for this thread's partitions
                    start_partition_lag_monitor(
                        Arc::clone(&consumer),
                        topic,
                        lag_monitor_interval_secs,
                    );

                    if let Err(e) = process_tweet_events(
                        consumer,
                        batch_size,
                        producer_clone,
                        post_retention_sec as i64,
                    )
                    .await
                    {
                        panic!(
                            "Tweet events processing thread {} exited unexpectedly: {:#}. This is a critical failure - the feeder cannot function without tweet event processing.",
                            thread_id, e
                        );
                    }
                }
                Err(e) => {
                    panic!(
                        "Failed to create consumer for thread {}: {:#}",
                        thread_id, e
                    );
                }
            }
        });
    }
}

/// Process a batch of messages: deserialize, extract posts, and store them
async fn process_message_batch(
    messages: Vec<KafkaMessage>,
    batch_num: usize,
    producer: Option<Arc<RwLock<KafkaProducer>>>,
    post_retention_sec: i64,
) -> Result<()> {
    let results = deserialize_kafka_messages(messages, deserialize_tweet_event)?;

    let mut create_tweets = Vec::new();
    let mut delete_tweets = Vec::new();
    let mut first_post_id = 0;
    let mut first_user_id = 0;

    let len_posts = results.len();

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    for tweet_event in results {
        let data = tweet_event.data.unwrap();

        match data {
            TweetEventData::TweetCreateEvent(create_event) => {
                first_post_id = create_event.tweet.as_ref().unwrap().id.unwrap();
                first_user_id = create_event.user.as_ref().unwrap().id.unwrap();

                let tweet = create_event.tweet.as_ref().unwrap();
                let core_data = tweet.core_data.as_ref().unwrap();

                if let Some(nullcast) = core_data.nullcast
                    && nullcast
                {
                    continue;
                }

                create_tweets.push(LightPost {
                    post_id: tweet.id.unwrap(),
                    author_id: create_event.user.as_ref().unwrap().id.unwrap(),
                    created_at: core_data.created_at_secs.unwrap(),
                    in_reply_to_post_id: core_data
                        .reply
                        .as_ref()
                        .and_then(|r| r.in_reply_to_status_id),
                    in_reply_to_user_id: core_data
                        .reply
                        .as_ref()
                        .and_then(|r| r.in_reply_to_user_id),
                    is_retweet: core_data.share.is_some(),
                    is_reply: core_data.reply.is_some(),
                    source_post_id: core_data.share.as_ref().and_then(|s| s.source_status_id),
                    source_user_id: core_data.share.as_ref().and_then(|s| s.source_user_id),
                    has_video: is_eligible_video(tweet),
                    conversation_id: core_data.conversation_id,
                });
            }
            TweetEventData::TweetDeleteEvent(delete_event) => {
                let created_at_secs = delete_event
                    .tweet
                    .as_ref()
                    .unwrap()
                    .core_data
                    .as_ref()
                    .unwrap()
                    .created_at_secs
                    .unwrap();
                if now_secs - created_at_secs > post_retention_sec {
                    continue;
                }
                delete_tweets.push(delete_event.tweet.as_ref().unwrap().id.unwrap());
            }
            TweetEventData::QuotedTweetDeleteEvent(delete_event) => {
                delete_tweets.push(delete_event.quoting_tweet_id.unwrap());
            }
            _ => {
                log::info!("Other non post creation/deletion event")
            }
        }
    }

    // Send each LightPost as an InNetworkEvent to the producer in separate tasks (only if producer is enabled)
    if let Some(ref producer) = producer {
        let mut send_tasks = Vec::with_capacity(create_tweets.len());
        for light_post in &create_tweets {
            let event = InNetworkEvent {
                event_variant: Some(in_network_event::EventVariant::TweetCreateEvent(
                    TweetCreateEvent {
                        post_id: light_post.post_id,
                        author_id: light_post.author_id,
                        created_at: light_post.created_at,
                        in_reply_to_post_id: light_post.in_reply_to_post_id,
                        in_reply_to_user_id: light_post.in_reply_to_user_id,
                        is_retweet: light_post.is_retweet,
                        is_reply: light_post.is_reply,
                        source_post_id: light_post.source_post_id,
                        source_user_id: light_post.source_user_id,
                        has_video: light_post.has_video,
                        conversation_id: light_post.conversation_id,
                    },
                )),
            };
            let payload = event.encode_to_vec();
            let producer_clone = Arc::clone(producer);
            send_tasks.push(tokio::spawn(async move {
                let producer_lock = producer_clone.read().await;
                if let Err(e) = producer_lock.send(&payload).await {
                    warn!("Failed to send InNetworkEvent to producer: {:#}", e);
                }
            }));
        }

        for post_id in &delete_tweets {
            let event = InNetworkEvent {
                event_variant: Some(in_network_event::EventVariant::TweetDeleteEvent(
                    TweetDeleteEvent {
                        post_id: *post_id,
                        deleted_at: now_secs,
                    },
                )),
            };
            let payload = event.encode_to_vec();
            let producer_clone = Arc::clone(producer);
            send_tasks.push(tokio::spawn(async move {
                let producer_lock = producer_clone.read().await;
                if let Err(e) = producer_lock.send(&payload).await {
                    warn!("Failed to send InNetworkEvent to producer: {:#}", e);
                }
            }));
        }

        // Wait for all send tasks to complete
        for task in send_tasks {
            if let Err(e) = task.await {
                error!("Error writing to kafka {}", e);
            }
        }
    }

    // Log every 100th batch
    let batch_count = BATCH_LOG_COUNTER.fetch_add(1, Ordering::Relaxed);
    if batch_count.is_multiple_of(1000) {
        info!(
            "Batch processing milestone: processed {} batches total, latest batch {} had {} posts (first: post_id={}, user_id={})",
            batch_count + 1,
            batch_num,
            len_posts,
            first_post_id,
            first_user_id
        );
    }

    Ok(())
}

/// Main message processing loop that polls Kafka, batches messages, and stores posts
async fn process_tweet_events(
    consumer: Arc<RwLock<KafkaConsumer>>,
    batch_size: usize,
    producer: Option<Arc<RwLock<KafkaProducer>>>,
    post_retention_sec: i64,
) -> Result<()> {
    let mut message_buffer = Vec::new();
    let mut batch_num = 0;

    loop {
        let poll_result = {
            let mut consumer_lock = consumer.write().await;
            consumer_lock.poll(100).await
        };

        match poll_result {
            Ok(messages) => {
                message_buffer.extend(messages);

                // Process batch when we have enough messages
                if message_buffer.len() >= batch_size {
                    batch_num += 1;

                    let messages = std::mem::take(&mut message_buffer);
                    let producer_clone = producer.clone();

                    // Spawn batch processing in a blocking task
                    process_message_batch(messages, batch_num, producer_clone, post_retention_sec)
                        .await
                        .context("Error processing tweet event batch")?;

                    consumer.write().await.commit_offsets()?;
                }
            }
            Err(e) => {
                warn!("Error polling messages: {:#}", e);
                metrics::KAFKA_POLL_ERRORS.inc();
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}
