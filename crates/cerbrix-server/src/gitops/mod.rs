//! Configuration from Git: turning a snapshot of the configuration repository
//! into the effective configuration, or into a list of issues.

mod boards;
pub mod compile;
pub mod git;
pub mod sync;
#[cfg(test)]
mod tests;
pub mod tree;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use compile::{
    Compiled, CompiledHost, Document, DocumentKind, EffectiveHost, EffectiveItem, EffectiveTrigger,
    compile,
};
pub use git::{ConfigSource, GitSource, SourceError};
pub use sync::{SyncOutcome, run_sync_loop, sync_once};
pub use tree::{MAX_FILE_BYTES, MAX_FILES, MAX_TOTAL_BYTES, check_config, format_issue, read_tree};

/// The files of one revision of the configuration repository.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub commit: String,
    /// Repository-relative paths with `/` separators, for example `hosts/db/db-01.json`.
    pub files: BTreeMap<String, Vec<u8>>,
}

/// One reason a revision cannot be applied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncIssue {
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub reason: String,
}

impl SyncIssue {
    pub(crate) fn new(file: Option<&str>, reason: impl Into<String>) -> Self {
        SyncIssue {
            file: file.map(str::to_owned),
            line: None,
            column: None,
            reason: reason.into(),
        }
    }
}

/// A snapshot reports at most this many issues.
pub const MAX_ISSUES: usize = 50;
