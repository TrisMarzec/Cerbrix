//! The dashboards: the list of the start page and everything one dashboard shows.
//!
//! Wall monitors call the detail every few seconds, so it costs three queries to PostgreSQL
//! (the dashboard, its hosts, their open problems) and one more for the configuration of the
//! hosts that graphs name, plus one call to the metrics store per graph or value panel. Those
//! calls run at the same time, each with its own time limit, and none of them can fail the
//! request: a panel whose data cannot be read has null data and `metrics_available` is false.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use cerbrix_common::{Interval, Panel, Placement, validate_name};
use futures::future::join_all;
use serde::Serialize;
use time::OffsetDateTime;

use super::{ApiError, Entry};
use crate::api::AppState;
use crate::auth::Viewer;
use crate::gitops::EffectiveHost;
use crate::metrics::{LatestQuery, MetricsStore};
use crate::state::{HostRow, Problem, StateError};
use crate::view::dashboard::{
    DashboardSummary, HostEntry, PROBLEMS_PANEL_LIMIT, host_entries, open_problems, summaries,
};
use crate::view::series::{
    CURRENT_VALUE_MAX_AGE_SECS, Threshold, bounded, log_metrics_failure, ms_to_rfc3339,
    names_one_series, range_query, step_for, thresholds,
};

pub(super) fn entries() -> Vec<Entry> {
    vec![
        Entry::new("GET", "/api/v1/dashboards", list),
        Entry::new("GET", "/api/v1/dashboards/{id}", detail),
    ]
}

/// How long one graph or value panel waits for the metrics store.
const PANEL_TIMEOUT: Duration = Duration::from_secs(5);

fn store_failed(what: &str, e: &StateError) -> ApiError {
    tracing::error!(error = %e, "{what}");
    ApiError::internal()
}

fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not_found", "no such dashboard")
}

#[derive(Serialize)]
struct Summaries {
    items: Vec<DashboardSummary>,
}

/// All dashboards with their state, for the start page.
async fn list(State(app): State<AppState>, Viewer(_): Viewer) -> Result<Json<Summaries>, ApiError> {
    let inputs = app
        .store
        .board_inputs()
        .await
        .map_err(|e| store_failed("listing dashboards failed", &e))?;
    Ok(Json(Summaries {
        items: summaries(&inputs),
    }))
}

/// A panel of the answer: its place and its data. Data that could not be read is `null`.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum PanelOut<'a> {
    Problems {
        #[serde(flatten)]
        place: Placement,
        problems: &'a [&'a Problem],
        /// How many open problems are not in `problems`.
        more: usize,
    },
    HostStates {
        #[serde(flatten)]
        place: Placement,
        total: usize,
        without_problems: usize,
        hosts: &'a [HostEntry<'a>],
    },
    Graph {
        #[serde(flatten)]
        place: Placement,
        host: &'a str,
        item: &'a str,
        params: BTreeMap<String, String>,
        range: Interval,
        step_secs: u32,
        /// `[ts_ms, value]` pairs in time order.
        points: Option<Vec<(i64, f64)>>,
        thresholds: Vec<Threshold>,
    },
    Value {
        #[serde(flatten)]
        place: Placement,
        host: &'a str,
        item: &'a str,
        params: BTreeMap<String, String>,
        value: Option<f64>,
        /// When the value was measured, RFC 3339.
        at: Option<String>,
    },
}

#[derive(Serialize)]
struct DetailOut<'a> {
    id: &'a str,
    name: &'a str,
    file: &'a str,
    priority: i32,
    hosts: &'a [HostEntry<'a>],
    panels: Vec<PanelOut<'a>>,
    /// False when the data of a panel could not be read from the metrics store.
    metrics_available: bool,
}

/// What every panel of one request is made from.
struct Context<'a> {
    metrics: &'a dyn MetricsStore,
    now: OffsetDateTime,
    /// The selected hosts.
    hosts: &'a [&'a HostRow],
    entries: &'a [HostEntry<'a>],
    problems: &'a [&'a Problem],
    /// The stored configuration of the hosts that graphs name.
    configs: &'a HashMap<String, EffectiveHost>,
}

impl Context<'_> {
    fn selects(&self, host: &str) -> bool {
        self.hosts.iter().any(|h| h.name == host)
    }
}

/// The data of a graph or value panel that could not be asked for or read.
enum Data {
    /// Nothing to show and nothing wrong with the metrics store: the panel names a host the
    /// dashboard does not show, or a series that is not one.
    Unavailable,
    /// The store did not answer.
    Failed,
}

/// Renders one panel; the flag is false when the metrics store failed it.
async fn render<'a>(ctx: &'a Context<'a>, panel: &'a Panel) -> (PanelOut<'a>, bool) {
    match panel {
        Panel::Problems { at } => {
            let shown = &ctx.problems[..ctx.problems.len().min(PROBLEMS_PANEL_LIMIT)];
            (
                PanelOut::Problems {
                    place: *at,
                    problems: shown,
                    more: ctx.problems.len() - shown.len(),
                },
                true,
            )
        }
        Panel::HostStates { at } => (
            PanelOut::HostStates {
                place: *at,
                total: ctx.hosts.len(),
                without_problems: ctx.hosts.iter().filter(|h| h.open.is_empty()).count(),
                hosts: ctx.entries,
            },
            true,
        ),
        Panel::Graph {
            host,
            item,
            params,
            range,
            at,
        } => {
            let params = params.clone().unwrap_or_default();
            let step_secs = step_for(range.as_secs());
            let (points, ok) = match graph_points(ctx, host, item, &params, *range, step_secs).await
            {
                Ok(points) => (Some(points), true),
                Err(Data::Unavailable) => (None, true),
                Err(Data::Failed) => (None, false),
            };
            let thresholds = ctx
                .configs
                .get(host.as_str())
                .filter(|_| ctx.selects(host))
                .map(|config| thresholds(config, item, &params))
                .unwrap_or_default();
            (
                PanelOut::Graph {
                    place: *at,
                    host,
                    item,
                    params,
                    range: *range,
                    step_secs,
                    points,
                    thresholds,
                },
                ok,
            )
        }
        Panel::Value {
            host,
            item,
            params,
            at,
        } => {
            let params = params.clone().unwrap_or_default();
            let (sample, ok) = match latest_value(ctx, host, item, &params).await {
                Ok(sample) => (sample, true),
                Err(Data::Unavailable) => (None, true),
                Err(Data::Failed) => (None, false),
            };
            (
                PanelOut::Value {
                    place: *at,
                    host,
                    item,
                    params,
                    value: sample.map(|(_, value)| value),
                    at: sample.and_then(|(ts_ms, _)| ms_to_rfc3339(ts_ms)),
                },
                ok,
            )
        }
    }
}

/// A series can be asked for when its host is shown by the dashboard and the parameters name
/// one series: a stored panel may break either rule when it was applied before a rule
/// existed or a later revision removed the host.
fn askable(
    ctx: &Context<'_>,
    host: &str,
    item: &str,
    params: &BTreeMap<String, String>,
) -> Result<(), Data> {
    if ctx.selects(host) && names_one_series(item, params) {
        Ok(())
    } else {
        Err(Data::Unavailable)
    }
}

async fn graph_points(
    ctx: &Context<'_>,
    host: &str,
    item: &str,
    params: &BTreeMap<String, String>,
    range: Interval,
    step_secs: u32,
) -> Result<Vec<(i64, f64)>, Data> {
    askable(ctx, host, item, params)?;
    let from =
        ctx.now - time::Duration::seconds(i64::try_from(range.as_secs()).unwrap_or(i64::MAX));
    let query = range_query(
        host.to_owned(),
        item.to_owned(),
        params.clone(),
        from,
        ctx.now,
        step_secs,
    );
    match bounded(PANEL_TIMEOUT, ctx.metrics.query_range(&query)).await {
        Ok(samples) => Ok(samples.into_iter().map(|s| (s.ts_ms, s.value)).collect()),
        Err(e) => {
            log_metrics_failure("dashboard graph", &e);
            Err(Data::Failed)
        }
    }
}

/// The newest value of the series and when it was measured; `None` when there is none.
async fn latest_value(
    ctx: &Context<'_>,
    host: &str,
    item: &str,
    params: &BTreeMap<String, String>,
) -> Result<Option<(i64, f64)>, Data> {
    askable(ctx, host, item, params)?;
    let query = LatestQuery {
        host: Some(host.to_owned()),
        key: Some(item.to_owned()),
        max_age_secs: CURRENT_VALUE_MAX_AGE_SECS,
    };
    match bounded(PANEL_TIMEOUT, ctx.metrics.latest(&query)).await {
        Ok(samples) => Ok(samples
            .into_iter()
            .filter(|s| s.params == *params)
            .max_by_key(|s| s.ts_ms)
            .map(|s| (s.ts_ms, s.value))),
        Err(e) => {
            log_metrics_failure("dashboard value", &e);
            Err(Data::Failed)
        }
    }
}

/// Everything one dashboard shows.
async fn detail(
    State(app): State<AppState>,
    Viewer(_): Viewer,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    answer(&app, &id, Reader::User).await
}

/// Who reads a dashboard answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reader {
    /// A logged-in user: everything.
    User,
    /// A wall display by its token: no names of people and no comments.
    DisplayToken,
}

/// Removes from the problems of `hosts` who acknowledged or closed them and the comments,
/// keeping `acknowledged_at` and everything else.
fn without_user_fields(hosts: &mut [HostRow]) {
    for problem in hosts.iter_mut().flat_map(|h| h.open.iter_mut()) {
        problem.acknowledged_by = None;
        problem.ack_comment = None;
        problem.closed_by = None;
        problem.close_comment = None;
    }
}

/// The answer of the dashboard route, also given to a wall display through its own route.
/// An unknown id, or one that cannot be a dashboard id, is a 404.
///
/// For [`Reader::DisplayToken`] the user fields are removed from the loaded problems before
/// anything is built from them: every problem of the answer, in any panel, comes from those.
pub(super) async fn answer(app: &AppState, id: &str, reader: Reader) -> Result<Response, ApiError> {
    if validate_name(id).is_err() {
        return Err(not_found());
    }
    let mut board = app
        .store
        .dashboard_hosts(id)
        .await
        .map_err(|e| store_failed("reading a dashboard failed", &e))?
        .ok_or_else(not_found)?;
    if reader == Reader::DisplayToken {
        without_user_fields(&mut board.hosts);
    }
    let panels = board.dashboard.effective_panels();
    let hosts: Vec<&HostRow> = board.hosts.iter().collect();

    // The limits drawn into a graph come from the configuration of its host: one query for
    // all graphs.
    let graph_hosts: BTreeSet<&str> = panels
        .iter()
        .filter_map(|panel| match panel {
            Panel::Graph { host, .. } if hosts.iter().any(|h| h.name == *host) => {
                Some(host.as_str())
            }
            _ => None,
        })
        .collect();
    let configs = if graph_hosts.is_empty() {
        HashMap::new()
    } else {
        let names: Vec<String> = graph_hosts.into_iter().map(str::to_owned).collect();
        app.store
            .host_configs(&names)
            .await
            .map_err(|e| store_failed("reading host configurations failed", &e))?
    };

    let entries = host_entries(&hosts);
    let problems = open_problems(&hosts);
    let ctx = Context {
        metrics: app.metrics.as_ref(),
        now: OffsetDateTime::now_utc(),
        hosts: &hosts,
        entries: &entries,
        problems: &problems,
        configs: &configs,
    };
    // All panels at once: the metrics calls wait side by side, so a slow store costs one
    // time limit, not one per panel.
    let rendered = join_all(panels.iter().map(|panel| render(&ctx, panel))).await;
    let metrics_available = rendered.iter().all(|(_, ok)| *ok);
    Ok(Json(DetailOut {
        id,
        name: &board.dashboard.name,
        file: &board.file,
        priority: board.dashboard.priority,
        hosts: &entries,
        panels: rendered.into_iter().map(|(panel, _)| panel).collect(),
        metrics_available,
    })
    .into_response())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use axum::http::StatusCode;
    use cerbrix_common::MetricValue;
    use cerbrix_common::config::Severity;
    use serde_json::{Value, json};
    use sqlx::PgPool;
    use time::OffsetDateTime;

    use crate::alerting::{Evaluation, OpenProblem, ProblemKind};
    use crate::api::ui::testing::*;
    use crate::auth::Role;
    use crate::metrics::{
        LatestQuery, LatestSample, MemoryMetrics, MetricsError, MetricsStore, RangeQuery, Sample,
    };

    const T0_MS: i64 = 1_800_000_000_000;

    async fn get(fx: &Fixture, uri: &str) -> Reply {
        let token = fx.session(Role::Viewer).await;
        fx.call("GET", uri, Some(&token), &[], None).await
    }

    fn now_ms() -> i64 {
        (OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
    }

    fn mv(key: &str, params: &[(&str, &str)], ts_ms: i64, value: f64) -> MetricValue {
        MetricValue {
            key: key.into(),
            params: params
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ts_ms,
            value,
        }
    }

    fn keys(value: &Value) -> Vec<&str> {
        let mut keys: Vec<_> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        keys
    }

    async fn open_problem(fx: &Fixture, host: &str, trigger: &str, severity: Severity) {
        let id = fx.store.host_by_name(host).await.unwrap().unwrap().id;
        let problem = OpenProblem {
            trigger: trigger.to_owned(),
            instance: String::new(),
            severity,
            at_ms: T0_MS,
            summary: format!("{trigger} is bad"),
            kind: ProblemKind::Compare,
        };
        fx.store
            .evaluate_host(id, false, |_| Evaluation {
                open: vec![problem],
                ..Evaluation::default()
            })
            .await
            .unwrap();
    }

    /// Stores a dashboard as the configuration sync would, without checking it.
    async fn add_dashboard(fx: &Fixture, id: &str, body: &str) {
        sqlx::query(
            "INSERT INTO config_documents (kind, id, source_file, body) \
             VALUES ('dashboard', $1, $2, $3::jsonb)",
        )
        .bind(id)
        .bind(format!("dashboards/{id}.json"))
        .bind(body)
        .execute(&fx.pool)
        .await
        .unwrap();
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn dashboards_list_shape(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.apply_example().await;
        open_problem(&fx, "web-01", "Disk almost full", Severity::High).await;

        let reply = get(&fx, "/api/v1/dashboards").await;
        assert_eq!(reply.status, StatusCode::OK);
        let body = reply.json();
        assert_eq!(keys(&body), ["items"]);
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        let d = &items[0];
        assert_eq!(
            keys(d),
            [
                "file",
                "host_count",
                "hosts_by_state",
                "id",
                "name",
                "open_problems",
                "priority",
                "unacknowledged"
            ]
        );
        assert_eq!(d["id"], "production");
        assert_eq!(d["name"], "Production");
        assert_eq!(d["file"], "dashboards/production.json");
        assert_eq!(d["priority"], 0);
        assert_eq!(d["host_count"], 2);
        assert_eq!(
            d["hosts_by_state"],
            json!({"critical": 0, "high": 1, "warning": 0, "info": 0, "acknowledged": 0, "ok": 1})
        );
        assert_eq!(
            d["open_problems"],
            json!({"critical": 0, "high": 1, "warning": 0, "info": 0})
        );
        assert_eq!(
            d["unacknowledged"],
            json!({"critical": 0, "high": 1, "warning": 0, "info": 0})
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn dashboards_list_is_ordered_and_empty_without_dashboards(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        assert_eq!(
            get(&fx, "/api/v1/dashboards").await.json(),
            json!({"items": []})
        );
        fx.apply_example().await;
        add_dashboard(
            &fx,
            "quiet",
            r#"{"name":"Quiet","priority":50,"hosts":{"names":["db-01"]}}"#,
        )
        .await;
        add_dashboard(
            &fx,
            "loud",
            r#"{"name":"Loud","priority":1,"hosts":{"names":["web-01"]}}"#,
        )
        .await;
        open_problem(&fx, "web-01", "Cpu", Severity::Warning).await;
        let body = get(&fx, "/api/v1/dashboards").await.json();
        let ids: Vec<_> = body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["id"].as_str().unwrap())
            .collect();
        // `production` also selects web-01, with priority 0: after `loud`.
        assert_eq!(ids, ["loud", "production", "quiet"]);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn dashboard_detail_standard_layout(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.apply_example().await;
        add_dashboard(
            &fx,
            "plain",
            r#"{"name":"Plain","priority":3,"hosts":{"names":["web-01"]}}"#,
        )
        .await;
        let reply = get(&fx, "/api/v1/dashboards/plain").await;
        assert_eq!(reply.status, StatusCode::OK);
        let body = reply.json();
        assert_eq!(
            keys(&body),
            [
                "file",
                "hosts",
                "id",
                "metrics_available",
                "name",
                "panels",
                "priority"
            ]
        );
        assert_eq!(body["id"], "plain");
        assert_eq!(body["name"], "Plain");
        assert_eq!(body["file"], "dashboards/plain.json");
        assert_eq!(body["priority"], 3);
        assert_eq!(body["metrics_available"], true);
        assert_eq!(body["hosts"], json!([{"name": "web-01", "state": null}]));

        let panels = body["panels"].as_array().unwrap();
        assert_eq!(panels.len(), 2);
        let problems = &panels[0];
        assert_eq!(
            keys(problems),
            ["h", "more", "problems", "type", "w", "x", "y"]
        );
        assert_eq!(problems["type"], "problems");
        assert_eq!(
            (
                problems["x"].clone(),
                problems["y"].clone(),
                problems["w"].clone(),
                problems["h"].clone()
            ),
            (json!(0), json!(0), json!(8), json!(6))
        );
        assert_eq!(problems["problems"], json!([]));
        assert_eq!(problems["more"], 0);
        let states = &panels[1];
        assert_eq!(
            keys(states),
            [
                "h",
                "hosts",
                "total",
                "type",
                "w",
                "without_problems",
                "x",
                "y"
            ]
        );
        assert_eq!(states["type"], "host-states");
        assert_eq!(
            (
                states["x"].clone(),
                states["y"].clone(),
                states["w"].clone(),
                states["h"].clone()
            ),
            (json!(8), json!(0), json!(4), json!(6))
        );
        assert_eq!(states["total"], 1);
        assert_eq!(states["without_problems"], 1);
        assert_eq!(states["hosts"], json!([{"name": "web-01", "state": null}]));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn hosts_and_the_host_states_panel_list_problem_hosts_first(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        // Applying the example deactivates the fixture host: it is not in the repository.
        fx.apply_example().await;
        fx.store.create_host("a-quiet").await.unwrap();
        open_problem(&fx, "web-01", "Cpu", Severity::Warning).await;
        open_problem(&fx, "db-01", "Disk", Severity::Critical).await;
        add_dashboard(&fx, "all", r#"{"name":"All"}"#).await;
        let body = get(&fx, "/api/v1/dashboards/all").await.json();
        let names: Vec<_> = body["hosts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["db-01", "web-01", "a-quiet"]);
        assert_eq!(body["hosts"][0]["state"]["severity"], "critical");
        let states = &body["panels"][1];
        assert_eq!(states["total"], 3);
        assert_eq!(states["without_problems"], 1);
        assert_eq!(states["hosts"], body["hosts"]);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn problems_panel_caps_at_100_and_reports_more(pool: PgPool) {
        let fx = Fixture::new(pool.clone()).await;
        let host = fx.store.host_by_name(fx.host).await.unwrap().unwrap().id;
        sqlx::query(
            "INSERT INTO problems (id, host_id, trigger_name, instance, severity, summary, \
             started_at) \
             SELECT gen_random_uuid(), $1, 'T' || i, '', \
                    CASE WHEN i % 2 = 0 THEN 'high' ELSE 'info' END, 'summary', \
                    now() - (i || ' minutes')::interval \
             FROM generate_series(1, 130) AS i",
        )
        .bind(host)
        .execute(&pool)
        .await
        .unwrap();
        add_dashboard(&fx, "all", r#"{"name":"All"}"#).await;
        let body = get(&fx, "/api/v1/dashboards/all").await.json();
        let panel = &body["panels"][0];
        let problems = panel["problems"].as_array().unwrap();
        assert_eq!(problems.len(), 100);
        assert_eq!(panel["more"], 30);
        // 65 high ones come first (newest first), then the info ones.
        assert!(problems[..65].iter().all(|p| p["severity"] == "high"));
        assert!(problems[65..].iter().all(|p| p["severity"] == "info"));
        let started: Vec<_> = problems[..65]
            .iter()
            .map(|p| p["started_at"].as_str().unwrap())
            .collect();
        assert!(started.windows(2).all(|w| w[0] >= w[1]), "newest first");
        assert_eq!(problems[0]["trigger"], "T2");
        // The same shape as everywhere: fifteen keys.
        assert_eq!(problems[0].as_object().unwrap().len(), 15);
        assert_eq!(body["panels"][1]["hosts"][0]["state"]["open_problems"], 130);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn graph_panel_has_points_and_thresholds(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.apply_example().await;
        let now = now_ms();
        fx.metrics
            .write(
                "db-01",
                &[
                    mv("disk.used_percent", &[("mount", "/")], now - 600_000, 52.0),
                    mv(
                        "disk.used_percent",
                        &[("mount", "/")],
                        now - 1_200_000,
                        51.0,
                    ),
                    // Another series, and a sample older than the 6 h range.
                    mv(
                        "disk.used_percent",
                        &[("mount", "/data")],
                        now - 600_000,
                        99.0,
                    ),
                    mv(
                        "disk.used_percent",
                        &[("mount", "/")],
                        now - 7 * 3_600_000,
                        10.0,
                    ),
                ],
            )
            .await
            .unwrap();
        let reply = get(&fx, "/api/v1/dashboards/production").await;
        assert_eq!(reply.status, StatusCode::OK);
        let body = reply.json();
        assert_eq!(body["metrics_available"], true);
        let types: Vec<_> = body["panels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["type"].as_str().unwrap())
            .collect();
        assert_eq!(types, ["problems", "host-states", "graph"]);

        let graph = &body["panels"][2];
        assert_eq!(
            keys(graph),
            [
                "h",
                "host",
                "item",
                "params",
                "points",
                "range",
                "step_secs",
                "thresholds",
                "type",
                "w",
                "x",
                "y"
            ]
        );
        assert_eq!(graph["host"], "db-01");
        assert_eq!(graph["item"], "disk.used_percent");
        assert_eq!(graph["params"], json!({"mount": "/"}));
        assert_eq!(graph["range"], "6h");
        assert_eq!(graph["step_secs"], 72);
        assert_eq!(
            (
                graph["x"].clone(),
                graph["y"].clone(),
                graph["w"].clone(),
                graph["h"].clone()
            ),
            (json!(8), json!(3), json!(4), json!(3))
        );
        let points = graph["points"].as_array().unwrap();
        assert_eq!(points.len(), 2, "{points:?}");
        assert!(points[0][0].as_i64().unwrap() < points[1][0].as_i64().unwrap());
        assert_eq!(
            (points[0][1].clone(), points[1][1].clone()),
            (json!(51.0), json!(52.0))
        );
        let thresholds = graph["thresholds"].as_array().unwrap();
        assert_eq!(thresholds.len(), 1, "{thresholds:?}");
        assert_eq!(keys(&thresholds[0]), ["op", "severity", "trigger", "value"]);
        assert_eq!(thresholds[0]["trigger"], "Disk almost full");
        assert_eq!(thresholds[0]["op"], ">");
        assert_eq!(thresholds[0]["value"].as_f64(), Some(90.0));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_graph_range_sets_the_span_and_the_step(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.apply_example().await;
        let now = now_ms();
        fx.metrics
            .write(
                "web-01",
                &[
                    mv("cpu.usage", &[], now - 30_000, 1.0),
                    mv("cpu.usage", &[], now - 3_000_000, 2.0),
                ],
            )
            .await
            .unwrap();
        let panel = |range: &str| {
            format!(
                r#"{{"type":"graph","host":"web-01","item":"cpu.usage","range":"{range}","x":0,"y":0,"w":2,"h":2}}"#
            )
        };
        for (range, step, points) in [("90s", 15, 1), ("1h", 15, 2), ("12h", 144, 2)] {
            add_dashboard(
                &fx,
                &format!("r-{range}"),
                &format!(
                    r#"{{"name":"{range}","hosts":{{"names":["web-01"]}},"panels":[{}]}}"#,
                    panel(range)
                ),
            )
            .await;
            let body = get(&fx, &format!("/api/v1/dashboards/r-{range}"))
                .await
                .json();
            let graph = &body["panels"][0];
            assert_eq!(graph["range"], range);
            assert_eq!(graph["step_secs"], step, "{range}");
            assert_eq!(graph["points"].as_array().unwrap().len(), points, "{range}");
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn value_panel_latest(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.apply_example().await;
        let now = now_ms() / 1000 * 1000;
        fx.metrics
            .write(
                "web-01",
                &[
                    mv("cpu.usage", &[], now - 20_000, 11.0),
                    mv("cpu.usage", &[], now - 5_000, 12.5),
                    mv("disk.used_percent", &[("mount", "/")], now - 5_000, 50.0),
                    mv(
                        "disk.used_percent",
                        &[("mount", "/data")],
                        now - 5_000,
                        70.0,
                    ),
                ],
            )
            .await
            .unwrap();
        add_dashboard(
            &fx,
            "values",
            r#"{"name":"Values","hosts":{"names":["web-01"]},"panels":[
                {"type":"value","host":"web-01","item":"cpu.usage","x":0,"y":0,"w":2,"h":1},
                {"type":"value","host":"web-01","item":"disk.used_percent","params":{"mount":"/data"},"x":2,"y":0,"w":2,"h":1},
                {"type":"value","host":"web-01","item":"memory.used_percent","x":4,"y":0,"w":2,"h":1}]}"#,
        )
        .await;
        let reply = get(&fx, "/api/v1/dashboards/values").await;
        assert_eq!(reply.status, StatusCode::OK);
        let body = reply.json();
        assert_eq!(body["metrics_available"], true);
        let [cpu, disk, none] = &body["panels"].as_array().unwrap()[..] else {
            panic!("three panels");
        };
        assert_eq!(
            keys(cpu),
            [
                "at", "h", "host", "item", "params", "type", "value", "w", "x", "y"
            ]
        );
        assert_eq!(cpu["type"], "value");
        assert_eq!(cpu["host"], "web-01");
        assert_eq!(cpu["item"], "cpu.usage");
        assert_eq!(cpu["params"], json!({}));
        assert_eq!(cpu["value"], 12.5);
        let at = crate::view::rfc3339(
            OffsetDateTime::from_unix_timestamp((now - 5_000) / 1000).unwrap(),
        );
        assert_eq!(cpu["at"], at);
        assert_eq!(disk["params"], json!({"mount": "/data"}));
        assert_eq!(disk["value"], 70.0);
        assert!(none["value"].is_null() && none["at"].is_null());
        assert_eq!(none["item"], "memory.used_percent");
    }

    /// A dashboard with one graph, one value and the two standard panels, for web-01 and the
    /// example dashboard's graph of db-01.
    async fn with_series_panels(fx: &Fixture) {
        fx.apply_example().await;
        add_dashboard(
            fx,
            "series",
            r#"{"name":"Series","panels":[
                {"type":"problems","x":0,"y":0,"w":4,"h":2},
                {"type":"graph","host":"web-01","item":"cpu.usage","x":4,"y":0,"w":4,"h":2},
                {"type":"graph","host":"db-01","item":"disk.used_percent","params":{"mount":"/"},"x":8,"y":0,"w":4,"h":2},
                {"type":"value","host":"web-01","item":"cpu.usage","x":0,"y":2,"w":2,"h":1}]}"#,
        )
        .await;
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn metrics_outage_gives_null_data_not_an_error(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        with_series_panels(&fx).await;
        open_problem(&fx, "web-01", "Cpu", Severity::High).await;
        fx.metrics.set_failing(true);
        let reply = get(&fx, "/api/v1/dashboards/series").await;
        assert_eq!(reply.status, StatusCode::OK);
        let body = reply.json();
        assert_eq!(body["metrics_available"], false);
        let panels = body["panels"].as_array().unwrap();
        assert_eq!(panels[0]["problems"].as_array().unwrap().len(), 1);
        for graph in &panels[1..3] {
            assert!(graph["points"].is_null(), "{graph}");
            assert!(graph["step_secs"].as_u64().unwrap() > 0);
        }
        // The limits come from the configuration, not from the metrics store.
        assert_eq!(panels[2]["thresholds"].as_array().unwrap().len(), 1);
        assert!(panels[3]["value"].is_null() && panels[3]["at"].is_null());
        assert_eq!(body["hosts"].as_array().unwrap().len(), 2);

        // The store is back: the same request has data again.
        fx.metrics.set_failing(false);
        fx.metrics
            .write("web-01", &[mv("cpu.usage", &[], now_ms() - 5_000, 3.0)])
            .await
            .unwrap();
        let body = get(&fx, "/api/v1/dashboards/series").await.json();
        assert_eq!(body["metrics_available"], true);
        assert_eq!(body["panels"][3]["value"], 3.0);
        assert_eq!(body["panels"][1]["points"].as_array().unwrap().len(), 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_refusing_metrics_store_gives_null_data_too(pool: PgPool) {
        let mut fx = Fixture::new(pool).await;
        with_series_panels(&fx).await;
        fx.use_metrics(Arc::new(Refusing));
        let reply = get(&fx, "/api/v1/dashboards/series").await;
        assert_eq!(reply.status, StatusCode::OK);
        let body = reply.json();
        assert_eq!(body["metrics_available"], false);
        assert!(body["panels"][1]["points"].is_null());
        assert!(body["panels"][3]["value"].is_null());
        assert!(!String::from_utf8_lossy(&reply.body).contains("\"no\""));
    }

    /// A store that refuses the range query of one item and serves everything else from
    /// memory.
    struct PickyStore {
        inner: MemoryMetrics,
        refused_key: &'static str,
    }

    #[async_trait::async_trait]
    impl MetricsStore for PickyStore {
        async fn write(&self, host: &str, values: &[MetricValue]) -> Result<(), MetricsError> {
            self.inner.write(host, values).await
        }
        async fn query_range(&self, query: &RangeQuery) -> Result<Vec<Sample>, MetricsError> {
            if query.key == self.refused_key {
                return Err(MetricsError::Rejected("more than one series".into()));
            }
            self.inner.query_range(query).await
        }
        async fn latest(&self, query: &LatestQuery) -> Result<Vec<LatestSample>, MetricsError> {
            self.inner.latest(query).await
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn one_failing_series_leaves_the_other_panels_alone(pool: PgPool) {
        let mut fx = Fixture::new(pool).await;
        fx.apply_example().await;
        let store = PickyStore {
            inner: MemoryMetrics::new(),
            refused_key: "memory.used_percent",
        };
        store
            .write(
                "web-01",
                &[
                    mv("cpu.usage", &[], now_ms() - 5_000, 4.0),
                    mv("memory.used_percent", &[], now_ms() - 5_000, 60.0),
                ],
            )
            .await
            .unwrap();
        fx.use_metrics(Arc::new(store));
        add_dashboard(
            &fx,
            "two",
            r#"{"name":"Two","hosts":{"names":["web-01"]},"panels":[
                {"type":"graph","host":"web-01","item":"cpu.usage","x":0,"y":0,"w":2,"h":2},
                {"type":"graph","host":"web-01","item":"memory.used_percent","x":2,"y":0,"w":2,"h":2},
                {"type":"value","host":"web-01","item":"memory.used_percent","x":4,"y":0,"w":2,"h":2}]}"#,
        )
        .await;
        let body = get(&fx, "/api/v1/dashboards/two").await.json();
        assert_eq!(body["metrics_available"], false);
        assert_eq!(body["panels"][0]["points"].as_array().unwrap().len(), 1);
        assert!(body["panels"][1]["points"].is_null());
        assert_eq!(body["panels"][2]["value"], 60.0);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_slow_metrics_store_costs_one_timeout_not_one_per_panel(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.apply_example().await;
        add_dashboard(
            &fx,
            "slow",
            r#"{"name":"Slow","hosts":{"names":["web-01"]},"panels":[
                {"type":"problems","x":0,"y":0,"w":2,"h":2},
                {"type":"graph","host":"web-01","item":"cpu.usage","x":2,"y":0,"w":2,"h":2},
                {"type":"graph","host":"web-01","item":"memory.used_percent","x":4,"y":0,"w":2,"h":2},
                {"type":"value","host":"web-01","item":"cpu.usage","x":6,"y":0,"w":2,"h":2},
                {"type":"value","host":"web-01","item":"memory.used_percent","x":8,"y":0,"w":2,"h":2}]}"#,
        )
        .await;
        fx.metrics.set_delay(Duration::from_secs(120));
        let started = Instant::now();
        let reply = get(&fx, "/api/v1/dashboards/slow").await;
        let took = started.elapsed();
        assert_eq!(reply.status, StatusCode::OK);
        let body = reply.json();
        assert_eq!(body["metrics_available"], false);
        assert!(body["panels"][1]["points"].is_null());
        assert!(body["panels"][4]["value"].is_null());
        // Four panels wait at the same time, each for at most 5 s: well below the 20 s of
        // waiting one after another, with room for a loaded machine.
        assert!(
            took >= Duration::from_secs(5) && took < Duration::from_secs(15),
            "{took:?}"
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_stored_panel_without_the_parameter_gets_null_data(pool: PgPool) {
        // A revision applied before the compiler required the parameter.
        let fx = Fixture::new(pool).await;
        fx.apply_example().await;
        let now = now_ms();
        fx.metrics
            .write(
                "db-01",
                &[
                    mv("disk.used_percent", &[("mount", "/")], now - 5_000, 50.0),
                    mv(
                        "disk.used_percent",
                        &[("mount", "/data")],
                        now - 5_000,
                        70.0,
                    ),
                ],
            )
            .await
            .unwrap();
        add_dashboard(
            &fx,
            "old",
            r#"{"name":"Old","hosts":{"names":["db-01"]},"panels":[
                {"type":"graph","host":"db-01","item":"disk.used_percent","x":0,"y":0,"w":2,"h":2},
                {"type":"value","host":"db-01","item":"disk.used_percent","x":2,"y":0,"w":2,"h":2},
                {"type":"graph","host":"db-01","item":"disk.used_percent","params":{"interface":"eth0"},"x":4,"y":0,"w":2,"h":2},
                {"type":"value","host":"db-01","item":"cpu.usage","params":{"mount":"/"},"x":6,"y":0,"w":2,"h":2},
                {"type":"value","host":"db-01","item":"disk.used_percent","params":{"mount":"/"},"x":8,"y":0,"w":2,"h":2}]}"#,
        )
        .await;
        let reply = get(&fx, "/api/v1/dashboards/old").await;
        assert_eq!(reply.status, StatusCode::OK);
        let body = reply.json();
        // Nothing was asked of the store for the unusable panels: no outage to report.
        assert_eq!(body["metrics_available"], true);
        let panels = body["panels"].as_array().unwrap();
        assert!(panels[0]["points"].is_null());
        assert_eq!(panels[0]["params"], json!({}));
        assert!(panels[1]["value"].is_null());
        assert!(panels[2]["points"].is_null());
        assert!(panels[3]["value"].is_null());
        assert_eq!(panels[4]["value"], 50.0);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_panel_of_a_host_that_is_not_selected_gets_null_data(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.apply_example().await;
        fx.metrics
            .write("db-01", &[mv("cpu.usage", &[], now_ms() - 5_000, 9.0)])
            .await
            .unwrap();
        fx.metrics
            .write("web-01", &[mv("cpu.usage", &[], now_ms() - 5_000, 4.0)])
            .await
            .unwrap();
        add_dashboard(
            &fx,
            "web",
            r#"{"name":"Web","hosts":{"names":["web-01"]},"panels":[
                {"type":"value","host":"web-01","item":"cpu.usage","x":0,"y":0,"w":2,"h":2},
                {"type":"value","host":"db-01","item":"cpu.usage","x":2,"y":0,"w":2,"h":2},
                {"type":"graph","host":"db-01","item":"cpu.usage","x":4,"y":0,"w":2,"h":2},
                {"type":"graph","host":"ghost","item":"cpu.usage","x":6,"y":0,"w":2,"h":2}]}"#,
        )
        .await;
        let reply = get(&fx, "/api/v1/dashboards/web").await;
        assert_eq!(reply.status, StatusCode::OK);
        let body = reply.json();
        assert_eq!(body["metrics_available"], true);
        let panels = body["panels"].as_array().unwrap();
        assert_eq!(panels[0]["value"], 4.0);
        assert!(panels[1]["value"].is_null());
        assert!(panels[2]["points"].is_null() && panels[2]["thresholds"] == json!([]));
        assert!(panels[3]["points"].is_null());
        assert_eq!(panels[3]["host"], "ghost");
        // The hosts of the dashboard are the selected ones only.
        assert_eq!(body["hosts"].as_array().unwrap().len(), 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn unknown_dashboard_is_not_found(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.apply_example().await;
        let long = "a".repeat(64);
        for id in ["nope", "Production", "bad%20id", "..", "-x", "a%2Fb", &long] {
            let reply = get(&fx, &format!("/api/v1/dashboards/{id}")).await;
            assert_eq!(reply.status, StatusCode::NOT_FOUND, "{id}");
            assert_eq!(reply.error_code(), "not_found", "{id}");
        }
        assert_eq!(
            get(&fx, "/api/v1/dashboards/production").await.status,
            StatusCode::OK
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn dashboard_with_many_hosts(pool: PgPool) {
        let fx = Fixture::new(pool.clone()).await;
        sqlx::query(
            "INSERT INTO hosts (id, name) \
             SELECT gen_random_uuid(), 'h' || lpad(i::text, 4, '0') FROM generate_series(0, 999) AS i",
        )
        .execute(&pool)
        .await
        .unwrap();
        // Two open problems per host, by the pattern of `i % 4`: critical and warning
        // unacknowledged; both acknowledged (high, info); critical acknowledged and high
        // unacknowledged; two unacknowledged info problems.
        sqlx::query(
            "INSERT INTO problems (id, host_id, trigger_name, instance, severity, summary, \
                                   started_at, acknowledged_at, acknowledged_by) \
             SELECT gen_random_uuid(), h.id, t.name, '', t.severity, 's', now() - interval '1 hour', \
                    CASE WHEN t.acked THEN now() END, CASE WHEN t.acked THEN 'anna' END \
             FROM (SELECT id, substr(name, 2)::int % 4 AS k FROM hosts WHERE name LIKE 'h%') h \
             JOIN (VALUES (0, 'A', 'critical', false), (0, 'B', 'warning', false), \
                          (1, 'A', 'high', true), (1, 'B', 'info', true), \
                          (2, 'A', 'critical', true), (2, 'B', 'high', false), \
                          (3, 'A', 'info', false), (3, 'B', 'info', false)) \
                  AS t(k, name, severity, acked) ON t.k = h.k",
        )
        .execute(&pool)
        .await
        .unwrap();
        add_dashboard(&fx, "all", r#"{"name":"All"}"#).await;

        let list = get(&fx, "/api/v1/dashboards").await.json();
        let d = &list["items"][0];
        // The fixture host (active: nothing was applied) has no problem.
        assert_eq!(d["host_count"], 1001);
        assert_eq!(
            d["hosts_by_state"],
            json!({"critical": 250, "high": 250, "warning": 0, "info": 250, "acknowledged": 250, "ok": 1})
        );
        assert_eq!(
            d["open_problems"],
            json!({"critical": 500, "high": 500, "warning": 250, "info": 750})
        );
        assert_eq!(
            d["unacknowledged"],
            json!({"critical": 250, "high": 250, "warning": 250, "info": 500})
        );

        let body = get(&fx, "/api/v1/dashboards/all").await.json();
        assert_eq!(body["hosts"].as_array().unwrap().len(), 1001);
        let problems = &body["panels"][0];
        assert_eq!(problems["problems"].as_array().unwrap().len(), 100);
        assert_eq!(problems["more"], 1900);
        assert_eq!(problems["problems"][0]["severity"], "critical");
        let states = &body["panels"][1];
        assert_eq!(states["total"], 1001);
        assert_eq!(states["without_problems"], 1);
        // Critical hosts first, ordered by name.
        assert_eq!(body["hosts"][0]["name"], "h0000");
        assert_eq!(body["hosts"][249]["name"], "h0996");
        assert_eq!(body["hosts"][250]["state"]["severity"], "high");
    }
}
