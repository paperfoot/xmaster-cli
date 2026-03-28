# X "For You" Feed Algorithm -- Technical Architecture Analysis (January 2026)

This document is a comprehensive technical analysis of the X recommendation algorithm as published in the [source-2026 repository](source-2026/). Every claim is traced to an exact file path and function name. This is the system that is **currently running** on X -- not the 2023 twitter/the-algorithm release, which is dead code.

---

## 1. Complete Pipeline Architecture

The For You feed is served by a gRPC service called **Home Mixer** (`ScoredPostsService`). A single request enters at `HomeMixerServer::get_scored_posts()` in `home-mixer/server.rs` and flows through these stages in strict order:

### 1.1 Entry Point

The server boots in `home-mixer/main.rs`. It instantiates a `HomeMixerServer` which holds a single `PhoenixCandidatePipeline`. The gRPC service is registered as `ScoredPostsServiceServer` on a configurable port with Gzip and Zstd compression.

When a request arrives, `get_scored_posts()` (`home-mixer/server.rs:27`) constructs a `ScoredPostsQuery` from the proto and calls `self.phx_candidate_pipeline.execute(query)`.

### 1.2 Pipeline Execution Order

The pipeline framework lives in `candidate-pipeline/candidate_pipeline.rs`. The `execute()` method (line 53) runs these stages sequentially:

```
1. hydrate_query()       -- Query hydrators run in PARALLEL via join_all()
2. fetch_candidates()    -- Sources run in PARALLEL via join_all()
3. hydrate()             -- Candidate hydrators run in PARALLEL via join_all()
4. filter()              -- Pre-scoring filters run SEQUENTIALLY
5. score()               -- Scorers run SEQUENTIALLY
6. select()              -- Top-K selection
7. hydrate_post_selection() -- Post-selection hydrators (parallel)
8. filter_post_selection()  -- Post-selection filters (sequential)
9. run_side_effects()    -- Side effects fire-and-forget (tokio::spawn)
```

The concrete pipeline is wired in `home-mixer/candidate_pipeline/phoenix_candidate_pipeline.rs` in the `build_with_clients()` method (line 73).

### 1.3 Query Hydration

Two query hydrators run in parallel:

| Hydrator | File | Purpose |
|----------|------|---------|
| `UserActionSeqQueryHydrator` | `home-mixer/query_hydrators/user_action_seq_query_hydrator.rs` | Fetches the user's recent engagement history (the `UserActionSequence` protobuf) |
| `UserFeaturesQueryHydrator` | `home-mixer/query_hydrators/user_features_query_hydrator.rs` | Fetches following list, blocked/muted user IDs, muted keywords, subscriptions |

### 1.4 Candidate Sourcing

Two sources run in parallel:

| Source | File | Type |
|--------|------|------|
| `ThunderSource` | `home-mixer/sources/thunder_source.rs` | In-network posts from accounts you follow |
| `PhoenixSource` | `home-mixer/sources/phoenix_source.rs` | Out-of-network posts discovered by ML retrieval |

`PhoenixSource` is disabled when `query.in_network_only` is true (its `enable()` method, line 16). `ThunderSource` calls `InNetworkPostsServiceClient::get_in_network_posts()` via gRPC to the Thunder service. `PhoenixSource` calls `PhoenixRetrievalClient::retrieve()` to the Phoenix retrieval service.

### 1.5 Candidate Hydration

Five hydrators enrich the raw candidate IDs in parallel:

| Hydrator | File | Purpose |
|----------|------|---------|
| `InNetworkCandidateHydrator` | `home-mixer/candidate_hydrators/in_network_candidate_hydrator.rs` | Marks candidates as in-network or out-of-network |
| `CoreDataCandidateHydrator` | `home-mixer/candidate_hydrators/core_data_candidate_hydrator.rs` | Fetches tweet text, media, metadata from TES |
| `VideoDurationCandidateHydrator` | `home-mixer/candidate_hydrators/video_duration_candidate_hydrator.rs` | Fetches video duration for video posts |
| `SubscriptionHydrator` | `home-mixer/candidate_hydrators/subscription_hydrator.rs` | Marks subscription-only content |
| `GizmoduckCandidateHydrator` | `home-mixer/candidate_hydrators/gizmoduck_hydrator.rs` | Fetches author info (screen name, follower count, verification) |

### 1.6 Pre-Scoring Filters (10 filters, sequential)

See Section 7 for detailed analysis.

### 1.7 Scoring (4 scorers, sequential)

```
PhoenixScorer -> WeightedScorer -> AuthorDiversityScorer -> OONScorer
```

See Sections 4-6 for detailed analysis.

### 1.8 Selection

`TopKScoreSelector` (`home-mixer/selectors/top_k_score_selector.rs`) sorts by `candidate.score` descending and takes the top `params::TOP_K_CANDIDATES_TO_SELECT` candidates.

### 1.9 Post-Selection Processing

One post-selection hydrator runs:
- `VFCandidateHydrator` -- calls the Visibility Filtering service to check for deleted/spam/violence/gore

Two post-selection filters run:
- `VFFilter` -- drops posts flagged by VF
- `DedupConversationFilter` -- keeps only the highest-scored post per conversation thread

### 1.10 Side Effects

`CacheRequestInfoSideEffect` (`home-mixer/side_effects/cache_request_info_side_effect.rs`) fires asynchronously after the response is built, caching request info to Strato for future use.

---

## 2. Phoenix Ranking Model (Grok Transformer)

**Files:** `phoenix/recsys_model.py`, `phoenix/grok.py`

The ranking model is a **Grok-1-derived transformer** adapted for recommendation. The README explicitly states: *"The transformer implementation is ported from the Grok-1 open source release by xAI."*

### 2.1 Model Configuration

Defined in `PhoenixModelConfig` (`phoenix/recsys_model.py:246`):

```python
@dataclass
class PhoenixModelConfig:
    model: TransformerConfig   # Grok transformer config
    emb_size: int              # Embedding dimension
    num_actions: int           # Number of engagement actions to predict
    history_seq_len: int = 128 # Max history length
    candidate_seq_len: int = 32 # Max candidates to rank
    product_surface_vocab_size: int = 16
    fprop_dtype: Any = jnp.bfloat16
    hash_config: HashConfig    # Hash embedding config
```

### 2.2 Input Assembly

The `PhoenixModel.build_inputs()` method (`phoenix/recsys_model.py:365`) constructs the transformer input as a concatenation of three blocks:

```
Input sequence = [User Embedding | History Embeddings | Candidate Embeddings]
                  [B, 1, D]        [B, S, D]            [B, C, D]
```

Each block is built by a dedicated "reduce" function:

**User block** -- `block_user_reduce()` (line 79): Takes `[B, num_user_hashes]` user hash IDs and their looked-up embeddings `[B, num_user_hashes, D]`. Concatenates the hash embeddings and projects through a learned matrix `proj_mat_1` of shape `[num_user_hashes * D, D]`. Output: `[B, 1, D]`.

**History block** -- `block_history_reduce()` (line 122): For each history position, concatenates four components:
1. Post hash embeddings `[B, S, num_item_hashes * D]`
2. Author hash embeddings `[B, S, num_author_hashes * D]`
3. Action embeddings `[B, S, D]` -- what the user did (liked, replied, etc.)
4. Product surface embeddings `[B, S, D]` -- where the user saw it (home timeline, search, etc.)

All four are concatenated along the feature dimension, then projected through `proj_mat_3` to `[B, S, D]`.

**Candidate block** -- `block_candidate_reduce()` (line 185): Same as history but without actions (candidates haven't been interacted with yet). Concatenates post hashes, author hashes, and product surface embeddings, projects through `proj_mat_2` to `[B, C, D]`.

### 2.3 Hash-Based Embeddings

All entity types (users, posts, authors) are represented via multiple hash functions. The `HashConfig` (`phoenix/recsys_model.py:33`) defaults to:

```python
num_user_hashes: int = 2
num_item_hashes: int = 2
num_author_hashes: int = 2
```

Hash value 0 is reserved for padding. The embeddings are pre-looked-up externally and passed as `RecsysEmbeddings` to the model. This allows the embedding tables to be served separately from the transformer.

### 2.4 Action Embeddings

`PhoenixModel._get_action_embeddings()` (line 293) converts multi-hot action vectors to embeddings. Actions are encoded as signed values: `actions_signed = (2 * actions - 1)` so that 0 maps to -1 and 1 maps to +1. These are projected through a learned `action_projection` matrix of shape `[num_actions, D]`.

### 2.5 Candidate Isolation Masking

This is the single most important architectural decision. The function `make_recsys_attn_mask()` (`phoenix/grok.py:39`) creates an attention mask where:

- **User + History positions**: Standard causal attention among themselves
- **Candidates -> User/History**: Each candidate CAN attend to all user+history positions
- **Candidates -> Other candidates**: Each candidate can ONLY attend to itself (diagonal self-attention)

Implementation (line 61-71):
```python
causal_mask = jnp.tril(jnp.ones((1, 1, seq_len, seq_len)))
attn_mask = causal_mask.at[:, :, candidate_start_offset:, candidate_start_offset:].set(0)
candidate_indices = jnp.arange(candidate_start_offset, seq_len)
attn_mask = attn_mask.at[:, :, candidate_indices, candidate_indices].set(1)
```

This ensures every candidate's score is **independent of which other candidates are in the batch** -- a critical property for score consistency and cacheability.

The mask is applied inside `Transformer.__call__()` (line 516) when `candidate_start_offset` is not None.

### 2.6 Transformer Architecture

Defined in `phoenix/grok.py`:

**`Transformer`** (line 504): N layers of `DecoderLayer`, each containing:
1. Pre-norm RMSNorm
2. `MHABlock` with grouped-query attention (GQA via `num_q_heads` / `num_kv_heads`)
3. Post-attention RMSNorm + residual
4. Pre-norm RMSNorm
5. `DenseBlock` (gated FFN with GELU activation)
6. Post-FFN RMSNorm + residual

**`MultiHeadAttention`** (line 264): Uses rotary position embeddings (RoPE) via `RotaryEmbedding` (line 205). Supports grouped-query attention where `num_q_heads` is a multiple of `num_kv_heads`. Attention logits are clamped via `tanh(logits / 30) * 30` to prevent overflow.

**`DenseBlock`** (line 414): Gated feed-forward network. Two parallel linear projections -- one through GELU, one without -- are multiplied element-wise, then projected back. The hidden size is `ffn_size()` (line 32): `int(widening_factor * emb_size) * 2 // 3`, rounded up to a multiple of 8.

### 2.7 Output Projection

After the transformer, `PhoenixModel.__call__()` (line 439):
1. Applies `layer_norm()` to all output embeddings
2. Extracts only the candidate positions: `out_embeddings[:, candidate_start_offset:, :]`
3. Projects through an unembedding matrix `[emb_size, num_actions]` to get logits

Output shape: `[B, num_candidates, num_actions]`

In the inference runner (`phoenix/runners.py:343`), these logits are converted to probabilities via `jax.nn.sigmoid(logits)`.

---

## 3. Phoenix Retrieval Model (Two-Tower)

**Files:** `phoenix/recsys_retrieval_model.py`, `phoenix/grok.py`

### 3.1 Architecture

`PhoenixRetrievalModel` (`phoenix/recsys_retrieval_model.py:144`) implements a two-tower architecture:

**User Tower** -- `build_user_representation()` (line 206): Uses the **same Grok transformer** as the ranking model (not a separate architecture). It:
1. Builds user + history embeddings using the same `block_user_reduce()` and `block_history_reduce()` functions
2. Concatenates `[user_embeddings, history_embeddings]` (no candidates)
3. Runs through the transformer with `candidate_start_offset=None` (standard causal masking)
4. Mean-pools the transformer outputs weighted by the padding mask
5. L2-normalizes the result to unit norm

Output: `[B, D]` -- one normalized embedding per user.

**Candidate Tower** -- `CandidateTower` (line 47): A simple 2-layer MLP with SiLU activation:
```python
hidden = jnp.dot(post_author_embedding, proj_1)  # [input_dim -> emb_size * 2]
hidden = jax.nn.silu(hidden)
candidate_embeddings = jnp.dot(hidden, proj_2)     # [emb_size * 2 -> emb_size]
```
The output is L2-normalized to unit norm. Input is the concatenation of post and author hash embeddings.

### 3.2 Retrieval

`_retrieve_top_k()` (line 346):
```python
scores = jnp.matmul(user_representation, corpus_embeddings.T)  # [B, N] dot product
top_k_scores, top_k_indices = jax.lax.top_k(scores, top_k)
```

Because both towers produce L2-normalized outputs, the dot product equals cosine similarity. An optional `corpus_mask` allows masking out invalid corpus entries by setting their scores to `-INF`.

### 3.3 Key Design: Shared Architecture

The retrieval user tower uses the exact same `TransformerConfig` and `Transformer` class as the ranking model. This means user representations are encoded with the same quality model in both stages.

---

## 4. WeightedScorer -- The Scoring Formula

**File:** `home-mixer/scorers/weighted_scorer.rs`

### 4.1 The 19 Engagement Signals

The `compute_weighted_score()` method (line 44) combines **19 predicted engagement probabilities** from the Phoenix model:

| # | Signal | Weight Param | Sign |
|---|--------|-------------|------|
| 1 | `favorite_score` (like) | `p::FAVORITE_WEIGHT` | Positive |
| 2 | `reply_score` | `p::REPLY_WEIGHT` | Positive |
| 3 | `retweet_score` | `p::RETWEET_WEIGHT` | Positive |
| 4 | `photo_expand_score` | `p::PHOTO_EXPAND_WEIGHT` | Positive |
| 5 | `click_score` | `p::CLICK_WEIGHT` | Positive |
| 6 | `profile_click_score` | `p::PROFILE_CLICK_WEIGHT` | Positive |
| 7 | `vqv_score` (video quality view) | `p::VQV_WEIGHT` (conditional) | Positive |
| 8 | `share_score` | `p::SHARE_WEIGHT` | Positive |
| 9 | `share_via_dm_score` | `p::SHARE_VIA_DM_WEIGHT` | Positive |
| 10 | `share_via_copy_link_score` | `p::SHARE_VIA_COPY_LINK_WEIGHT` | Positive |
| 11 | `dwell_score` | `p::DWELL_WEIGHT` | Positive |
| 12 | `quote_score` | `p::QUOTE_WEIGHT` | Positive |
| 13 | `quoted_click_score` | `p::QUOTED_CLICK_WEIGHT` | Positive |
| 14 | `dwell_time` (continuous) | `p::CONT_DWELL_TIME_WEIGHT` | Positive |
| 15 | `follow_author_score` | `p::FOLLOW_AUTHOR_WEIGHT` | Positive |
| 16 | `not_interested_score` | `p::NOT_INTERESTED_WEIGHT` | **Negative** |
| 17 | `block_author_score` | `p::BLOCK_AUTHOR_WEIGHT` | **Negative** |
| 18 | `mute_author_score` | `p::MUTE_AUTHOR_WEIGHT` | **Negative** |
| 19 | `report_score` | `p::REPORT_WEIGHT` | **Negative** |

### 4.2 The Formula

```
combined_score = SUM(P(action_i) * weight_i)  for i in 1..19
final_score = offset_score(combined_score)
normalized_score = normalize_score(candidate, final_score)
```

The `apply()` helper (line 40) handles missing predictions: `score.unwrap_or(0.0) * weight`.

### 4.3 VQV Weight Eligibility

`vqv_weight_eligibility()` (line 72): The video quality view weight is only applied if the post has a video longer than `p::MIN_VIDEO_DURATION_MS`. Otherwise it contributes 0.

### 4.4 The `offset_score()` Normalization

`offset_score()` (line 83) prevents negative scores from dominating:

```rust
fn offset_score(combined_score: f64) -> f64 {
    if p::WEIGHTS_SUM == 0.0 {
        combined_score.max(0.0)
    } else if combined_score < 0.0 {
        (combined_score + p::NEGATIVE_WEIGHTS_SUM) / p::WEIGHTS_SUM * p::NEGATIVE_SCORES_OFFSET
    } else {
        combined_score + p::NEGATIVE_SCORES_OFFSET
    }
}
```

For positive scores, the offset simply shifts them up by `NEGATIVE_SCORES_OFFSET`. For negative scores, the formula compresses the negative range into `[0, NEGATIVE_SCORES_OFFSET]` proportionally. This ensures all output scores are non-negative.

### 4.5 Weight Values Are NOT Published

All weight constants are imported from `crate::params as p`. This module is **not included** in the published source. The actual numerical values of `FAVORITE_WEIGHT`, `REPLY_WEIGHT`, etc. are proprietary.

---

## 5. OON Scorer

**File:** `home-mixer/scorers/oon_scorer.rs`

The `OONScorer` adjusts scores for out-of-network content. It runs **after** the `AuthorDiversityScorer`, so it operates on the final `score` field.

```rust
let updated_score = c.score.map(|base_score| match c.in_network {
    Some(false) => base_score * p::OON_WEIGHT_FACTOR,
    _ => base_score,
});
```

If `in_network` is `Some(false)` (meaning the post came from `PhoenixSource`, not `ThunderSource`), the score is multiplied by `p::OON_WEIGHT_FACTOR`. The actual value is not published, but the architecture implies it is less than 1.0 to prioritize in-network content, or it could be greater than 1.0 if the system is tuned to favor discovery.

In-network candidates and candidates with no `in_network` flag are left unchanged.

---

## 6. Author Diversity Scorer

**File:** `home-mixer/scorers/author_diversity_scorer.rs`

### 6.1 Purpose

Prevents a single prolific author from dominating the feed by applying exponential decay to repeated appearances.

### 6.2 Algorithm

`AuthorDiversityScorer::score()` (line 37):

1. Sort candidates by `weighted_score` descending
2. Track how many times each author has appeared (`author_counts` HashMap)
3. For the N-th appearance of an author (0-indexed), multiply the score by:

```
multiplier(position) = (1 - floor) * decay^position + floor
```

Implemented in `multiplier()` (line 29):
```rust
fn multiplier(&self, position: usize) -> f64 {
    (1.0 - self.floor) * self.decay_factor.powf(position as f64) + self.floor
}
```

### 6.3 Behavior

- `position = 0` (first post): multiplier = 1.0 (no penalty)
- `position = 1` (second post): multiplier = `(1-floor) * decay + floor`
- `position = N`: approaches `floor` as N grows

The `decay_factor` and `floor` values come from `p::AUTHOR_DIVERSITY_DECAY` and `p::AUTHOR_DIVERSITY_FLOOR` (defaults via `Default::default()`, line 15).

The scorer writes to `candidate.score` (not `weighted_score`), so this is the value used for final selection.

---

## 7. All 12 Filters

### 7.1 Pre-Scoring Filters (10 filters)

These run sequentially **before** scoring, in the order wired in `phoenix_candidate_pipeline.rs:109`:

**1. `DropDuplicatesFilter`** (`home-mixer/filters/drop_duplicates_filter.rs`)
Removes duplicate `tweet_id` values using a HashSet. First occurrence wins.

**2. `CoreDataHydrationFilter`** (`home-mixer/filters/core_data_hydration_filter.rs`)
Removes candidates where `author_id == 0` or `tweet_text` is empty. This catches posts that failed to hydrate core metadata from TES.

**3. `AgeFilter`** (`home-mixer/filters/age_filter.rs`)
Removes posts older than `params::MAX_POST_AGE` seconds. Uses Snowflake ID timestamp extraction via `snowflake::duration_since_creation_opt()`.

**4. `SelfTweetFilter`** (`home-mixer/filters/self_tweet_filter.rs`)
Removes posts where `author_id == viewer_id`. You do not see your own posts in For You.

**5. `RetweetDeduplicationFilter`** (`home-mixer/filters/retweet_deduplication_filter.rs`)
Deduplicates retweets. If you follow both Alice and Bob, and both retweeted the same post, only the first retweet survives. Tracks seen `retweeted_tweet_id` values; original posts also register their IDs so retweets of already-seen originals are dropped.

**6. `IneligibleSubscriptionFilter`** (`home-mixer/filters/ineligible_subscription_filter.rs`)
Removes subscription-only (paywalled) posts from authors the viewer has not subscribed to. Checks `candidate.subscription_author_id` against `query.user_features.subscribed_user_ids`.

**7. `PreviouslySeenPostsFilter`** (`home-mixer/filters/previously_seen_posts_filter.rs`)
Removes posts the user has already seen. Uses two mechanisms:
- `query.seen_ids` -- explicit IDs sent from the client
- `query.bloom_filter_entries` -- Bloom filters of impression history, checked via `BloomFilter::from_entry()` and `may_contain()`

**8. `PreviouslyServedPostsFilter`** (`home-mixer/filters/previously_served_posts_filter.rs`)
Removes posts already served in the current session. Only enabled for bottom-of-feed ("load more") requests: `fn enable(&self, query) -> bool { query.is_bottom_request }`. Checks `query.served_ids`.

**9. `MutedKeywordFilter`** (`home-mixer/filters/muted_keyword_filter.rs`)
Removes posts containing the user's muted keywords. Uses `TweetTokenizer` for tokenization and `MatchTweetGroup` for matching against `UserMutes`.

**10. `AuthorSocialgraphFilter`** (`home-mixer/filters/author_socialgraph_filter.rs`)
Removes posts from authors the viewer has blocked or muted. Checks `candidate.author_id` against `query.user_features.blocked_user_ids` and `query.user_features.muted_user_ids`.

### 7.2 Post-Selection Filters (2 filters)

These run **after** scoring and selection, in the order wired in `phoenix_candidate_pipeline.rs:142`:

**11. `VFFilter`** (`home-mixer/filters/vf_filter.rs`)
Visibility Filtering. Removes posts flagged by the VF service as deleted, spam, violence, gore, etc. The `should_drop()` function (line 25) checks for `FilteredReason::SafetyResult` with `Action::Drop`, or any other `FilteredReason` variant.

**12. `DedupConversationFilter`** (`home-mixer/filters/dedup_conversation_filter.rs`)
Deduplicates conversation branches. For posts in the same conversation thread (identified by `get_conversation_id()` which returns the earliest ancestor), only the highest-scored candidate survives. This prevents the feed from showing multiple branches of the same conversation.

---

## 8. Thunder -- In-Memory Post Store

**Files:** `thunder/posts/post_store.rs`, `thunder/thunder_service.rs`, `thunder/kafka/tweet_events_listener.rs`, `thunder/kafka/tweet_events_listener_v2.rs`, `thunder/main.rs`

### 8.1 Architecture

Thunder is a standalone Rust service that provides sub-millisecond in-network post lookups. It:
- Runs as an independent gRPC server (`InNetworkPostsService`)
- Stores all recent posts entirely in memory
- Consumes post create/delete events from Kafka in real time
- Serves requests from Home Mixer via gRPC

### 8.2 PostStore Data Structures

`PostStore` (`thunder/posts/post_store.rs:39`) uses `DashMap` (concurrent HashMap) for thread-safe access without global locks:

```rust
pub struct PostStore {
    posts: Arc<DashMap<i64, LightPost>>,                     // Full post data by post_id
    original_posts_by_user: Arc<DashMap<i64, VecDeque<TinyPost>>>,  // Original posts per author
    secondary_posts_by_user: Arc<DashMap<i64, VecDeque<TinyPost>>>, // Replies + retweets per author
    video_posts_by_user: Arc<DashMap<i64, VecDeque<TinyPost>>>,     // Video posts per author
    deleted_posts: Arc<DashMap<i64, bool>>,                  // Deleted post tracking
    retention_seconds: u64,                                   // Default: 2 days (172800s)
    request_timeout: Duration,                                // Per-request timeout
}
```

`TinyPost` (line 21) is a minimal reference containing only `post_id` and `created_at` to keep per-user timelines compact.

### 8.3 Per-User Stores

Posts are categorized into three stores per user:
- **`original_posts_by_user`**: Non-reply, non-retweet posts. Capped at `MAX_ORIGINAL_POSTS_PER_AUTHOR`.
- **`secondary_posts_by_user`**: Replies and retweets. Capped at `MAX_REPLY_POSTS_PER_AUTHOR`.
- **`video_posts_by_user`**: Posts with eligible video. Capped at `MAX_VIDEO_POSTS_PER_AUTHOR`.

### 8.4 Kafka Ingestion

Two ingestion pipelines exist:

**V1 Pipeline** (`thunder/kafka/tweet_events_listener.rs`): Reads Thrift-encoded `TweetEvent` from Kafka, extracts `LightPost` data, and optionally re-publishes as protobuf `InNetworkEvent` to a secondary Kafka topic. Runs in feeder mode (non-serving).

**V2 Pipeline** (`thunder/kafka/tweet_events_listener_v2.rs`): Reads protobuf-encoded `InNetworkEvent` from Kafka and directly inserts into the `PostStore`. Runs in serving mode. Uses a semaphore (3 permits) after initial catchup to prevent CPU-starving the serving path.

The V2 pipeline has a catchup mechanism: it monitors partition lag and signals completion via a channel when lag drops below threshold (`lags.len() * batch_size`).

### 8.5 Service Layer

`ThunderServiceImpl::get_in_network_posts()` (`thunder/thunder_service.rs:154`):
1. Acquires a semaphore permit (rejects with `RESOURCE_EXHAUSTED` if at capacity)
2. Fetches following list from request or from Strato
3. Calls `post_store.get_all_posts_by_users()` on a `spawn_blocking` thread
4. Scores by recency: `score_recent()` (line 334) sorts by `created_at` descending
5. Returns top `max_results` posts

For video requests, it calls `post_store.get_videos_by_users()` instead.

### 8.6 Auto-Trimming

`PostStore::start_auto_trim()` (line 393) runs every 2 minutes and removes posts older than `retention_seconds`. It also shrinks VecDeque capacity when utilization drops below 50%.

---

## 9. PhoenixScorer -- ML Prediction Extraction

**File:** `home-mixer/scorers/phoenix_scorer.rs`

`PhoenixScorer::score()` (line 19):
1. Extracts `user_id` and constructs a `prediction_request_id`
2. Builds `TweetInfo` for each candidate (using `retweeted_tweet_id` or `tweet_id` and corresponding author ID)
3. Calls `self.phoenix_client.predict(user_id, sequence, tweet_infos)` -- the gRPC call to the Phoenix ranking service
4. Builds a `HashMap<u64, ActionPredictions>` mapping tweet_id to predictions
5. Extracts 18 discrete action probabilities + 1 continuous action value from the response

The response contains `top_log_probs` (log probabilities) which are exponentiated via `(*log_prob as f64).exp()` (line 107) to recover actual probabilities. Continuous action values (like `DwellTime`) are taken directly.

For retweets, the scorer looks up predictions using the original tweet ID, not the retweet wrapper.

---

## 10. What Is NOT in the Repository

### 10.1 Exact Weight Values

All scoring weights are in `crate::params` which is **not published**. This includes:
- All 19 engagement weights (`FAVORITE_WEIGHT`, `REPLY_WEIGHT`, etc.)
- `OON_WEIGHT_FACTOR`
- `AUTHOR_DIVERSITY_DECAY` and `AUTHOR_DIVERSITY_FLOOR`
- `NEGATIVE_WEIGHTS_SUM`, `WEIGHTS_SUM`, `NEGATIVE_SCORES_OFFSET`
- `MIN_VIDEO_DURATION_MS`
- `THUNDER_MAX_RESULTS`, `PHOENIX_MAX_RESULTS`, `TOP_K_CANDIDATES_TO_SELECT`, `RESULT_SIZE`
- `MAX_POST_AGE`

### 10.2 Training Data and Training Code

The published code is inference-only. There is no training loop, loss function, dataset pipeline, or data preprocessing code. The `runners.py` file provides initialization and inference wrappers but no training.

### 10.3 Embedding Tables

The hash-based embedding tables (user embeddings, post embeddings, author embeddings) are looked up externally and passed as `RecsysEmbeddings`. The embedding serving infrastructure is not included.

### 10.4 TweepCred -- Removed

The 2023 algorithm used TweepCred (a PageRank-based author reputation score). There is **zero reference** to TweepCred anywhere in this codebase. It has been completely eliminated in favor of the transformer learning author relevance directly from engagement history.

### 10.5 SimClusters -- Removed

The 2023 algorithm used SimClusters (community-based collaborative filtering) as a primary retrieval source. There is **zero reference** to SimClusters in this codebase. Retrieval is now done entirely by the Phoenix two-tower model.

### 10.6 Real Graph -- Removed

The 2023 algorithm used Real Graph (a logistic regression model predicting pairwise user-user engagement) for in-network scoring. There is **zero reference** to Real Graph. The Grok transformer now handles all relevance prediction.

### 10.7 Earlybird / Lucene-based Search

The 2023 algorithm used Earlybird (a Lucene-based real-time search index) for candidate retrieval. There is **zero reference** to Earlybird. Thunder handles in-network retrieval and Phoenix handles out-of-network retrieval.

### 10.8 Model Scaling Parameters

The published transformer configs in the demo runners use tiny dimensions (emb_size=128, 2 layers, 2 heads) which are obviously not production scale. The README notes: *"This code is representative of the model used internally with the exception of specific scaling optimizations."*

### 10.9 Score Normalization Logic

The `normalize_score()` function called in `WeightedScorer::score()` (line 22) is imported from `crate::util::score_normalizer` but is not included in the published source.

---

## 11. Key Design Philosophy

### 11.1 "Zero Hand-Engineered Features"

From the README: *"We have eliminated every single hand-engineered feature and most heuristics from the system."*

The 2023 algorithm was full of hand-crafted features: TweepCred, SimClusters, Real Graph, Earlybird topic annotations, and dozens of feature engineering pipelines. The 2026 system replaces all of that with a single transformer that learns directly from raw engagement sequences.

The only inputs to the ranking model are:
- Hash IDs (user, post, author) -- no engineered features
- Multi-hot action vectors -- what the user did, not why
- Product surface indices -- where the user saw it

Everything else -- content relevance, author quality, topic matching, social affinity -- is learned end-to-end by the transformer.

### 11.2 Candidate Isolation is Non-Negotiable

The attention mask in `make_recsys_attn_mask()` ensures each candidate is scored as if it were the only candidate. This enables:
- **Score caching**: A post's score doesn't change based on what else is in the batch
- **Consistent ranking**: Same post always gets the same score for the same user
- **Serving efficiency**: Candidates can be scored in variable-size batches without affecting results

### 11.3 Two-Stage Architecture with Shared Backbone

The retrieval model's user tower uses the **same transformer architecture** as the ranking model. This is a deliberate design choice -- the retrieval stage benefits from the same powerful representation learning as the ranking stage.

### 11.4 Composable Pipeline Architecture

The `CandidatePipeline` trait (`candidate-pipeline/candidate_pipeline.rs`) separates pipeline orchestration from business logic. Adding a new filter, scorer, or source is as simple as implementing a trait and adding it to the pipeline configuration. The framework handles:
- Parallel execution of independent stages
- Sequential execution of dependent stages
- Graceful error handling (backup/restore on filter failure)
- Metrics and logging at every stage

### 11.5 Negative Signal Integration

Unlike systems that only optimize for positive engagement, the scoring formula explicitly includes negative signals (not interested, block, mute, report) with negative weights. This directly penalizes content that the model predicts the user would find objectionable.

---

## Appendix A: File Index

### Home Mixer (Rust)
| File | Purpose |
|------|---------|
| `home-mixer/main.rs` | Server entry point, gRPC setup |
| `home-mixer/server.rs` | `HomeMixerServer::get_scored_posts()` |
| `home-mixer/lib.rs` | Module declarations |
| `home-mixer/candidate_pipeline/phoenix_candidate_pipeline.rs` | Pipeline wiring |
| `home-mixer/candidate_pipeline/candidate.rs` | `PostCandidate`, `PhoenixScores` structs |
| `home-mixer/candidate_pipeline/query.rs` | `ScoredPostsQuery` struct |
| `home-mixer/scorers/phoenix_scorer.rs` | ML prediction extraction |
| `home-mixer/scorers/weighted_scorer.rs` | 19-signal scoring formula |
| `home-mixer/scorers/author_diversity_scorer.rs` | Exponential decay diversity |
| `home-mixer/scorers/oon_scorer.rs` | Out-of-network adjustment |
| `home-mixer/selectors/top_k_score_selector.rs` | Top-K selection |
| `home-mixer/sources/thunder_source.rs` | In-network candidate source |
| `home-mixer/sources/phoenix_source.rs` | Out-of-network candidate source |
| `home-mixer/filters/*.rs` | 12 filters (see Section 7) |

### Phoenix (Python/JAX)
| File | Purpose |
|------|---------|
| `phoenix/grok.py` | Grok transformer, RoPE, attention masking |
| `phoenix/recsys_model.py` | Ranking model, input assembly, hash reduction |
| `phoenix/recsys_retrieval_model.py` | Two-tower retrieval, candidate tower |
| `phoenix/runners.py` | Inference runners, `ACTIONS` list |
| `phoenix/run_ranker.py` | Ranking demo |
| `phoenix/run_retrieval.py` | Retrieval demo |

### Thunder (Rust)
| File | Purpose |
|------|---------|
| `thunder/main.rs` | Service entry point |
| `thunder/thunder_service.rs` | gRPC service, `get_in_network_posts()` |
| `thunder/posts/post_store.rs` | In-memory post store, DashMap-based |
| `thunder/kafka/tweet_events_listener.rs` | V1 Kafka ingestion (Thrift) |
| `thunder/kafka/tweet_events_listener_v2.rs` | V2 Kafka ingestion (Protobuf) |
| `thunder/deserializer.rs` | Thrift + Protobuf deserializers |
| `thunder/kafka_utils.rs` | Kafka consumer setup |

### Candidate Pipeline Framework (Rust)
| File | Purpose |
|------|---------|
| `candidate-pipeline/candidate_pipeline.rs` | Generic pipeline execution framework |
