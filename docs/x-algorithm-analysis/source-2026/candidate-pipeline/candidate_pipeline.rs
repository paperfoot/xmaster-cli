use crate::filter::Filter;
use crate::hydrator::Hydrator;
use crate::query_hydrator::QueryHydrator;
use crate::scorer::Scorer;
use crate::selector::Selector;
use crate::side_effect::{SideEffect, SideEffectInput};
use crate::source::Source;
use futures::future::join_all;
use log::{error, info, warn};
use std::sync::Arc;
use tonic::async_trait;

#[derive(Copy, Clone, Debug)]
pub enum PipelineStage {
    QueryHydrator,
    Source,
    Hydrator,
    PostSelectionHydrator,
    Filter,
    PostSelectionFilter,
    Scorer,
}

pub struct PipelineResult<Q, C> {
    pub retrieved_candidates: Vec<C>,
    pub filtered_candidates: Vec<C>,
    pub selected_candidates: Vec<C>,
    pub query: Arc<Q>,
}

/// Provides a stable request identifier for logging/tracing.
pub trait HasRequestId {
    fn request_id(&self) -> &str;
}

#[async_trait]
pub trait CandidatePipeline<Q, C>: Send + Sync
where
    Q: HasRequestId + Clone + Send + Sync + 'static,
    C: Clone + Send + Sync + 'static,
{
    fn query_hydrators(&self) -> &[Box<dyn QueryHydrator<Q>>];
    fn sources(&self) -> &[Box<dyn Source<Q, C>>];
    fn hydrators(&self) -> &[Box<dyn Hydrator<Q, C>>];
    fn filters(&self) -> &[Box<dyn Filter<Q, C>>];
    fn scorers(&self) -> &[Box<dyn Scorer<Q, C>>];
    fn selector(&self) -> &dyn Selector<Q, C>;
    fn post_selection_hydrators(&self) -> &[Box<dyn Hydrator<Q, C>>];
    fn post_selection_filters(&self) -> &[Box<dyn Filter<Q, C>>];
    fn side_effects(&self) -> Arc<Vec<Box<dyn SideEffect<Q, C>>>>;
    fn result_size(&self) -> usize;

    async fn execute(&self, query: Q) -> PipelineResult<Q, C> {
        let hydrated_query = self.hydrate_query(query).await;

        let candidates = self.fetch_candidates(&hydrated_query).await;

        let hydrated_candidates = self.hydrate(&hydrated_query, candidates).await;

        let (kept_candidates, mut filtered_candidates) = self
            .filter(&hydrated_query, hydrated_candidates.clone())
            .await;

        let scored_candidates = self.score(&hydrated_query, kept_candidates).await;

        let selected_candidates = self.select(&hydrated_query, scored_candidates);

        let post_selection_hydrated_candidates = self
            .hydrate_post_selection(&hydrated_query, selected_candidates)
            .await;

        let (mut final_candidates, post_selection_filtered_candidates) = self
            .filter_post_selection(&hydrated_query, post_selection_hydrated_candidates)
            .await;
        filtered_candidates.extend(post_selection_filtered_candidates);

        final_candidates.truncate(self.result_size());

        let arc_hydrated_query = Arc::new(hydrated_query);
        let input = Arc::new(SideEffectInput {
            query: arc_hydrated_query.clone(),
            selected_candidates: final_candidates.clone(),
        });
        self.run_side_effects(input);

        PipelineResult {
            retrieved_candidates: hydrated_candidates,
            filtered_candidates,
            selected_candidates: final_candidates,
            query: arc_hydrated_query,
        }
    }

    /// Run all query hydrators in parallel and merge results into the query.
    async fn hydrate_query(&self, query: Q) -> Q {
        let request_id = query.request_id().to_string();
        let hydrators: Vec<_> = self
            .query_hydrators()
            .iter()
            .filter(|h| h.enable(&query))
            .collect();
        let hydrate_futures = hydrators.iter().map(|h| h.hydrate(&query));
        let results = join_all(hydrate_futures).await;

        let mut hydrated_query = query;
        for (hydrator, result) in hydrators.iter().zip(results) {
            match result {
                Ok(hydrated) => {
                    hydrator.update(&mut hydrated_query, hydrated);
                }
                Err(err) => {
                    error!(
                        "request_id={} stage={:?} component={} failed: {}",
                        request_id,
                        PipelineStage::QueryHydrator,
                        hydrator.name(),
                        err
                    );
                }
            }
        }
        hydrated_query
    }

    /// Run all candidate sources in parallel and collect results.
    async fn fetch_candidates(&self, query: &Q) -> Vec<C> {
        let request_id = query.request_id().to_string();
        let sources: Vec<_> = self.sources().iter().filter(|s| s.enable(query)).collect();
        let source_futures = sources.iter().map(|s| s.get_candidates(query));
        let results = join_all(source_futures).await;

        let mut collected = Vec::new();
        for (source, result) in sources.iter().zip(results) {
            match result {
                Ok(mut candidates) => {
                    info!(
                        "request_id={} stage={:?} component={} fetched {} candidates",
                        request_id,
                        PipelineStage::Source,
                        source.name(),
                        candidates.len()
                    );
                    collected.append(&mut candidates);
                }
                Err(err) => {
                    error!(
                        "request_id={} stage={:?} component={} failed: {}",
                        request_id,
                        PipelineStage::Source,
                        source.name(),
                        err
                    );
                }
            }
        }
        collected
    }

    /// Run all candidate hydrators in parallel and merge results into candidates.
    async fn hydrate(&self, query: &Q, candidates: Vec<C>) -> Vec<C> {
        self.run_hydrators(query, candidates, self.hydrators(), PipelineStage::Hydrator)
            .await
    }

    /// Run post-selection candidate hydrators in parallel and merge results into candidates.
    async fn hydrate_post_selection(&self, query: &Q, candidates: Vec<C>) -> Vec<C> {
        self.run_hydrators(
            query,
            candidates,
            self.post_selection_hydrators(),
            PipelineStage::PostSelectionHydrator,
        )
        .await
    }

    /// Shared helper to hydrate with a provided hydrator list.
    async fn run_hydrators(
        &self,
        query: &Q,
        mut candidates: Vec<C>,
        hydrators: &[Box<dyn Hydrator<Q, C>>],
        stage: PipelineStage,
    ) -> Vec<C> {
        let request_id = query.request_id().to_string();
        let hydrators: Vec<_> = hydrators.iter().filter(|h| h.enable(query)).collect();
        let expected_len = candidates.len();
        let hydrate_futures = hydrators.iter().map(|h| h.hydrate(query, &candidates));
        let results = join_all(hydrate_futures).await;
        for (hydrator, result) in hydrators.iter().zip(results) {
            match result {
                Ok(hydrated) => {
                    if hydrated.len() == expected_len {
                        hydrator.update_all(&mut candidates, hydrated);
                    } else {
                        warn!(
                            "request_id={} stage={:?} component={} skipped: length_mismatch expected={} got={}",
                            request_id,
                            stage,
                            hydrator.name(),
                            expected_len,
                            hydrated.len()
                        );
                    }
                }
                Err(err) => {
                    error!(
                        "request_id={} stage={:?} component={} failed: {}",
                        request_id,
                        stage,
                        hydrator.name(),
                        err
                    );
                }
            }
        }
        candidates
    }

    /// Run all filters sequentially. Each filter partitions candidates into kept and removed.
    async fn filter(&self, query: &Q, candidates: Vec<C>) -> (Vec<C>, Vec<C>) {
        self.run_filters(query, candidates, self.filters(), PipelineStage::Filter)
            .await
    }

    /// Run post-scoring filters sequentially on already-scored candidates.
    async fn filter_post_selection(&self, query: &Q, candidates: Vec<C>) -> (Vec<C>, Vec<C>) {
        self.run_filters(
            query,
            candidates,
            self.post_selection_filters(),
            PipelineStage::PostSelectionFilter,
        )
        .await
    }

    // Shared helper to run filters sequentially from a provided filter list.
    async fn run_filters(
        &self,
        query: &Q,
        mut candidates: Vec<C>,
        filters: &[Box<dyn Filter<Q, C>>],
        stage: PipelineStage,
    ) -> (Vec<C>, Vec<C>) {
        let request_id = query.request_id().to_string();
        let mut all_removed = Vec::new();
        for filter in filters.iter().filter(|f| f.enable(query)) {
            let backup = candidates.clone();
            match filter.filter(query, candidates).await {
                Ok(result) => {
                    candidates = result.kept;
                    all_removed.extend(result.removed);
                }
                Err(err) => {
                    error!(
                        "request_id={} stage={:?} component={} failed: {}",
                        request_id,
                        stage,
                        filter.name(),
                        err
                    );
                    candidates = backup;
                }
            }
        }
        info!(
            "request_id={} stage={:?} kept {}, removed {}",
            request_id,
            stage,
            candidates.len(),
            all_removed.len()
        );
        (candidates, all_removed)
    }

    /// Run all scorers sequentially and apply their results to candidates.
    async fn score(&self, query: &Q, mut candidates: Vec<C>) -> Vec<C> {
        let request_id = query.request_id().to_string();
        let expected_len = candidates.len();
        for scorer in self.scorers().iter().filter(|s| s.enable(query)) {
            match scorer.score(query, &candidates).await {
                Ok(scored) => {
                    if scored.len() == expected_len {
                        scorer.update_all(&mut candidates, scored);
                    } else {
                        warn!(
                            "request_id={} stage={:?} component={} skipped: length_mismatch expected={} got={}",
                            request_id,
                            PipelineStage::Scorer,
                            scorer.name(),
                            expected_len,
                            scored.len()
                        );
                    }
                }
                Err(err) => {
                    error!(
                        "request_id={} stage={:?} component={} failed: {}",
                        request_id,
                        PipelineStage::Scorer,
                        scorer.name(),
                        err
                    );
                }
            }
        }
        candidates
    }

    /// Select (sort/truncate) candidates using the configured selector
    fn select(&self, query: &Q, candidates: Vec<C>) -> Vec<C> {
        if self.selector().enable(query) {
            self.selector().select(query, candidates)
        } else {
            candidates
        }
    }

    // Run all side effects in parallel
    fn run_side_effects(&self, input: Arc<SideEffectInput<Q, C>>) {
        let side_effects = self.side_effects();
        tokio::spawn(async move {
            let futures = side_effects
                .iter()
                .filter(|se| se.enable(input.query.clone()))
                .map(|se| se.run(input.clone()));
            let _ = join_all(futures).await;
        });
    }
}
