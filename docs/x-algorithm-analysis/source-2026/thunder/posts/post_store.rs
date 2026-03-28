use anyhow::Result;
use dashmap::DashMap;
use log::info;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use xai_thunder_proto::{LightPost, TweetDeleteEvent};

use crate::config::{
    DELETE_EVENT_KEY, MAX_ORIGINAL_POSTS_PER_AUTHOR, MAX_REPLY_POSTS_PER_AUTHOR,
    MAX_TINY_POSTS_PER_USER_SCAN, MAX_VIDEO_POSTS_PER_AUTHOR,
};
use crate::metrics::{
    POST_STORE_DELETED_POSTS, POST_STORE_DELETED_POSTS_FILTERED, POST_STORE_ENTITY_COUNT,
    POST_STORE_POSTS_RETURNED, POST_STORE_POSTS_RETURNED_RATIO, POST_STORE_REQUEST_TIMEOUTS,
    POST_STORE_REQUESTS, POST_STORE_TOTAL_POSTS, POST_STORE_USER_COUNT,
};

/// Minimal post reference stored in user timelines (only ID and timestamp)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TinyPost {
    pub post_id: i64,
    pub created_at: i64,
}

impl TinyPost {
    /// Create a new TinyPost from a post ID and creation timestamp
    pub fn new(post_id: i64, created_at: i64) -> Self {
        TinyPost {
            post_id,
            created_at,
        }
    }
}

/// A thread-safe store for posts grouped by user ID
/// Note: LightPost is now defined in the protobuf schema (in-network.proto)
#[derive(Clone)]
pub struct PostStore {
    /// Full post data indexed by post_id
    posts: Arc<DashMap<i64, LightPost>>,
    /// Maps user_id to a deque of TinyPost references for original posts (non-reply, non-retweet)
    original_posts_by_user: Arc<DashMap<i64, VecDeque<TinyPost>>>,
    /// Maps user_id to a deque of TinyPost references for replies and retweets
    secondary_posts_by_user: Arc<DashMap<i64, VecDeque<TinyPost>>>,
    /// Maps user_id to a deque of TinyPost references for video posts
    video_posts_by_user: Arc<DashMap<i64, VecDeque<TinyPost>>>,
    deleted_posts: Arc<DashMap<i64, bool>>,
    /// Retention period for posts in seconds
    retention_seconds: u64,
    /// Request timeout for get_posts_by_users iteration (0 = no timeout)
    request_timeout: Duration,
}

impl PostStore {
    /// Creates a new empty PostStore with the specified retention period and request timeout
    pub fn new(retention_seconds: u64, request_timeout_ms: u64) -> Self {
        PostStore {
            posts: Arc::new(DashMap::new()),
            original_posts_by_user: Arc::new(DashMap::new()),
            secondary_posts_by_user: Arc::new(DashMap::new()),
            video_posts_by_user: Arc::new(DashMap::new()),
            deleted_posts: Arc::new(DashMap::new()),
            retention_seconds,
            request_timeout: Duration::from_millis(request_timeout_ms),
        }
    }

    pub fn mark_as_deleted(&self, posts: Vec<TweetDeleteEvent>) {
        for post in posts.into_iter() {
            self.posts.remove(&post.post_id);
            self.deleted_posts.insert(post.post_id, true);

            let mut user_posts_entry = self
                .original_posts_by_user
                .entry(DELETE_EVENT_KEY)
                .or_default();
            user_posts_entry.push_back(TinyPost {
                post_id: post.post_id,
                created_at: post.deleted_at,
            });
        }
    }

    /// Inserts posts into the post store
    pub fn insert_posts(&self, mut posts: Vec<LightPost>) {
        // Filter to keep only posts created in the last retention_seconds and not from the future
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        posts.retain(|p| {
            p.created_at < current_time
                && current_time - p.created_at <= (self.retention_seconds as i64)
        });

        // Sort remaining posts by created_at timestamp
        posts.sort_unstable_by_key(|p| p.created_at);

        Self::insert_posts_internal(self, posts);
    }

    pub async fn finalize_init(&self) -> Result<()> {
        self.sort_all_user_posts().await;
        self.trim_old_posts().await;

        // This is needed because order of create_event/delete_event can be be lost in the feeder
        for entry in self.deleted_posts.iter() {
            self.posts.remove(entry.key());
        }

        Ok(())
    }

    fn insert_posts_internal(&self, posts: Vec<LightPost>) {
        for post in posts {
            let post_id = post.post_id;
            let author_id = post.author_id;
            let created_at = post.created_at;
            let is_original = !post.is_reply && !post.is_retweet;

            if self.deleted_posts.contains_key(&post_id) {
                continue;
            }

            // Store the full post data
            let old = self.posts.insert(post_id, post);
            if old.is_some() {
                // if already stored - don't add it again
                continue;
            }

            // Create a TinyPost reference for the timeline
            let tiny_post = TinyPost::new(post_id, created_at);

            // Use entry API to get mutable access to the appropriate user's posts timeline
            if is_original {
                let mut user_posts_entry =
                    self.original_posts_by_user.entry(author_id).or_default();
                user_posts_entry.push_back(tiny_post.clone());
            } else {
                let mut user_posts_entry =
                    self.secondary_posts_by_user.entry(author_id).or_default();
                user_posts_entry.push_back(tiny_post.clone());
            }

            let mut video_eligible = post.has_video;

            // If this is a retweet and the retweeted post has video, mark has_video as true
            if !video_eligible
                && post.is_retweet
                && let Some(source_post_id) = post.source_post_id
                && let Some(source_post) = self.posts.get(&source_post_id)
            {
                video_eligible = !source_post.is_reply && source_post.has_video;
            }

            if post.is_reply {
                video_eligible = false;
            }

            // Also add to video posts timeline if post has video
            if video_eligible {
                let mut user_posts_entry = self.video_posts_by_user.entry(author_id).or_default();
                user_posts_entry.push_back(tiny_post);
            }
        }
    }

    /// Retrieves video posts from multiple users
    pub fn get_videos_by_users(
        &self,
        user_ids: &[i64],
        exclude_tweet_ids: &HashSet<i64>,
        start_time: Instant,
        request_user_id: i64,
    ) -> Vec<LightPost> {
        let video_posts = self.get_posts_from_map(
            &self.video_posts_by_user,
            user_ids,
            MAX_VIDEO_POSTS_PER_AUTHOR,
            exclude_tweet_ids,
            &HashSet::new(),
            start_time,
            request_user_id,
        );

        POST_STORE_POSTS_RETURNED.observe(video_posts.len() as f64);
        video_posts
    }

    /// Retrieves all posts from multiple users
    pub fn get_all_posts_by_users(
        &self,
        user_ids: &[i64],
        exclude_tweet_ids: &HashSet<i64>,
        start_time: Instant,
        request_user_id: i64,
    ) -> Vec<LightPost> {
        let following_users_set: HashSet<i64> = user_ids.iter().copied().collect();

        let mut all_posts = self.get_posts_from_map(
            &self.original_posts_by_user,
            user_ids,
            MAX_ORIGINAL_POSTS_PER_AUTHOR,
            exclude_tweet_ids,
            &HashSet::new(),
            start_time,
            request_user_id,
        );

        let secondary_posts = self.get_posts_from_map(
            &self.secondary_posts_by_user,
            user_ids,
            MAX_REPLY_POSTS_PER_AUTHOR,
            exclude_tweet_ids,
            &following_users_set,
            start_time,
            request_user_id,
        );

        all_posts.extend(secondary_posts);
        POST_STORE_POSTS_RETURNED.observe(all_posts.len() as f64);
        all_posts
    }

    #[allow(clippy::too_many_arguments)]
    pub fn get_posts_from_map(
        &self,
        posts_map: &Arc<DashMap<i64, VecDeque<TinyPost>>>,
        user_ids: &[i64],
        max_per_user: usize,
        exclude_tweet_ids: &HashSet<i64>,
        following_users: &HashSet<i64>,
        start_time: Instant,
        request_user_id: i64,
    ) -> Vec<LightPost> {
        POST_STORE_REQUESTS.inc();
        let mut light_posts = Vec::new();

        let mut total_eligible: usize = 0;

        for (i, user_id) in user_ids.iter().enumerate() {
            if !self.request_timeout.is_zero() && start_time.elapsed() >= self.request_timeout {
                log::error!(
                    "Timed out fetching posts for user={}; Processed: {}/{}. Stage: {}",
                    request_user_id,
                    i,
                    user_ids.len(),
                    if following_users.is_empty() {
                        "original"
                    } else {
                        "secondary"
                    }
                );
                POST_STORE_REQUEST_TIMEOUTS.inc();
                break;
            }

            if let Some(user_posts_ref) = posts_map.get(user_id) {
                let user_posts = user_posts_ref.value();
                total_eligible += user_posts.len();

                // Start from newest posts (reverse iterator)
                // Take a capped number to prevent from going all the way back to when user is inactive
                let tiny_posts_iter = user_posts
                    .iter()
                    .rev()
                    .filter(|post| !exclude_tweet_ids.contains(&post.post_id))
                    .take(MAX_TINY_POSTS_PER_USER_SCAN);

                // Perform light doc lookup to get full LightPost data. This will also filter deleted posts
                // Note: We copy the value immediately to release the read lock and avoid potential
                // deadlock when acquiring nested read locks while a writer is waiting.
                let light_post_iter_1 = tiny_posts_iter
                    .filter_map(|tiny_post| self.posts.get(&tiny_post.post_id).map(|r| *r.value()));

                let light_post_iter = light_post_iter_1.filter(|post| {
                    if self.deleted_posts.get(&post.post_id).is_some() {
                        POST_STORE_DELETED_POSTS_FILTERED.inc();
                        false
                    } else {
                        true
                    }
                });

                let light_post_iter = light_post_iter.filter(|post| {
                    !(post.is_retweet && post.source_user_id == Some(request_user_id))
                });

                let filtered_post_iter = light_post_iter.filter(|post| {
                    if following_users.is_empty() {
                        return true;
                    }
                    post.in_reply_to_post_id.is_none_or(|reply_to_post_id| {
                        if let Some(replied_to_post) = self.posts.get(&reply_to_post_id) {
                            if !replied_to_post.is_retweet && !replied_to_post.is_reply {
                                return true;
                            }

                            return post.conversation_id.is_some_and(|convo_id| {
                                let reply_to_reply_to_original =
                                    replied_to_post.in_reply_to_post_id == Some(convo_id);
                                let reply_to_followed_user = post
                                    .in_reply_to_user_id
                                    .map(|uid| following_users.contains(&uid))
                                    .unwrap_or(false);

                                reply_to_reply_to_original && reply_to_followed_user
                            });
                        }

                        false
                    })
                });

                light_posts.extend(filtered_post_iter.take(max_per_user));
            }
        }

        // Track ratio of returned posts to eligible posts
        if total_eligible > 0 {
            let ratio = light_posts.len() as f64 / total_eligible as f64;
            POST_STORE_POSTS_RETURNED_RATIO.observe(ratio);
        }

        light_posts
    }

    /// Start a background task that periodically logs PostStore statistics
    pub fn start_stats_logger(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));

            loop {
                interval.tick().await;

                let user_count = self.original_posts_by_user.len();
                let total_posts = self.posts.len();
                let deleted_posts = self.deleted_posts.len();

                // Sum up all VecDeque sizes for each map
                let original_posts_count: usize = self
                    .original_posts_by_user
                    .iter()
                    .map(|entry| entry.value().len())
                    .sum();
                let secondary_posts_count: usize = self
                    .secondary_posts_by_user
                    .iter()
                    .map(|entry| entry.value().len())
                    .sum();
                let video_posts_count: usize = self
                    .video_posts_by_user
                    .iter()
                    .map(|entry| entry.value().len())
                    .sum();

                // Update Prometheus gauges
                POST_STORE_USER_COUNT.set(user_count as f64);
                POST_STORE_TOTAL_POSTS.set(total_posts as f64);
                POST_STORE_DELETED_POSTS.set(deleted_posts as f64);

                // Update entity count gauge with labels
                POST_STORE_ENTITY_COUNT
                    .with_label_values(&["users"])
                    .set(user_count as f64);
                POST_STORE_ENTITY_COUNT
                    .with_label_values(&["posts"])
                    .set(total_posts as f64);
                POST_STORE_ENTITY_COUNT
                    .with_label_values(&["original_posts"])
                    .set(original_posts_count as f64);
                POST_STORE_ENTITY_COUNT
                    .with_label_values(&["secondary_posts"])
                    .set(secondary_posts_count as f64);
                POST_STORE_ENTITY_COUNT
                    .with_label_values(&["video_posts"])
                    .set(video_posts_count as f64);
                POST_STORE_ENTITY_COUNT
                    .with_label_values(&["deleted_posts"])
                    .set(deleted_posts as f64);

                info!(
                    "PostStore Stats: {} users, {} total posts, {} deleted posts",
                    user_count, total_posts, deleted_posts
                );
            }
        });
    }

    /// Start a background task that periodically trims old posts
    pub fn start_auto_trim(self: Arc<Self>, interval_minutes: u64) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(interval_minutes * 60));

            loop {
                interval.tick().await;
                let trimmed = self.trim_old_posts().await;
                if trimmed > 0 {
                    info!("Auto-trim: removed {} old posts", trimmed);
                }
            }
        });
    }

    /// Manually trim posts older than retention period from all users
    /// Returns the number of posts trimmed
    pub async fn trim_old_posts(&self) -> usize {
        let posts_map = Arc::clone(&self.posts);
        let original_posts_by_user = Arc::clone(&self.original_posts_by_user);
        let secondary_posts_by_user = Arc::clone(&self.secondary_posts_by_user);
        let video_posts_by_user = Arc::clone(&self.video_posts_by_user);
        let deleted_posts = Arc::clone(&self.deleted_posts);
        let retention_seconds = self.retention_seconds;

        tokio::task::spawn_blocking(move || {
            let current_time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();

            let mut total_trimmed = 0;

            // Helper closure to trim posts from a given map
            let trim_map = |posts_by_user: &DashMap<i64, VecDeque<TinyPost>>,
                            posts_map: &DashMap<i64, LightPost>,
                            deleted_posts: &DashMap<i64, bool>|
             -> usize {
                let mut trimmed = 0;
                let mut users_to_remove = Vec::new();

                for mut entry in posts_by_user.iter_mut() {
                    let user_id = *entry.key();
                    let user_posts = entry.value_mut();

                    while let Some(oldest_post) = user_posts.front() {
                        if current_time - (oldest_post.created_at as u64) > retention_seconds {
                            let trimmed_post = user_posts.pop_front().unwrap();
                            posts_map.remove(&trimmed_post.post_id);

                            if user_id == DELETE_EVENT_KEY {
                                deleted_posts.remove(&trimmed_post.post_id);
                            }
                            trimmed += 1;
                        } else {
                            break;
                        }
                    }

                    if user_posts.capacity() > user_posts.len() * 2 {
                        let new_cap = user_posts.len() as f32 * 1.5_f32;
                        user_posts.shrink_to(new_cap as usize);
                    }

                    if user_posts.is_empty() {
                        users_to_remove.push(user_id);
                    }
                }

                for user_id in users_to_remove {
                    posts_by_user.remove_if(&user_id, |_, posts| posts.is_empty());
                }

                trimmed
            };

            total_trimmed += trim_map(&original_posts_by_user, &posts_map, &deleted_posts);
            total_trimmed += trim_map(&secondary_posts_by_user, &posts_map, &deleted_posts);
            trim_map(&video_posts_by_user, &posts_map, &deleted_posts);

            total_trimmed
        })
        .await
        .expect("spawn_blocking failed")
    }

    /// Sorts all user post lists by creation time (newest first)
    pub async fn sort_all_user_posts(&self) {
        let original_posts_by_user = Arc::clone(&self.original_posts_by_user);
        let secondary_posts_by_user = Arc::clone(&self.secondary_posts_by_user);
        let video_posts_by_user = Arc::clone(&self.video_posts_by_user);

        tokio::task::spawn_blocking(move || {
            // Sort original posts
            for mut entry in original_posts_by_user.iter_mut() {
                let user_posts = entry.value_mut();
                user_posts
                    .make_contiguous()
                    .sort_unstable_by_key(|a| a.created_at);
            }
            // Sort secondary posts
            for mut entry in secondary_posts_by_user.iter_mut() {
                let user_posts = entry.value_mut();
                user_posts
                    .make_contiguous()
                    .sort_unstable_by_key(|a| a.created_at);
            }
            // Sort video posts
            for mut entry in video_posts_by_user.iter_mut() {
                let user_posts = entry.value_mut();
                user_posts
                    .make_contiguous()
                    .sort_unstable_by_key(|a| a.created_at);
            }
        })
        .await
        .expect("spawn_blocking failed");
    }

    /// Clears all posts from the store
    pub fn clear(&self) {
        self.posts.clear();
        self.original_posts_by_user.clear();
        self.secondary_posts_by_user.clear();
        self.video_posts_by_user.clear();
        info!("PostStore cleared");
    }
}

impl Default for PostStore {
    fn default() -> Self {
        // Default to 2 days retention, no timeout
        Self::new(2 * 24 * 60 * 60, 0)
    }
}
