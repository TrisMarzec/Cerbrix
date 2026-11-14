//! The collector abstraction: something that measures a set of items.

use std::collections::BTreeMap;

/// One raw reading produced by a collector. The scheduler adds the timestamp.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    /// Item key, e.g. `disk.used_percent`.
    pub key: &'static str,
    /// Item parameters, e.g. `{"mount": "/"}`.
    pub params: BTreeMap<String, String>,
    pub value: f64,
}

/// A source of readings. `collect` may block; the scheduler runs it on the
/// blocking thread pool.
pub trait Collector: Send {
    /// Short name used in log messages.
    fn name(&self) -> &'static str;
    /// Takes one round of measurements.
    fn collect(&mut self) -> anyhow::Result<Vec<Reading>>;
}
