use crate::candidate_pipeline::candidate::PostCandidate;
use crate::candidate_pipeline::query::ScoredPostsQuery;
use crate::params as p;
use std::cmp::Ordering;
use std::collections::HashMap;
use tonic::async_trait;
use xai_candidate_pipeline::scorer::Scorer;

/// Diversify authors served within a single feed response
pub struct AuthorDiversityScorer {
    decay_factor: f64,
    floor: f64,
}

impl Default for AuthorDiversityScorer {
    fn default() -> Self {
        Self::new(p::AUTHOR_DIVERSITY_DECAY, p::AUTHOR_DIVERSITY_FLOOR)
    }
}

impl AuthorDiversityScorer {
    pub fn new(decay_factor: f64, floor: f64) -> Self {
        Self {
            decay_factor,
            floor,
        }
    }

    fn multiplier(&self, position: usize) -> f64 {
        (1.0 - self.floor) * self.decay_factor.powf(position as f64) + self.floor
    }
}

#[async_trait]
impl Scorer<ScoredPostsQuery, PostCandidate> for AuthorDiversityScorer {
    #[xai_stats_macro::receive_stats]
    async fn score(
        &self,
        _query: &ScoredPostsQuery,
        candidates: &[PostCandidate],
    ) -> Result<Vec<PostCandidate>, String> {
        let mut author_counts: HashMap<u64, usize> = HashMap::new();
        let mut scored = vec![PostCandidate::default(); candidates.len()];

        let mut ordered: Vec<(usize, &PostCandidate)> = candidates.iter().enumerate().collect();
        ordered.sort_by(|(_, a), (_, b)| {
            let a_score = a.weighted_score.unwrap_or(f64::NEG_INFINITY);
            let b_score = b.weighted_score.unwrap_or(f64::NEG_INFINITY);
            b_score.partial_cmp(&a_score).unwrap_or(Ordering::Equal)
        });

        for (original_idx, candidate) in ordered {
            let entry = author_counts.entry(candidate.author_id).or_insert(0);
            let position = *entry;
            *entry += 1;

            let multiplier = self.multiplier(position);
            let adjusted_score = candidate.weighted_score.map(|score| score * multiplier);

            let updated = PostCandidate {
                score: adjusted_score,
                ..Default::default()
            };
            scored[original_idx] = updated;
        }

        Ok(scored)
    }

    fn update(&self, candidate: &mut PostCandidate, scored: PostCandidate) {
        candidate.score = scored.score;
    }
}