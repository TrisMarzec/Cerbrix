//! Glob matching for the exclusion lists of the disk and network collectors.

use anyhow::bail;

/// A set of glob patterns. `*` matches any run of characters (including `/`
/// and the empty run), `?` matches exactly one character. Matching is
/// anchored at both ends and case-sensitive; there are no other metacharacters.
#[derive(Debug, Clone, Default)]
pub struct Patterns(Vec<Vec<char>>);

impl Patterns {
    /// Compiles `globs`. An empty pattern string is rejected: it could only
    /// ever match the empty string and is almost certainly a configuration
    /// mistake.
    pub fn new(globs: &[String]) -> anyhow::Result<Self> {
        let mut compiled = Vec::with_capacity(globs.len());
        for glob in globs {
            if glob.is_empty() {
                bail!("empty pattern");
            }
            compiled.push(glob.chars().collect());
        }
        Ok(Self(compiled))
    }

    /// True if `s` matches at least one pattern.
    pub fn matches(&self, s: &str) -> bool {
        let text: Vec<char> = s.chars().collect();
        self.0.iter().any(|p| glob_match(p, &text))
    }
}

/// Iterative glob match that remembers the last `*` so it runs in
/// O(pattern * text) without recursion.
fn glob_match(pattern: &[char], text: &[char]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p) {
            Some('*') => {
                backtrack = Some((p, t));
                p += 1;
            }
            Some(&c) if c == '?' || c == text[t] => {
                p += 1;
                t += 1;
            }
            _ => match backtrack {
                Some((star_p, star_t)) => {
                    backtrack = Some((star_p, star_t + 1));
                    p = star_p + 1;
                    t = star_t + 1;
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

/// The parameter-value rule of the wire format: 1 to 256 bytes without control
/// characters (mirrors `cerbrix_common::MetricValue::validate`).
pub(crate) fn is_valid_param_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= cerbrix_common::MAX_PARAM_VALUE_LEN
        && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patterns(globs: &[&str]) -> Patterns {
        Patterns::new(&globs.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn patterns_match_wildcards() {
        let p = patterns(&["veth*"]);
        assert!(p.matches("veth1"));
        assert!(p.matches("veth"));
        assert!(!p.matches("eth0"));
        let p = patterns(&["eth?"]);
        assert!(p.matches("eth0"));
        assert!(!p.matches("eth10"));
        assert!(!p.matches("eth"));
    }

    #[test]
    fn star_spans_slashes_and_is_anchored_and_case_sensitive() {
        let p = patterns(&["/var/lib/docker/*"]);
        assert!(p.matches("/var/lib/docker/overlay2/x"));
        assert!(!p.matches("/var/lib/docker"));
        assert!(!p.matches("/x/var/lib/docker/a"));
        assert!(!patterns(&["Eth0"]).matches("eth0"));
        assert!(patterns(&["a*b*c"]).matches("aXXbYYc"));
        assert!(!patterns(&["a*b*c"]).matches("aXXbYY"));
        assert!(patterns(&["*"]).matches(""));
        assert!(!patterns(&["."]).matches("x"));
    }

    #[test]
    fn empty_pattern_is_rejected_and_empty_set_matches_nothing() {
        assert!(Patterns::new(&["".to_string()]).is_err());
        assert!(!Patterns::default().matches("anything"));
    }

    #[test]
    fn param_value_rule() {
        assert!(is_valid_param_value("/"));
        assert!(!is_valid_param_value(""));
        assert!(!is_valid_param_value("a\nb"));
        assert!(is_valid_param_value(&"x".repeat(256)));
        assert!(!is_valid_param_value(&"x".repeat(257)));
    }
}
