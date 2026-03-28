use anyhow::Result;
use log::{info, warn};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, Semaphore};
use xai_kafka::{KafkaMessage, config::KafkaConsumerConfig, consumer::KafkaConsumer};

use xai_thunder_proto::{LightPost, TweetDeleteEvent, in_network_event};

use crate::{
    args::Args,
    deserializer::deserialize_tweet_event_v2,
    kafka::utils::{create_kafka_consumer, deserialize_kafka_messages},
    metrics,
    posts::post_store::PostStore,
};

/// Counter for logging deserialization every Nth time
static DESER_LOG_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Start the tweet event processing loop in the background with configurable number of threads
pub async fn start_tweet_event_processing_v2(
    base_config: KafkaConsumerConfig,
    post_store: Arc<PostStore>,
    args: &Args,
    tx: tokio::sync::mpsc::Sender<i64>,
) {
    let num_partitions = args.kafka_tweet_events_v2_num_partitions;
    let kafka_num_threads = args.kafka_num_threads;

    // Use all available partitions
    let partitions_to_use: Vec<i32> = (0..num_partitions as i32).collect();
    let partitions_per_thread = num_partitions.div_ceil(kafka_num_threads);

    info!(
        "Starting {} message processing threads for {} partitions ({} partitions per thread)",
        kafka_num_threads, num_partitions, partitions_per_thread
    );

    spawn_processing_threads_v2(base_config, partitions_to_use, post_store, args, tx);
}

/// Spawn multiple processing threads, each handling a subset of partitions
fn spawn_processing_threads_v2(
    base_config: KafkaConsumerConfig,
    partitions_to_use: Vec<i32>,
    post_store: Arc<PostStore>,
    args: &Args,
    tx: tokio::sync::mpsc::Sender<i64>,
) {
    let total_partitions = partitions_to_use.len();
    let partitions_per_thread = total_partitions.div_ceil(args.kafka_num_threads);

    // Create shared semaphore to prevent too many tweet_events partition updates at the same time
    let semaphore = Arc::new(Semaphore::new(3));

    for thread_id in 0..args.kafka_num_threads {
        let start_idx = thread_id * partitions_per_thread;
        let end_idx = ((thread_id + 1) * partitions_per_thread).min(total_partitions);

        if start_idx >= total_partitions {
            break;
        }

        let thread_partitions = partitions_to_use[start_idx..end_idx].to_vec();
        let mut thread_config = base_config.clone();
        thread_config.partitions = Some(thread_partitions.clone());

        let post_store_clone = Arc::clone(&post_store);
        let topic = thread_config.base_config.topic.clone();
        let lag_monitor_interval_secs = args.lag_monitor_interval_secs;
        let batch_size = args.kafka_batch_size;
        let tx_clone = tx.clone();
        let semaphore_clone = Arc::clone(&semaphore);

        tokio::spawn(async move {
            info!(
                "Starting message processing thread {} for partitions {:?}",
                thread_id, thread_partitions
            );

            match create_kafka_consumer(thread_config).await {
                Ok(consumer) => {
                    // Start partition lag monitoring for this thread's partitions
                    crate::kafka::tweet_events_listener::start_partition_lag_monitor(
                        Arc::clone(&consumer),
                        topic,
                        lag_monitor_interval_secs,
                    );

                    if let Err(e) = process_tweet_events_v2(
                        consumer,
                        post_store_clone,
                        batch_size,
                        tx_clone,
                        semaphore_clone,
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

/// Process a single batch of messages: deserialize, extract posts, and store them
fn deserialize_batch(
    messages: Vec<KafkaMessage>,
) -> Result<(Vec<LightPost>, Vec<TweetDeleteEvent>)> {
    let start_time = Instant::now();
    let num_messages = messages.len();
    let results = deserialize_kafka_messages(messages, deserialize_tweet_event_v2)?;
    let deser_elapsed = start_time.elapsed();
    if DESER_LOG_COUNTER
        .fetch_add(1, Ordering::Relaxed)
        .is_multiple_of(1000)
    {
        info!(
            "Deserialized {} messages in {:?} ({:.2} msgs/sec)",
            num_messages,
            deser_elapsed,
            num_messages as f64 / deser_elapsed.as_secs_f64()
        );
    }

    let mut create_tweets = Vec::with_capacity(results.len());
    let mut delete_tweets = Vec::with_capacity(10);

    for tweet_event in results {
        match tweet_event.event_variant.unwrap() {
            in_network_event::EventVariant::TweetCreateEvent(create_event) => {
                create_tweets.push(LightPost {
                    post_id: create_event.post_id,
                    author_id: create_event.author_id,
                    created_at: create_event.created_at,
                    in_reply_to_post_id: create_event.in_reply_to_post_id,
                    in_reply_to_user_id: create_event.in_reply_to_user_id,
                    is_retweet: create_event.is_retweet,
                    is_reply: create_event.is_reply
                        || create_event.in_reply_to_post_id.is_some()
                        || create_event.in_reply_to_user_id.is_some(),
                    source_post_id: create_event.source_post_id,
                    source_user_id: create_event.source_user_id,
                    has_video: create_event.has_video,
                    conversation_id: create_event.conversation_id,
                });
            }
            in_network_event::EventVariant::TweetDeleteEvent(delete_event) => {
                delete_tweets.push(delete_event);
            }
        }
    }

    Ok((create_tweets, delete_tweets))
}

/// Main message processing loop that polls Kafka, batches messages, and stores posts
async fn process_tweet_events_v2(
    consumer: Arc<RwLock<KafkaConsumer>>,
    post_store: Arc<PostStore>,
    batch_size: usize,
    tx: tokio::sync::mpsc::Sender<i64>,
    semaphore: Arc<Semaphore>,
) -> Result<()> {
    let mut message_buffer = Vec::new();
    let mut batch_count = 0_usize;
    let mut init_data_downloaded = false;

    loop {
        let poll_result = {
            let mut consumer_lock = consumer.write().await;
            consumer_lock.poll(batch_size).await
        };

        match poll_result {
            Ok(messages) => {
                let catchup_sender = if !init_data_downloaded {
                    let consumer_lock = consumer.read().await;
                    if let Ok(lags) = consumer_lock.get_partition_lags().await {
                        let total_lag: i64 = lags.iter().map(|l| l.lag).sum();
                        if total_lag < (lags.len() * batch_size) as i64 {
                            init_data_downloaded = true;
                            Some((tx.clone(), total_lag))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };

                message_buffer.extend(messages);

                // Process batch when we have enough messages
                if message_buffer.len() >= batch_size {
                    batch_count += 1;
                    let messages = std::mem::take(&mut message_buffer);
                    let post_store_clone = Arc::clone(&post_store);

                    // Acquire semaphore permit if init data is downloaded to allow enough CPU for serving requests
                    let permit = if init_data_downloaded {
                        Some(semaphore.clone().acquire_owned().await.unwrap())
                    } else {
                        None
                    };

                    // Send batch to blocking thread pool for processing
                    let _ = tokio::task::spawn_blocking(move || {
                        let _permit = permit; // Hold permit until task completes
                        match deserialize_batch(messages) {
                            Err(e) => warn!("Error processing batch {}: {:#}", batch_count, e),
                            Ok((light_posts, delete_posts)) => {
                                post_store_clone.insert_posts(light_posts);
                                post_store_clone.mark_as_deleted(delete_posts);
                            }
                        };
                    })
                    .await;

                    if let Some((sender, lag)) = catchup_sender {
                        info!("Completed kafka init for a single thread");
                        if let Err(e) = sender.send(lag).await {
                            log::error!("error sending {}", e);
                        }
                    }
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
