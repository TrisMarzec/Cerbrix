//! Dashboard and display files.

use std::collections::BTreeMap;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value, json};

use super::Severity;
use crate::interval::Interval;

/// Number of grid columns of a dashboard.
pub const GRID_COLUMNS: u8 = 12;
/// Number of grid rows of a dashboard.
pub const GRID_ROWS: u8 = 6;

/// Selects the hosts a dashboard is about.
///
/// A host must be in all `groups`; `names` adds single hosts. With both empty
/// every host is selected.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct HostSelector {
    pub groups: Vec<String>,
    pub names: Vec<String>,
}

impl HostSelector {
    /// Whether the host with the given name and groups is selected.
    pub fn selects(&self, host: &str, host_groups: &[String]) -> bool {
        if self.groups.is_empty() && self.names.is_empty() {
            return true;
        }
        if self.names.iter().any(|n| n == host) {
            return true;
        }
        !self.groups.is_empty() && self.groups.iter().all(|g| host_groups.contains(g))
    }
}

/// Position and size of a panel in grid cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    pub x: u8,
    pub y: u8,
    pub w: u8,
    pub h: u8,
}

fn six_hours() -> Interval {
    "6h".parse().expect("6h is a valid interval")
}

/// A panel of a dashboard. Deserialization and the JSON Schema are written by
/// hand because `deny_unknown_fields` does not combine with `flatten`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Panel {
    Problems {
        #[serde(flatten)]
        at: Placement,
    },
    HostStates {
        #[serde(flatten)]
        at: Placement,
    },
    Graph {
        host: String,
        item: String,
        params: Option<BTreeMap<String, String>>,
        range: Interval,
        #[serde(flatten)]
        at: Placement,
    },
    Value {
        host: String,
        item: String,
        params: Option<BTreeMap<String, String>>,
        #[serde(flatten)]
        at: Placement,
    },
}

impl Panel {
    /// The panel's place on the grid.
    pub fn placement(&self) -> Placement {
        match self {
            Panel::Problems { at }
            | Panel::HostStates { at }
            | Panel::Graph { at, .. }
            | Panel::Value { at, .. } => *at,
        }
    }
}

fn take<T: serde::de::DeserializeOwned, E: serde::de::Error>(
    map: &mut Map<String, Value>,
    key: &'static str,
) -> Result<Option<T>, E> {
    match map.remove(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => serde_json::from_value(v)
            .map(Some)
            .map_err(|e| E::custom(format!("field `{key}`: {e}"))),
    }
}

fn require<T: serde::de::DeserializeOwned, E: serde::de::Error>(
    map: &mut Map<String, Value>,
    key: &'static str,
) -> Result<T, E> {
    take(map, key)?.ok_or_else(|| E::missing_field(key))
}

/// Reads the placement from what is left of the panel object; `Placement`
/// rejects any other key.
fn placement<E: serde::de::Error>(rest: Map<String, Value>) -> Result<Placement, E> {
    serde_json::from_value(Value::Object(rest)).map_err(|e| E::custom(e.to_string()))
}

impl<'de> Deserialize<'de> for Panel {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut map = Map::<String, Value>::deserialize(deserializer)?;
        let kind: String = require(&mut map, "type")?;
        let panel = match kind.as_str() {
            "problems" => Panel::Problems {
                at: placement(map)?,
            },
            "host-states" => Panel::HostStates {
                at: placement(map)?,
            },
            "graph" => Panel::Graph {
                host: require(&mut map, "host")?,
                item: require(&mut map, "item")?,
                params: take(&mut map, "params")?,
                range: match map.remove("range") {
                    None => six_hours(),
                    Some(v) => serde_json::from_value(v)
                        .map_err(|e| D::Error::custom(format!("field `range`: {e}")))?,
                },
                at: placement(map)?,
            },
            "value" => Panel::Value {
                host: require(&mut map, "host")?,
                item: require(&mut map, "item")?,
                params: take(&mut map, "params")?,
                at: placement(map)?,
            },
            other => {
                return Err(D::Error::custom(format!(
                    "unknown panel type `{other}`, expected one of `problems`, `host-states`, `graph`, `value`"
                )));
            }
        };
        Ok(panel)
    }
}

impl JsonSchema for Panel {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Panel".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let interval = generator.subschema_for::<Interval>().to_value();
        let cell = |min: u8, max: u8| json!({"type": "integer", "minimum": min, "maximum": max});
        let variant = |kind: &str, extra: Vec<(&str, Value)>, required: &[&str]| {
            let mut props = Map::new();
            props.insert("type".into(), json!({"const": kind}));
            for (k, v) in extra {
                props.insert(k.into(), v);
            }
            props.insert("x".into(), cell(0, GRID_COLUMNS - 1));
            props.insert("y".into(), cell(0, GRID_ROWS - 1));
            props.insert("w".into(), cell(1, GRID_COLUMNS));
            props.insert("h".into(), cell(1, GRID_ROWS));
            let mut req = vec!["type"];
            req.extend(required);
            req.extend(["x", "y", "w", "h"]);
            json!({
                "type": "object",
                "properties": props,
                "required": req,
                "additionalProperties": false,
            })
        };
        let host_item = || {
            vec![
                ("host", json!({"type": "string"})),
                ("item", json!({"type": "string"})),
                (
                    "params",
                    json!({"type": ["object", "null"], "additionalProperties": {"type": "string"}}),
                ),
            ]
        };
        let mut graph = host_item();
        graph.push(("range", interval));
        let value = json!({
            "description": "A dashboard panel. `type` selects the kind; x, y, w and h place it on the 12x6 grid.",
            "oneOf": [
                variant("problems", vec![], &[]),
                variant("host-states", vec![], &[]),
                variant("graph", graph, &["host", "item"]),
                variant("value", host_item(), &["host", "item"]),
            ],
        });
        Schema::try_from(value).expect("panel schema is an object")
    }
}

/// A file in `dashboards/`. The file name without `.json` is the dashboard id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DashboardFile {
    pub name: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub hosts: HostSelector,
    pub panels: Option<Vec<Panel>>,
}

impl DashboardFile {
    /// The configured panels, or the standard layout when `panels` is absent.
    pub fn effective_panels(&self) -> Vec<Panel> {
        match &self.panels {
            Some(p) => p.clone(),
            None => vec![
                Panel::Problems {
                    at: Placement {
                        x: 0,
                        y: 0,
                        w: 8,
                        h: GRID_ROWS,
                    },
                },
                Panel::HostStates {
                    at: Placement {
                        x: 8,
                        y: 0,
                        w: 4,
                        h: GRID_ROWS,
                    },
                },
            ],
        }
    }
}

/// What a display shows when no alarm switches it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
pub enum Show {
    Fixed {
        dashboard: String,
    },
    Rotate {
        rotate: Vec<String>,
        every: Interval,
    },
}

fn critical() -> Severity {
    Severity::Critical
}

/// Switching to a dashboard with an alarm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SwitchOnAlarm {
    #[serde(default = "critical")]
    pub min_severity: Severity,
}

/// A file in `displays/`. The file name without `.json` is the display id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DisplayFile {
    pub name: String,
    pub show: Show,
    pub switch_on_alarm: Option<SwitchOnAlarm>,
    pub dashboards: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const DASHBOARD: &str = r#"{
  "name": "Kunde A – Region 2",
  "priority": 20,
  "hosts": { "groups": ["kunde-a", "region-2"] },
  "panels": [
    { "type": "problems",    "x": 0, "y": 0, "w": 8, "h": 6 },
    { "type": "host-states", "x": 8, "y": 0, "w": 4, "h": 3 },
    { "type": "graph", "host": "db-02", "item": "disk.used_percent",
      "params": { "mount": "/" }, "range": "6h",
      "x": 8, "y": 3, "w": 4, "h": 2 },
    { "type": "value", "host": "web-01", "item": "load.1m",
      "x": 8, "y": 5, "w": 4, "h": 1 }
  ]
}"#;

    const DISPLAY: &str = r#"{
  "name": "NOC left",
  "show": { "rotate": ["kunde-a-region-1", "kunde-a-region-2", "kunde-b-region-1"], "every": "30s" },
  "switch_on_alarm": { "min_severity": "critical" }
}"#;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn dashboard_parses_concept_example() {
        let d: DashboardFile = serde_json::from_str(DASHBOARD).unwrap();
        assert_eq!(d.priority, 20);
        let panels = d.panels.unwrap();
        assert_eq!(panels.len(), 4);
        match &panels[2] {
            Panel::Graph { range, at, .. } => {
                assert_eq!(range.as_secs(), 6 * 3600);
                assert_eq!(
                    *at,
                    Placement {
                        x: 8,
                        y: 3,
                        w: 4,
                        h: 2
                    }
                );
            }
            other => panic!("expected graph, got {other:?}"),
        }
    }

    #[test]
    fn minimal_dashboard_gets_standard_layout() {
        let d: DashboardFile =
            serde_json::from_str(r#"{"name":"A","hosts":{"groups":["kunde-a"]}}"#).unwrap();
        assert_eq!(d.priority, 0);
        assert_eq!(
            d.effective_panels(),
            vec![
                Panel::Problems {
                    at: Placement {
                        x: 0,
                        y: 0,
                        w: 8,
                        h: 6
                    }
                },
                Panel::HostStates {
                    at: Placement {
                        x: 8,
                        y: 0,
                        w: 4,
                        h: 6
                    }
                },
            ]
        );
    }

    #[test]
    fn panel_rejects_unknown_type_and_field() {
        let e =
            serde_json::from_str::<Panel>(r#"{"type":"pie","x":0,"y":0,"w":1,"h":1}"#).unwrap_err();
        assert!(e.to_string().contains("pie"), "{e}");
        let e = serde_json::from_str::<Panel>(
            r#"{"type":"problems","x":0,"y":0,"w":1,"h":1,"colour":"red"}"#,
        )
        .unwrap_err();
        assert!(e.to_string().contains("colour"), "{e}");
    }

    #[test]
    fn panel_roundtrips_and_defaults_range() {
        let p: Panel = serde_json::from_str(
            r#"{"type":"graph","host":"h","item":"i","x":0,"y":0,"w":2,"h":2}"#,
        )
        .unwrap();
        assert!(matches!(&p, Panel::Graph { range, .. } if range.as_secs() == 21600));
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["type"], "graph");
        assert_eq!(v["w"], 2);
        assert_eq!(serde_json::from_value::<Panel>(v).unwrap(), p);
    }

    #[test]
    fn selector_requires_all_groups() {
        let s = HostSelector {
            groups: strings(&["kunde-a", "region-2"]),
            names: strings(&["db-02"]),
        };
        assert!(s.selects("web-01", &strings(&["kunde-a", "region-2", "x"])));
        assert!(!s.selects("web-01", &strings(&["kunde-a"])));
        assert!(s.selects("db-02", &[]));
        assert!(HostSelector::default().selects("anything", &[]));
        let names_only = HostSelector {
            groups: vec![],
            names: strings(&["db-02"]),
        };
        assert!(!names_only.selects("web-01", &strings(&["kunde-a"])));
    }

    #[test]
    fn display_parses_fixed_and_rotate() {
        let d: DisplayFile = serde_json::from_str(DISPLAY).unwrap();
        match d.show {
            Show::Rotate { rotate, every } => {
                assert_eq!(rotate.len(), 3);
                assert_eq!(every.as_secs(), 30);
            }
            other => panic!("expected rotate, got {other:?}"),
        }
        assert_eq!(d.switch_on_alarm.unwrap().min_severity, Severity::Critical);

        let d: DisplayFile =
            serde_json::from_str(r#"{"name":"x","show":{"dashboard":"a"}}"#).unwrap();
        assert_eq!(
            d.show,
            Show::Fixed {
                dashboard: "a".into()
            }
        );
        assert!(d.switch_on_alarm.is_none());
    }

    #[test]
    fn display_rejects_ambiguous_show() {
        let r = serde_json::from_str::<DisplayFile>(
            r#"{"name":"x","show":{"dashboard":"a","rotate":["b"],"every":"30s"}}"#,
        );
        assert!(r.is_err());
    }

    #[test]
    fn graph_panel_rejects_null_range() {
        let e = serde_json::from_str::<Panel>(
            r#"{"type":"graph","host":"h","item":"i","range":null,"x":0,"y":0,"w":2,"h":2}"#,
        )
        .unwrap_err();
        assert!(e.to_string().contains("range"), "{e}");
        // params stays nullable, as the schema declares.
        assert!(
            serde_json::from_str::<Panel>(
                r#"{"type":"value","host":"h","item":"i","params":null,"x":0,"y":0,"w":2,"h":2}"#
            )
            .is_ok()
        );
    }

    fn validator<T: JsonSchema>() -> jsonschema::Validator {
        let schema = serde_json::to_value(schemars::schema_for!(T)).unwrap();
        jsonschema::validator_for(&schema).unwrap()
    }

    #[test]
    fn schema_and_parser_agree() {
        let dash = validator::<DashboardFile>();
        let disp = validator::<DisplayFile>();
        let parse_dash = |s: &str| serde_json::from_str::<DashboardFile>(s).is_ok();
        let parse_disp = |s: &str| serde_json::from_str::<DisplayFile>(s).is_ok();
        let json = |s: &str| serde_json::from_str::<Value>(s).unwrap();

        for s in [DASHBOARD, r#"{"name":"A","hosts":{"groups":["kunde-a"]}}"#] {
            assert!(dash.is_valid(&json(s)), "schema rejects {s}");
            assert!(parse_dash(s), "parser rejects {s}");
        }
        for s in [DISPLAY, r#"{"name":"x","show":{"dashboard":"a"}}"#] {
            assert!(disp.is_valid(&json(s)), "schema rejects {s}");
            assert!(parse_disp(s), "parser rejects {s}");
        }

        let dash_with = |panel: &str| format!(r#"{{"name":"A","panels":[{panel}]}}"#);
        let bad_panels = [
            r#"{"type":"problems","x":0,"y":0,"w":1,"h":1,"colour":"red"}"#,
            r#"{"type":"pie","x":0,"y":0,"w":1,"h":1}"#,
            r#"{"type":"graph","item":"i","x":0,"y":0,"w":1,"h":1}"#,
            r#"{"type":"graph","host":"h","item":"i","range":null,"x":0,"y":0,"w":1,"h":1}"#,
            r#"{"type":"graph","host":"h","item":"i","range":"1 hour","x":0,"y":0,"w":1,"h":1}"#,
        ];
        for p in bad_panels {
            let s = dash_with(p);
            assert!(!dash.is_valid(&json(&s)), "schema accepts {p}");
            assert!(!parse_dash(&s), "parser accepts {p}");
        }
        let good = dash_with(
            r#"{"type":"graph","host":"h","item":"i","range":"1h","x":0,"y":0,"w":1,"h":1}"#,
        );
        assert!(dash.is_valid(&json(&good)));
        assert!(parse_dash(&good));

        let bad_shows = [
            r#"{"name":"x","show":{"dashboard":"a","rotate":["b"],"every":"30s"}}"#,
            r#"{"name":"x","show":{"dashbord":"a"}}"#,
        ];
        for s in bad_shows {
            assert!(!disp.is_valid(&json(s)), "schema accepts {s}");
            assert!(!parse_disp(s), "parser accepts {s}");
        }
    }
}
