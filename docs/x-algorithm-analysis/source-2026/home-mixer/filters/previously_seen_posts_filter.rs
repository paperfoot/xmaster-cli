use crate::candidate_pipeline::candidate::PostCandidate;
use crate::candidate_pipeline::query::ScoredPostsQuery;
use crate::util::bloom_filter::BloomFilter;
use crate::util::candidates_util::get_related_post_ids;
use tonic::async_trait;
use xai_candidate_pipeline::filter::{Filter, FilterResult};

/// Filter out previously seen posts using a Bloom Filter and
/// the seen IDs sent in the request directly from the client
pub struct PreviouslySeenPostsFilter;

#[async_trait]
impl Filter<ScoredPostsQuery, PostCandidate> for PreviouslySeenPostsFilter {
    async fn filter(
        &self,
        query: &ScoredPostsQuery,
        candidates: Vec<PostCandidate>,
    ) -> Result<FilterResult<PostCandidate>, String> {
        let bloom_filters = query
            .bloom_filter_entries
            .iter()
            .map(BloomFilter::from_entry)
            .collect::<Vec<_>>();

        let (removed, kept): (Vec<_>, Vec<_>) = candidates.into_iter().partition(|c| {
            get_related_post_ids(c).iter().any(|&post_id| {
                query.seen_ids.contains(&post_id)
                    || bloom_filters
                        .iter()
                        .any(|filter| filter.may_contain(post_id))
            })
        });

        Ok(FilterResult { kept, removed })
    }
}
