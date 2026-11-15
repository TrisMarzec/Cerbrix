//! Wire types for the configuration the agent fetches from the server.
//!
//! Named `ItemConfig` because the agent crate already has an `AgentConfig` (its local file).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One item the agent collects, with its interval in seconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ConfiguredItem {
    pub key: String,
    pub params: Option<BTreeMap<String, String>>,
    pub interval_secs: u64,
}

/// The agent's configuration: a version string and the items to collect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ItemConfig {
    pub version: String,
    pub items: Vec<ConfiguredItem>,
}
