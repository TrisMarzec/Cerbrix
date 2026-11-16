//! Alerting: trigger evaluation, problems and notifications.

use std::collections::BTreeMap;

use cerbrix_common::config::Severity;
use cerbrix_common::{TriggerExpr, parse_trigger};

use crate::gitops::EffectiveHost;

pub mod channel;
pub mod dispatch;
pub mod eval;
pub mod message;
pub mod nodata;
pub mod rules;

pub use message::{EventKind, Notification};
pub use rules::recipients;

/// Parameters of an item instance, e.g. `{"mount": "/"}`.
pub type Params = BTreeMap<String, String>;

/// Canonical text of an instance: "" for no parameters, otherwise `k=v` pairs
/// sorted by key and joined with ",", e.g. `mount=/`.
pub fn instance_key(params: &Params) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// A trigger of a host with its expression parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveTrigger {
    pub name: String,
    pub severity: Severity,
    pub expression: String,
    pub expr: TriggerExpr,
}

/// Parses the host's triggers; one that does not parse is skipped and logged at error level.
pub fn active_triggers(host: &EffectiveHost) -> Vec<ActiveTrigger> {
    host.triggers
        .iter()
        .filter_map(|t| match parse_trigger(&t.expression) {
            Ok(expr) => Some(ActiveTrigger {
                name: t.name.clone(),
                severity: t.severity,
                expression: t.expression.clone(),
                expr,
            }),
            Err(e) => {
                tracing::error!(
                    trigger = %t.name,
                    expression = %t.expression,
                    error = %e,
                    "skipping trigger with an unparsable expression"
                );
                None
            }
        })
        .collect()
}

/// Stored evaluation state of one (trigger, instance).
#[derive(Debug, Clone, PartialEq)]
pub struct TriggerState {
    pub trigger: String,
    pub instance: String,
    pub firing: bool,
    pub condition_since_ms: Option<i64>,
    pub last_ts_ms: i64,
    pub last_value: Option<f64>,
}

/// A problem that opens.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenProblem {
    pub trigger: String,
    pub instance: String,
    pub severity: Severity,
    pub at_ms: i64,
    pub summary: String,
}

/// A problem that resolves.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolveProblem {
    pub trigger: String,
    pub instance: String,
    pub at_ms: i64,
}

/// The outcome of one evaluation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Evaluation {
    /// New state of every (trigger, instance) that received a usable value.
    pub states: Vec<TriggerState>,
    pub open: Vec<OpenProblem>,
    pub resolve: Vec<ResolveProblem>,
    /// Values older than the tolerance (counted once each, watched by a trigger or not).
    pub too_old: usize,
}
