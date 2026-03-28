use std::any::{Any, type_name_of_val};
use tonic::async_trait;

use crate::util;

pub struct FilterResult<C> {
    pub kept: Vec<C>,
    pub removed: Vec<C>,
}

/// Filters run sequentially and partition candidates into kept and removed sets
#[async_trait]
pub trait Filter<Q, C>: Any + Send + Sync
where
    Q: Clone + Send + Sync + 'static,
    C: Clone + Send + Sync + 'static,
{
    /// Decide if this filter should run for the given query
    fn enable(&self, _query: &Q) -> bool {
        true
    }

    /// Filter candidates by evaluating each against some criteria.
    /// Returns a FilterResult containing kept candidates (which continue to the next stage)
    /// and removed candidates (which are excluded from further processing).
    async fn filter(&self, query: &Q, candidates: Vec<C>) -> Result<FilterResult<C>, String>;

    /// Returns a stable name for logging/metrics.
    fn name(&self) -> &'static str {
        util::short_type_name(type_name_of_val(self))
    }
}
