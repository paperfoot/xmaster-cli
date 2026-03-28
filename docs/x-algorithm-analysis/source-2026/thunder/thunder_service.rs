use lazy_static::lazy_static;
use log::{debug, info, warn};
use std::cmp::Reverse;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Semaphore;
use tonic::{Request, Response, Status};

use xai_thunder_proto::{
    GetInNetworkPostsRequest, GetInNetworkPostsResponse, LightPost,
    in_network_posts_service_server::{InNetworkPostsService, InNetworkPostsServiceServer},
};

use crate::config::{
    MAX_INPUT_LIST_SIZE, MAX_POSTS_TO_RETURN, MAX_VIDEOS_TO_RETURN,
};
use crate::metrics::{
    GET_IN_NETWORK_POSTS_COUNT, GET_IN_NETWORK_POSTS_DURATION,
    GET_IN_NETWORK_POSTS_DURATION_WITHOUT_STRATO, GET_IN_NETWORK_POSTS_EXCLUDED_SIZE,
    GET_IN_NETWORK_POSTS_FOLLOWING_SIZE, GET_IN_NETWORK_POSTS_FOUND_FRESHNESS_SECONDS,
    GET_IN_NETWORK_POSTS_FOUND_POSTS_PER_AUTHOR, GET_IN_NETWORK_POSTS_FOUND_REPLY_RATIO,
    GET_IN_NETWORK_POSTS_FOUND_TIME_RANGE_SECONDS, GET_IN_NETWORK_POSTS_FOUND_UNIQUE_AUTHORS,
    GET_IN_NETWORK_POSTS_MAX_RESULTS, IN_FLIGHT_REQUESTS, REJECTED_REQUESTS, Timer,
};
use crate::posts::post_store::PostStore;
use crate::strato_client::StratoClient;

pub struct ThunderServiceImpl {
    /// PostStore for retrieving posts by user ID
    post_store: Arc<PostStore>,
    /// StratoClient for fetching following lists when not provided
    strato_client: Arc<StratoClient>,
    /// Semaphore to limit concurrent requests and prevent overload
    request_semaphore: Arc<Semaphore>,
}

impl ThunderServiceImpl {
    pub fn new(
        post_store: Arc<PostStore>,
        strato_client: Arc<StratoClient>,
        max_concurrent_requests: usize,
    ) -> Self {
        info!(
            "Initializing ThunderService with max_concurrent_requests={}",
            max_concurrent_requests
        );
        Self {
            post_store,
            strato_client,
            request_semaphore: Arc::new(Semaphore::new(max_concurrent_requests)),
        }
    }

    /// Create a gRPC server for this service
    pub fn server(self) -> InNetworkPostsServiceServer<Self> {
        InNetworkPostsServiceServer::new(self)
            .accept_compressed(tonic::codec::CompressionEncoding::Zstd)
            .send_compressed(tonic::codec::CompressionEncoding::Zstd)
    }

    /// Analyze found posts, calculate statistics, and report metrics
    /// The `stage` parameter is used as a label to differentiate between stages (e.g., "post_store", "scored")
    fn analyze_and_report_post_statistics(posts: &[LightPost], stage: &str) {
        if posts.is_empty() {
            debug!("[{}] No posts found for analysis", stage);
            return;
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // Time since most recent post
        let time_since_most_recent = posts
            .iter()
            .map(|post| post.created_at)
            .max()
            .map(|most_recent| now - most_recent);

        // Time since oldest post
        let time_since_oldest = posts
            .iter()
            .map(|post| post.created_at)
            .min()
            .map(|oldest| now - oldest);

        // Count replies vs original posts
        let reply_count = posts.iter().filter(|post| post.is_reply).count();
        let original_count = posts.len() - reply_count;

        // Unique authors
        let unique_authors: HashSet<_> = posts.iter().map(|post| post.author_id).collect();
        let unique_author_count = unique_authors.len();

        // Report metrics with stage label
        if let Some(freshness) = time_since_most_recent {
            GET_IN_NETWORK_POSTS_FOUND_FRESHNESS_SECONDS
                .with_label_values(&[stage])
                .observe(freshness as f64);
        }

        if let (Some(oldest), Some(newest)) = (time_since_oldest, time_since_most_recent) {
            let time_range = oldest - newest;
            GET_IN_NETWORK_POSTS_FOUND_TIME_RANGE_SECONDS
                .with_label_values(&[stage])
                .observe(time_range as f64);
        }

        let reply_ratio = reply_count as f64 / posts.len() as f64;
        GET_IN_NETWORK_POSTS_FOUND_REPLY_RATIO
            .with_label_values(&[stage])
            .observe(reply_ratio);

        GET_IN_NETWORK_POSTS_FOUND_UNIQUE_AUTHORS
            .with_label_values(&[stage])
            .observe(unique_author_count as f64);

        if unique_author_count > 0 {
            let posts_per_author = posts.len() as f64 / unique_author_count as f64;
            GET_IN_NETWORK_POSTS_FOUND_POSTS_PER_AUTHOR
                .with_label_values(&[stage])
                .observe(posts_per_author);
        }

        // Log statistics with stage label
        debug!(
            "[{}] Post statistics: total={}, original={}, replies={}, unique_authors={}, posts_per_author={:.2}, reply_ratio={:.2}, time_since_most_recent={:?}s, time_range={:?}s",
            stage,
            posts.len(),
            original_count,
            reply_count,
            unique_author_count,
            if unique_author_count > 0 {
                posts.len() as f64 / unique_author_count as f64
            } else {
                0.0
            },
            reply_ratio,
            time_since_most_recent,
            if let (Some(o), Some(n)) = (time_since_oldest, time_since_most_recent) {
                Some(o - n)
            } else {
                None
            }
        );
    }
}

#[tonic::async_trait]
impl InNetworkPostsService for ThunderServiceImpl {
    /// Get posts from users in the network
    async fn get_in_network_posts(
        &self,
        request: Request<GetInNetworkPostsRequest>,
    ) -> Result<Response<GetInNetworkPostsResponse>, Status> {
        // Try to acquire semaphore permit without blocking
        // If we're at capacity, reject immediately with RESOURCE_EXHAUSTED
        let _permit = match self.request_semaphore.try_acquire() {
            Ok(permit) => {
                IN_FLIGHT_REQUESTS.inc();
                permit
            }
            Err(_) => {
                REJECTED_REQUESTS.inc();
                return Err(Status::resource_exhausted(
                    "Server at capacity, please retry",
                ));
            }
        };

        // Use a guard to decrement in_flight_requests when the request completes
        struct InFlightGuard;
        impl Drop for InFlightGuard {
            fn drop(&mut self) {
                IN_FLIGHT_REQUESTS.dec();
            }
        }
        let _in_flight_guard = InFlightGuard;

        // Start timer for total latency
        let _total_timer = Timer::new(GET_IN_NETWORK_POSTS_DURATION.clone());

        let req = request.into_inner();

        if req.debug {
            info!(
                "Received GetInNetworkPosts request: user_id={}, following_count={}, exclude_tweet_ids={}",
                req.user_id,
                req.following_user_ids.len(),
                req.exclude_tweet_ids.len(),
            );
        }

        // If following_user_id list is empty, fetch it from Strato
        let following_user_ids = if req.following_user_ids.is_empty() && req.debug {
            info!(
                "Following list is empty, fetching from Strato for user {}",
                req.user_id
            );

            match self
                .strato_client
                .fetch_following_list(req.user_id as i64, MAX_INPUT_LIST_SIZE as i32)
                .await
            {
                Ok(following_list) => {
                    info!(
                        "Fetched {} following users from Strato for user {}",
                        following_list.len(),
                        req.user_id
                    );
                    following_list.into_iter().map(|id| id as u64).collect()
                }
                Err(e) => {
                    warn!(
                        "Failed to fetch following list from Strato for user {}: {}",
                        req.user_id, e
                    );
                    return Err(Status::internal(format!(
                        "Failed to fetch following list: {}",
                        e
                    )));
                }
            }
        } else {
            req.following_user_ids
        };

        // Record metrics for request parameters
        GET_IN_NETWORK_POSTS_FOLLOWING_SIZE.observe(following_user_ids.len() as f64);
        GET_IN_NETWORK_POSTS_EXCLUDED_SIZE.observe(req.exclude_tweet_ids.len() as f64);

        // Start timer for latency without strato call
        let _processing_timer = Timer::new(GET_IN_NETWORK_POSTS_DURATION_WITHOUT_STRATO.clone());

        // Default max_results if not specified
        let max_results = if req.max_results > 0 {
            req.max_results as usize
        } else if req.is_video_request {
            MAX_VIDEOS_TO_RETURN
        } else {
            MAX_POSTS_TO_RETURN
        };
        GET_IN_NETWORK_POSTS_MAX_RESULTS.observe(max_results as f64);

        // Limit following_user_ids and exclude_tweet_ids to first K entries
        let following_count = following_user_ids.len();
        if following_count > MAX_INPUT_LIST_SIZE {
            warn!(
                "Limiting following_user_ids from {} to {} entries for user {}",
                following_count, MAX_INPUT_LIST_SIZE, req.user_id
            );
        }
        let following_user_ids: Vec<u64> = following_user_ids
            .into_iter()
            .take(MAX_INPUT_LIST_SIZE)
            .collect();

        let exclude_count = req.exclude_tweet_ids.len();
        if exclude_count > MAX_INPUT_LIST_SIZE {
            warn!(
                "Limiting exclude_tweet_ids from {} to {} entries for user {}",
                exclude_count, MAX_INPUT_LIST_SIZE, req.user_id
            );
        }
        let exclude_tweet_ids: Vec<u64> = req
            .exclude_tweet_ids
            .into_iter()
            .take(MAX_INPUT_LIST_SIZE)
            .collect();

        // Clone Arc references needed inside spawn_blocking
        let post_store = Arc::clone(&self.post_store);
        let request_user_id = req.user_id as i64;

        // Use spawn_blocking to avoid blocking tokio's async runtime
        let proto_posts = tokio::task::spawn_blocking(move || {
            // Create exclude tweet IDs set for efficient filtering of previously seen posts
            let exclude_tweet_ids: HashSet<i64> =
                exclude_tweet_ids.iter().map(|&id| id as i64).collect();

            let start_time = Instant::now();

            // Fetch all posts (original + secondary) for the followed users
            let all_posts: Vec<LightPost> = if req.is_video_request {
                post_store.get_videos_by_users(
                    &following_user_ids,
                    &exclude_tweet_ids,
                    start_time,
                    request_user_id,
                )
            } else {
                post_store.get_all_posts_by_users(
                    &following_user_ids,
                    &exclude_tweet_ids,
                    start_time,
                    request_user_id,
                )
            };

            // Analyze posts and report statistics after querying post_store
            ThunderServiceImpl::analyze_and_report_post_statistics(&all_posts, "retrieved");

            let scored_posts = score_recent(all_posts, max_results);

            // Analyze posts and report statistics after scoring
            ThunderServiceImpl::analyze_and_report_post_statistics(&scored_posts, "scored");

            scored_posts
        })
        .await
        .map_err(|e| Status::internal(format!("Failed to process posts: {}", e)))?;

        if req.debug {
            info!(
                "Returning {} posts for user {}",
                proto_posts.len(),
                req.user_id
            );
        }

        // Record the number of posts returned
        GET_IN_NETWORK_POSTS_COUNT.observe(proto_posts.len() as f64);

        let response = GetInNetworkPostsResponse { posts: proto_posts };

        Ok(Response::new(response))
    }
}

/// Score posts by recency (created_at timestamp, newer posts first)
fn score_recent(mut light_posts: Vec<LightPost>, max_results: usize) -> Vec<LightPost> {
    light_posts.sort_unstable_by_key(|post| Reverse(post.created_at));

    // Limit to max results
    light_posts.into_iter().take(max_results).collect()
}
