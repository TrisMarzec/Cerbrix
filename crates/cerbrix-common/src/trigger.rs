//! Parser for trigger expressions such as `disk.used_percent{mount=/} > 90 for 10m`.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

/// An item key with optional parameter filters.
#[derive(Debug, Clone, PartialEq)]
pub struct Selector {
    pub key: String,
    pub params: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comparison {
    Gt,
    Lt,
    Ge,
    Le,
    Eq,
    Ne,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TriggerExpr {
    Compare {
        selector: Selector,
        op: Comparison,
        threshold: f64,
        hold: Option<Duration>,
    },
    NoData {
        selector: Option<Selector>,
        window: Duration,
    },
}

/// A syntax error; `column` is 1-based and counts characters.
#[derive(Debug, Clone, PartialEq)]
pub struct TriggerParseError {
    pub column: usize,
    pub message: String,
}

impl fmt::Display for TriggerParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "column {}: {}", self.column, self.message)
    }
}

impl std::error::Error for TriggerParseError {}

use crate::interval::Interval;
use crate::metric::{validate_item_key, validate_param_name};

pub fn parse_trigger(input: &str) -> Result<TriggerExpr, TriggerParseError> {
    let mut p = Parser {
        chars: input.chars().collect(),
        pos: 0,
    };
    p.expr()
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

type PResult<T> = Result<T, TriggerParseError>;

impl Parser {
    /// Error at the character index `at` (0-based), reported 1-based.
    fn err_at<T>(&self, at: usize, message: impl Into<String>) -> PResult<T> {
        Err(TriggerParseError {
            column: at + 1,
            message: message.into(),
        })
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.pos += 1;
        }
    }

    fn take_while(&mut self, f: impl Fn(char) -> bool) -> (usize, String) {
        let start = self.pos;
        while self.peek().is_some_and(&f) {
            self.pos += 1;
        }
        (start, self.chars[start..self.pos].iter().collect())
    }

    /// Characters that can occur in an item-key token; validity is checked afterwards.
    fn is_key_char(c: char) -> bool {
        c.is_ascii_alphanumeric() || c == '_' || c == '.'
    }

    /// The run of key characters at the cursor, without consuming it.
    fn peek_word(&self) -> String {
        self.chars[self.pos..]
            .iter()
            .take_while(|c| Self::is_key_char(**c))
            .collect()
    }

    fn expr(&mut self) -> PResult<TriggerExpr> {
        self.skip_ws();
        let selector = if self.at_no_data() {
            None
        } else {
            let selector = self.selector()?;
            self.skip_ws();
            if !self.at_no_data() {
                return self.compare(selector);
            }
            Some(selector)
        };
        self.no_data(selector)
    }

    /// True when the cursor is at the keywords `no data`.
    fn at_no_data(&self) -> bool {
        if self.peek_word() != "no" {
            return false;
        }
        let word: String = self.chars[self.pos + 2..]
            .iter()
            .skip_while(|c| c.is_whitespace())
            .take_while(|c| Self::is_key_char(**c))
            .collect();
        word == "data"
    }

    fn keyword(&mut self, word: &str) -> PResult<()> {
        self.skip_ws();
        let at = self.pos;
        if self.peek_word() == word {
            self.pos += word.len();
            Ok(())
        } else {
            self.err_at(at, format!("expected `{word}`"))
        }
    }

    fn no_data(&mut self, selector: Option<Selector>) -> PResult<TriggerExpr> {
        self.keyword("no")?;
        self.keyword("data")?;
        self.keyword("for")?;
        let window = self.duration()?;
        self.end()?;
        Ok(TriggerExpr::NoData { selector, window })
    }

    fn compare(&mut self, selector: Selector) -> PResult<TriggerExpr> {
        let op = self.operator()?;
        let threshold = self.number()?;
        self.skip_ws();
        let hold = if self.pos < self.chars.len() {
            if self.peek_word() != "for" {
                return self.err_at(self.pos, "unexpected input, expected `for` or end");
            }
            self.pos += 3;
            Some(self.duration()?)
        } else {
            None
        };
        self.end()?;
        Ok(TriggerExpr::Compare {
            selector,
            op,
            threshold,
            hold,
        })
    }

    fn end(&mut self) -> PResult<()> {
        self.skip_ws();
        if self.pos < self.chars.len() {
            return self.err_at(self.pos, "unexpected input after end of expression");
        }
        Ok(())
    }

    fn selector(&mut self) -> PResult<Selector> {
        let (start, key) = self.take_while(Self::is_key_char);
        if key.is_empty() {
            return self.err_at(start, "expected an item key");
        }
        if validate_item_key(&key).is_err() {
            return self.err_at(start, format!("invalid item key `{key}`"));
        }
        let mut params = BTreeMap::new();
        if self.peek() == Some('{') {
            self.pos += 1;
            loop {
                self.skip_ws();
                let (name_at, name) = self.take_while(|c| c.is_ascii_alphanumeric() || c == '_');
                if validate_param_name(&name).is_err() {
                    return self.err_at(name_at, "expected a parameter name");
                }
                if params.contains_key(&name) {
                    return self.err_at(name_at, format!("duplicate parameter `{name}`"));
                }
                self.skip_ws();
                if self.peek() != Some('=') {
                    return self.err_at(self.pos, "expected `=`");
                }
                self.pos += 1;
                self.skip_ws();
                let value = self.param_value()?;
                params.insert(name, value);
                self.skip_ws();
                match self.peek() {
                    Some(',') => self.pos += 1,
                    Some('}') => {
                        self.pos += 1;
                        break;
                    }
                    _ => return self.err_at(self.pos, "expected `,` or `}`"),
                }
            }
        }
        Ok(Selector { key, params })
    }

    fn param_value(&mut self) -> PResult<String> {
        if self.peek() == Some('"') {
            let open = self.pos;
            self.pos += 1;
            let (_, value) = self.take_while(|c| c != '"');
            if self.peek() != Some('"') {
                return self.err_at(open, "unterminated quoted value");
            }
            self.pos += 1;
            Ok(value)
        } else {
            let (start, value) =
                self.take_while(|c| !matches!(c, ',' | '}' | '"') && !c.is_whitespace());
            if value.is_empty() {
                return self.err_at(start, "expected a parameter value");
            }
            Ok(value)
        }
    }

    fn operator(&mut self) -> PResult<Comparison> {
        self.skip_ws();
        let first = self.peek();
        let second = self.chars.get(self.pos + 1).copied();
        let (op, len) = match (first, second) {
            (Some('>'), Some('=')) => (Comparison::Ge, 2),
            (Some('<'), Some('=')) => (Comparison::Le, 2),
            (Some('='), Some('=')) => (Comparison::Eq, 2),
            (Some('!'), Some('=')) => (Comparison::Ne, 2),
            (Some('>'), _) => (Comparison::Gt, 1),
            (Some('<'), _) => (Comparison::Lt, 1),
            _ => {
                return self.err_at(
                    self.pos,
                    "expected a comparison operator (>, <, >=, <=, ==, !=) or `no data`",
                );
            }
        };
        self.pos += len;
        Ok(op)
    }

    fn number(&mut self) -> PResult<f64> {
        self.skip_ws();
        let start = self.pos;
        if self.peek() == Some('-') {
            self.pos += 1;
        }
        if self.take_while(|c| c.is_ascii_digit()).1.is_empty() {
            return self.err_at(start, "expected a number");
        }
        if self.peek() == Some('.') {
            self.pos += 1;
            if self.take_while(|c| c.is_ascii_digit()).1.is_empty() {
                return self.err_at(self.pos, "expected digits after `.`");
            }
        }
        if self.peek().is_some_and(|c| c.is_alphanumeric() || c == '.') {
            return self.err_at(self.pos, "unexpected character in number");
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        match text.parse::<f64>() {
            Ok(n) if n.is_finite() => Ok(n),
            _ => self.err_at(start, "number is not finite"),
        }
    }

    fn duration(&mut self) -> PResult<Duration> {
        self.skip_ws();
        let (start, token) = self.take_while(|c| c.is_ascii_alphanumeric());
        if token.is_empty() {
            return self.err_at(start, "expected a duration such as `10m`");
        }
        match token.parse::<Interval>() {
            Ok(i) => Ok(Duration::from_secs(i.as_secs())),
            Err(e) => self.err_at(start, format!("invalid duration `{token}`: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(key: &str, params: &[(&str, &str)]) -> Selector {
        Selector {
            key: key.to_owned(),
            params: params
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        }
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn parses_spec_example() {
        assert_eq!(
            parse_trigger("disk.used_percent{mount=/} > 90 for 10m").unwrap(),
            TriggerExpr::Compare {
                selector: sel("disk.used_percent", &[("mount", "/")]),
                op: Comparison::Gt,
                threshold: 90.0,
                hold: Some(secs(600)),
            }
        );
    }

    #[test]
    fn parses_quoted_param() {
        assert_eq!(
            parse_trigger(r#"disk.used_percent{mount="/var/lib/my data"} >= 95"#).unwrap(),
            TriggerExpr::Compare {
                selector: sel("disk.used_percent", &[("mount", "/var/lib/my data")]),
                op: Comparison::Ge,
                threshold: 95.0,
                hold: None,
            }
        );
    }

    #[test]
    fn parses_multiple_params() {
        let expr = parse_trigger("net.in{iface=eth0, dir = \"rx\"} > 1").unwrap();
        let TriggerExpr::Compare { selector, .. } = expr else {
            panic!("expected Compare");
        };
        assert_eq!(selector, sel("net.in", &[("iface", "eth0"), ("dir", "rx")]));
    }

    #[test]
    fn parses_every_operator() {
        for (text, op) in [
            (">", Comparison::Gt),
            ("<", Comparison::Lt),
            (">=", Comparison::Ge),
            ("<=", Comparison::Le),
            ("==", Comparison::Eq),
            ("!=", Comparison::Ne),
        ] {
            for input in [format!("cpu.usage {text} 1"), format!("cpu.usage{text}1")] {
                assert_eq!(
                    parse_trigger(&input).unwrap(),
                    TriggerExpr::Compare {
                        selector: sel("cpu.usage", &[]),
                        op,
                        threshold: 1.0,
                        hold: None,
                    },
                    "{input}"
                );
            }
        }
    }

    #[test]
    fn parses_negative_and_decimal() {
        let TriggerExpr::Compare { threshold, .. } = parse_trigger("system.load1 > -0.5").unwrap()
        else {
            panic!("expected Compare");
        };
        assert_eq!(threshold, -0.5);
    }

    #[test]
    fn parses_host_nodata() {
        assert_eq!(
            parse_trigger("no data for 3m").unwrap(),
            TriggerExpr::NoData {
                selector: None,
                window: secs(180)
            }
        );
    }

    #[test]
    fn parses_item_nodata() {
        assert_eq!(
            parse_trigger("cpu.usage no data for 5m").unwrap(),
            TriggerExpr::NoData {
                selector: Some(sel("cpu.usage", &[])),
                window: secs(300)
            }
        );
        assert_eq!(
            parse_trigger("disk.used_percent{mount=/} no data for 1h").unwrap(),
            TriggerExpr::NoData {
                selector: Some(sel("disk.used_percent", &[("mount", "/")])),
                window: secs(3600)
            }
        );
    }

    #[test]
    fn rejects_with_column() {
        let cases: &[(&str, usize)] = &[
            ("cpu.usage >> 90", 12),
            ("cpu.usage > ninety", 13),
            (
                "cpu.usage > 90 for",
                "cpu.usage > 90 for".chars().count() + 1,
            ),
            ("cpu.usage{mount=} > 1", 17),
            ("cpu.usage > 90 for 0s", 20),
            ("cpu.usage > 90 extra", 16),
            ("", 1),
            ("cpu.usage > NaN", 13),
            ("cpu.usage > 1e400", 14),
            ("cpu.usage{mount=a", 18),
            ("cpu.usage{mount=\"a} > 1", 17),
            ("cpu.usage{mount=a,mount=b} > 1", 19),
            ("cpu.usage{} > 1", 11),
            ("no data for", 12),
            ("cpu.usage", 10),
        ];
        for (input, column) in cases {
            let err = parse_trigger(input).expect_err(input);
            assert_eq!(err.column, *column, "{input:?}: {err}");
            assert!(!err.message.is_empty());
        }
    }

    #[test]
    fn columns_count_characters_not_bytes() {
        let err = parse_trigger("cpu.usage{mount=\"é\"} >> 1").unwrap_err();
        assert_eq!(err.column, 23);
    }

    #[test]
    fn rejects_invalid_key_and_param_name() {
        assert_eq!(parse_trigger("CPU.usage > 1").unwrap_err().column, 1);
        assert_eq!(parse_trigger("cpu.usage{1x=a} > 1").unwrap_err().column, 11);
    }

    #[test]
    fn display_shows_column_and_message() {
        let err = parse_trigger("cpu.usage >> 90").unwrap_err();
        assert!(err.to_string().starts_with("column 12: "));
    }
}
