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
    compile, item_config,
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

/// Collects the issues of one compile, at most [`MAX_ISSUES`] of them. Once it is
/// full, further issues are dropped without being built, so a hostile input
/// cannot make the issue list (or its text) grow with the size of the input.
#[derive(Debug, Default)]
pub(crate) struct Issues {
    list: Vec<SyncIssue>,
}

impl Issues {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn is_full(&self) -> bool {
        self.list.len() >= MAX_ISSUES
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Adds an issue about `file`; `reason` is only called while there is room.
    pub(crate) fn add(&mut self, file: Option<&str>, reason: impl FnOnce() -> String) {
        if !self.is_full() {
            self.list.push(SyncIssue::new(file, reason()));
        }
    }

    /// Adds a prepared issue; `issue` is only called while there is room.
    pub(crate) fn add_issue(&mut self, issue: impl FnOnce() -> SyncIssue) {
        if !self.is_full() {
            self.list.push(issue());
        }
    }

    pub(crate) fn into_vec(self) -> Vec<SyncIssue> {
        self.list
    }
}

/// Removes terminal escape sequences (ESC `[` ... final byte, ESC `]` ... BEL or
/// ESC `\`, ESC and one more character) and then every other control character.
pub(crate) fn strip_control(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.next() {
                // CSI: parameter and intermediate bytes, then one final byte.
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c) {
                            break;
                        }
                    }
                }
                // OSC: up to BEL or the string terminator ESC `\`.
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{07}' {
                            break;
                        }
                        if c == '\u{1b}' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if !c.is_control() {
            out.push(c);
        }
    }
    out
}

/// Replaces every control character by `?`, keeping the length of the text visible.
pub(crate) fn replace_control(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}
