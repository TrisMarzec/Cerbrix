//! A collector wrapper that passes on only the readings the configuration asks for.

use std::collections::BTreeMap;

use crate::collector::{Collector, Reading};
use crate::remote_config::Wanted;

/// Wraps a collector and keeps only the readings some [`Wanted`] entry selects.
pub struct FilteredCollector {
    inner: Box<dyn Collector>,
    wanted: Vec<Wanted>,
}

impl FilteredCollector {
    pub fn new(inner: Box<dyn Collector>, wanted: Vec<Wanted>) -> Self {
        Self { inner, wanted }
    }
}

fn selects(w: &Wanted, key: &str, params: &BTreeMap<String, String>) -> bool {
    w.key == key
        && match &w.params {
            Some(p) => p == params,
            None => !w.except.contains(params),
        }
}

impl Collector for FilteredCollector {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn collect(&mut self) -> anyhow::Result<Vec<Reading>> {
        let mut readings = self.inner.collect()?;
        readings.retain(|r| self.wanted.iter().any(|w| selects(w, r.key, &r.params)));
        Ok(readings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(Vec<Reading>, bool);

    impl Collector for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn collect(&mut self) -> anyhow::Result<Vec<Reading>> {
            if self.1 {
                anyhow::bail!("boom");
            }
            Ok(self.0.clone())
        }
    }

    fn reading(key: &'static str, mount: Option<&str>) -> Reading {
        Reading {
            key,
            params: mount
                .map(|m| BTreeMap::from([("mount".to_owned(), m.to_owned())]))
                .unwrap_or_default(),
            value: 1.0,
        }
    }

    fn wanted(key: &str, mount: Option<&str>, except: &[&str]) -> Wanted {
        Wanted {
            key: key.to_owned(),
            params: mount.map(|m| BTreeMap::from([("mount".to_owned(), m.to_owned())])),
            except: except
                .iter()
                .map(|m| BTreeMap::from([("mount".to_owned(), (*m).to_owned())]))
                .collect(),
        }
    }

    #[test]
    fn filter_keeps_only_wanted_keys() {
        let inner = Fake(
            vec![
                reading("memory.used_percent", None),
                reading("memory.total_bytes", None),
            ],
            false,
        );
        let mut f = FilteredCollector::new(
            Box::new(inner),
            vec![wanted("memory.used_percent", None, &[])],
        );
        assert_eq!(f.name(), "fake");
        let out = f.collect().unwrap();
        assert_eq!(out, vec![reading("memory.used_percent", None)]);
    }

    #[test]
    fn filter_applies_params_and_except() {
        let readings = || {
            vec![
                reading("disk.used_percent", Some("/")),
                reading("disk.used_percent", Some("/data")),
            ]
        };
        let mut general = FilteredCollector::new(
            Box::new(Fake(readings(), false)),
            vec![wanted("disk.used_percent", None, &["/"])],
        );
        assert_eq!(
            general.collect().unwrap(),
            vec![reading("disk.used_percent", Some("/data"))]
        );
        let mut specific = FilteredCollector::new(
            Box::new(Fake(readings(), false)),
            vec![wanted("disk.used_percent", Some("/"), &[])],
        );
        assert_eq!(
            specific.collect().unwrap(),
            vec![reading("disk.used_percent", Some("/"))]
        );
    }

    #[test]
    fn filter_forwards_inner_error() {
        let mut f = FilteredCollector::new(Box::new(Fake(vec![], true)), vec![]);
        assert_eq!(f.collect().unwrap_err().to_string(), "boom");
    }
}
