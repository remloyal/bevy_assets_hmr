//! Lightweight HMR runtime metrics.

use crate::core::HmrSource;
use crate::diff::ConfigDiff;
use bevy::prelude::Resource;
use std::marker::PhantomData;
use std::time::Duration;

/// Per-asset-type counters for diagnosing refresh cost and clone pressure.
///
/// The counters are intentionally passive: the HMR systems update them, but
/// applications decide whether and how often to report or reset them.
#[derive(Resource)]
pub struct HmrMetrics<A: HmrSource> {
    pub diff_runs: u64,
    pub total_diff_time_ns: u128,
    pub max_diff_time_ns: u128,
    pub refreshes_dispatched: u64,
    pub recoveries_dispatched: u64,
    pub failures_reported: u64,
    pub removals_dispatched: u64,
    pub cascades_dispatched: u64,
    pub config_clone_count: u64,
    pub estimated_clone_bytes: u64,
    pub unestimated_clone_count: u64,
    size_estimator: Option<fn(&A::Config) -> usize>,
    _marker: PhantomData<fn() -> A>,
}

impl<A: HmrSource> Default for HmrMetrics<A> {
    fn default() -> Self {
        Self {
            diff_runs: 0,
            total_diff_time_ns: 0,
            max_diff_time_ns: 0,
            refreshes_dispatched: 0,
            recoveries_dispatched: 0,
            failures_reported: 0,
            removals_dispatched: 0,
            cascades_dispatched: 0,
            config_clone_count: 0,
            estimated_clone_bytes: 0,
            unestimated_clone_count: 0,
            size_estimator: None,
            _marker: PhantomData,
        }
    }
}

impl<A: HmrSource> HmrMetrics<A> {
    /// Install a cheap owned-size estimator used for clone byte accounting.
    pub fn set_size_estimator(&mut self, estimator: fn(&A::Config) -> usize) {
        self.size_estimator = Some(estimator);
    }

    pub(crate) fn record_diff(&mut self, elapsed: Duration) {
        let nanos = elapsed.as_nanos();
        self.diff_runs += 1;
        self.total_diff_time_ns += nanos;
        self.max_diff_time_ns = self.max_diff_time_ns.max(nanos);
    }

    pub(crate) fn record_config_clone(&mut self, config: &A::Config) {
        self.config_clone_count += 1;
        let estimated_bytes = self
            .size_estimator
            .map(|estimate| estimate(config))
            .or_else(|| config.estimated_size_bytes());
        if let Some(bytes) = estimated_bytes {
            self.estimated_clone_bytes = self.estimated_clone_bytes.saturating_add(bytes as u64);
        } else {
            self.unestimated_clone_count += 1;
        }
    }
}
