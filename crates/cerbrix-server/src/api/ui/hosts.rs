//! The hosts list, the groups and the detail of one host.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use cerbrix_common::config::Severity;
use cerbrix_common::{
    MAX_PARAMS, builtin_item, default_agent_items, validate_host_name, validate_item_key,
    validate_param_name,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::{ApiError, Entry, Page, invalid, query_text};
use crate::api::AppState;
use crate::auth::Viewer;
use crate::gitops::{EffectiveHost, EffectiveItem, EffectiveTrigger};
use crate::metrics::{LatestQuery, MetricsError, MetricsStore};
use crate::state::{HostDetail, HostQuery, HostStateFilter, Problem, StateError};
use crate::view::dashboard::{DashboardRef, dashboards_of_hosts};
use crate::view::series::{
    CURRENT_VALUE_MAX_AGE_SECS, Threshold, bounded, log_metrics_failure, ms_to_rfc3339,
    range_query, thresholds,
};
use crate::view::{HostState, host_state, rfc3339, serialize_time_opt};

pub(super) fn entries() -> Vec<Entry> {
    vec![
        Entry::new("GET", "/api/v1/hosts", list),
        Entry::new("GET", "/api/v1/host-groups", groups),
        Entry::new("GET", "/api/v1/hosts/{name}", detail),
        Entry::new("GET", "/api/v1/hosts/{name}/values", values),
        Entry::new("GET", "/api/v1/hosts/{name}/series", series),
    ]
}

/// The longest search text taken.
const MAX_SEARCH_CHARS: usize = 200;

/// The longest group name (the length of a valid one).
const MAX_GROUP_CHARS: usize = 63;

/// How long the hosts list and the values of a host wait for the metrics store.
const CURRENT_VALUES_TIMEOUT: Duration = Duration::from_secs(3);

/// How long a history series waits for the metrics store.
const SERIES_TIMEOUT: Duration = Duration::from_secs(10);

/// The newest value shown in the hosts list is at most this old.
const LIST_MAX_AGE_SECS: u32 = 600;

/// The longest parameter value taken in a series request.
const MAX_PARAM_VALUE_CHARS: usize = 256;

fn store_failed(what: &str, e: &StateError) -> ApiError {
    tracing::error!(error = %e, "{what}");
    ApiError::internal()
}

/// A host of the list.
#[derive(Serialize)]
struct HostOut {
    name: String,
    address: Option<String>,
    groups: Vec<String>,
    state: Option<HostState>,
    #[serde(serialize_with = "serialize_time_opt")]
    last_seen_at: Option<OffsetDateTime>,
    managed_by_git: bool,
    /// `cpu.usage`, `null` without a recent value or when the metrics store is down.
    cpu: Option<f64>,
    /// `memory.used_percent`, as `cpu`.
    memory: Option<f64>,
}

/// The answer of the hosts list.
#[derive(Serialize)]
struct HostsListed {
    items: Vec<HostOut>,
    total: i64,
    /// False when a current value could not be read from the metrics store.
    metrics_available: bool,
}

/// The newest value of the item `key` of every host that has one, one call to the store.
async fn newest_by_host(
    metrics: &dyn MetricsStore,
    key: &str,
) -> Result<HashMap<String, f64>, MetricsError> {
    let query = LatestQuery {
        host: None,
        key: Some(key.to_owned()),
        max_age_secs: LIST_MAX_AGE_SECS,
    };
    let samples = bounded(CURRENT_VALUES_TIMEOUT, metrics.latest(&query)).await?;
    let mut newest: HashMap<String, (i64, f64)> = HashMap::new();
    for s in samples.into_iter().filter(|s| s.params.is_empty()) {
        let entry = newest.entry(s.host).or_insert((s.ts_ms, s.value));
        if entry.0 < s.ts_ms {
            *entry = (s.ts_ms, s.value);
        }
    }
    Ok(newest.into_iter().map(|(host, (_, v))| (host, v)).collect())
}

#[derive(Deserialize)]
struct HostsParams {
    search: Option<String>,
    group: Option<String>,
    state: Option<String>,
}

async fn list(
    State(app): State<AppState>,
    Viewer(_): Viewer,
    page: Page,
    params: Result<Query<HostsParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<HostsListed>, ApiError> {
    let Query(params) = params.map_err(|_| invalid("the query parameters are malformed"))?;
    for (value, max, message) in [
        (
            &params.search,
            MAX_SEARCH_CHARS,
            "search must have at most 200 characters and no control character",
        ),
        (
            &params.group,
            MAX_GROUP_CHARS,
            "group must have at most 63 characters and no control character",
        ),
    ] {
        if let Some(text) = value {
            query_text(text, max, message)?;
        }
    }
    let state = params
        .state
        .as_deref()
        .map(|s| {
            s.parse::<HostStateFilter>()
                .map_err(|()| invalid("state must be problems, no_data or ok"))
        })
        .transpose()?;
    let query = HostQuery {
        search: params.search,
        group: params.group,
        state,
        limit: page.limit,
        offset: page.offset,
    };
    let (rows, total) = app
        .store
        .hosts_page(&query)
        .await
        .map_err(|e| store_failed("listing hosts failed", &e))?;
    let managed_by_git = app.config.gitops.enabled;
    // Both calls run at once and give up after the same time; a store that is down or slow
    // empties the column, never the list.
    let (cpu, memory) = tokio::join!(
        newest_by_host(app.metrics.as_ref(), "cpu.usage"),
        newest_by_host(app.metrics.as_ref(), "memory.used_percent"),
    );
    let mut metrics_available = true;
    let mut column = |what: &str, result: Result<HashMap<String, f64>, MetricsError>| match result {
        Ok(values) => values,
        Err(e) => {
            log_metrics_failure(what, &e);
            metrics_available = false;
            HashMap::new()
        }
    };
    let (cpu, memory) = (
        column("hosts list: cpu", cpu),
        column("hosts list: memory", memory),
    );
    Ok(Json(HostsListed {
        items: rows
            .into_iter()
            .map(|row| HostOut {
                state: host_state(&row.open),
                cpu: cpu.get(&row.name).copied(),
                memory: memory.get(&row.name).copied(),
                name: row.name,
                address: row.address,
                groups: row.groups,
                last_seen_at: row.last_seen_at,
                managed_by_git,
            })
            .collect(),
        total,
        metrics_available,
    }))
}

#[derive(Serialize)]
struct Groups {
    items: Vec<String>,
}

async fn groups(State(app): State<AppState>, Viewer(_): Viewer) -> Result<Json<Groups>, ApiError> {
    let items = app
        .store
        .host_groups()
        .await
        .map_err(|e| store_failed("listing host groups failed", &e))?;
    Ok(Json(Groups { items }))
}

#[derive(Serialize)]
struct TriggerOut {
    name: String,
    expression: String,
    severity: Severity,
    origin: String,
    firing: bool,
}

#[derive(Serialize)]
struct DetailOut {
    name: String,
    address: Option<String>,
    groups: Vec<String>,
    templates: Vec<String>,
    source_file: Option<String>,
    managed_by_git: bool,
    #[serde(serialize_with = "serialize_time_opt")]
    enrolled_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "serialize_time_opt")]
    last_seen_at: Option<OffsetDateTime>,
    /// The version the agent reported in its last request that carried one.
    agent_version: Option<String>,
    /// The dashboards that select this host, sorted by name.
    dashboards: Vec<DashboardRef>,
    state: Option<HostState>,
    items: Vec<EffectiveItem>,
    triggers: Vec<TriggerOut>,
    open_problems: Vec<Problem>,
}

async fn detail(
    State(app): State<AppState>,
    Viewer(_): Viewer,
    Path(name): Path<String>,
) -> Result<Json<DetailOut>, ApiError> {
    let not_found = || ApiError::new(StatusCode::NOT_FOUND, "not_found", "no such host");
    // A name that cannot be a host name names no host.
    if validate_host_name(&name).is_err() {
        return Err(not_found());
    }
    let HostDetail {
        row,
        config,
        enrolled_at,
        agent_version,
        firing,
    } = app
        .store
        .host_detail(&name)
        .await
        .map_err(|e| store_failed("reading a host failed", &e))?
        .ok_or_else(not_found)?;
    // Membership is decided by `selected_hosts` alone, on this one host.
    let dashboards = app
        .store
        .dashboards()
        .await
        .map_err(|e| store_failed("reading dashboards failed", &e))?;
    let dashboards = dashboards_of_hosts(&dashboards, std::slice::from_ref(&row))
        .remove(&row.name)
        .unwrap_or_default();
    // A host created by hand has no stored configuration: nothing is configured.
    let config = config.unwrap_or(EffectiveHost {
        templates: Vec::new(),
        items: Vec::new(),
        triggers: Vec::new(),
    });
    Ok(Json(DetailOut {
        state: host_state(&row.open),
        name: row.name,
        address: row.address,
        groups: row.groups,
        templates: config.templates,
        source_file: row.source_file,
        managed_by_git: app.config.gitops.enabled,
        enrolled_at,
        last_seen_at: row.last_seen_at,
        agent_version,
        dashboards,
        items: config.items,
        triggers: config
            .triggers
            .into_iter()
            .map(|t: EffectiveTrigger| TriggerOut {
                firing: firing.contains(&t.name),
                name: t.name,
                expression: t.expression,
                severity: t.severity,
                origin: t.origin,
            })
            .collect(),
        open_problems: row.open,
    }))
}

/// One current value of a host.
#[derive(Serialize)]
struct ValueOut {
    key: String,
    params: BTreeMap<String, String>,
    value: f64,
    /// When the value was measured, RFC 3339.
    at: String,
}

#[derive(Serialize)]
struct ValuesOut {
    items: Vec<ValueOut>,
    /// False when the metrics store could not be read; `items` is then empty.
    metrics_available: bool,
}

/// An active host by name, or 404.
async fn active_host(app: &AppState, name: &str) -> Result<(), ApiError> {
    let not_found = || ApiError::new(StatusCode::NOT_FOUND, "not_found", "no such host");
    // A name that cannot be a host name names no host.
    if validate_host_name(name).is_err() {
        return Err(not_found());
    }
    match app
        .store
        .host_by_name(name)
        .await
        .map_err(|e| store_failed("reading a host failed", &e))?
    {
        Some(host) if host.active => Ok(()),
        _ => Err(not_found()),
    }
}

/// The newest value of every series of a host. A store that is down or slow gives an empty
/// list, not an error: the page can still show the rest of the host.
async fn values(
    State(app): State<AppState>,
    Viewer(_): Viewer,
    Path(name): Path<String>,
) -> Result<Json<ValuesOut>, ApiError> {
    active_host(&app, &name).await?;
    let query = LatestQuery {
        host: Some(name),
        key: None,
        max_age_secs: CURRENT_VALUE_MAX_AGE_SECS,
    };
    let samples = match bounded(CURRENT_VALUES_TIMEOUT, app.metrics.latest(&query)).await {
        Ok(samples) => samples,
        Err(e) => {
            log_metrics_failure("host values", &e);
            return Ok(Json(ValuesOut {
                items: Vec::new(),
                metrics_available: false,
            }));
        }
    };
    let mut items: Vec<ValueOut> = samples
        .into_iter()
        .filter_map(|s| {
            Some(ValueOut {
                at: ms_to_rfc3339(s.ts_ms)?,
                key: s.key,
                params: s.params,
                value: s.value,
            })
        })
        .collect();
    items.sort_by(|a, b| (&a.key, &a.params).cmp(&(&b.key, &b.params)));
    Ok(Json(ValuesOut {
        items,
        metrics_available: true,
    }))
}

/// The ranges of a series with the resolution used for each: seconds and step.
fn series_range(range: &str) -> Option<(i64, u32)> {
    match range {
        "1h" => Some((3_600, 15)),
        "6h" => Some((21_600, 90)),
        "24h" => Some((86_400, 300)),
        "7d" => Some((604_800, 1_800)),
        _ => None,
    }
}

/// What a series request names: the item, the range and the item's parameters.
struct SeriesRequest {
    item: String,
    range: String,
    params: BTreeMap<String, String>,
}

/// Reads the query of a series request. Every parameter is known and given once: `item`,
/// `range` and `p_<name>` for a parameter of the item; anything else is refused.
fn series_request(pairs: Vec<(String, String)>) -> Result<SeriesRequest, ApiError> {
    let mut item = None;
    let mut range = None;
    let mut params = BTreeMap::new();
    let twice = || invalid("a query parameter is given more than once");
    for (name, value) in pairs {
        match name.as_str() {
            "item" if item.is_none() => item = Some(value),
            "range" if range.is_none() => range = Some(value),
            "item" | "range" => return Err(twice()),
            _ => {
                let param = name
                    .strip_prefix("p_")
                    .ok_or_else(|| invalid("unknown query parameter"))?;
                validate_param_name(param)
                    .map_err(|_| invalid("a parameter name must be p_ and a valid name"))?;
                query_text(
                    &value,
                    MAX_PARAM_VALUE_CHARS,
                    "a parameter value must have 1 to 256 characters and no control character",
                )?;
                if value.is_empty() {
                    return Err(invalid(
                        "a parameter value must have 1 to 256 characters and no control character",
                    ));
                }
                if params.insert(param.to_owned(), value).is_some() {
                    return Err(twice());
                }
            }
        }
    }
    if params.len() > MAX_PARAMS {
        return Err(invalid("too many item parameters"));
    }
    let item = item.ok_or_else(|| invalid("item is required"))?;
    validate_item_key(&item).map_err(|_| invalid("item is not a valid item key"))?;
    check_item_params(&item, &params)?;
    let range = range.ok_or_else(|| invalid("range is required"))?;
    if series_range(&range).is_none() {
        return Err(invalid("range must be 1h, 6h, 24h or 7d"));
    }
    Ok(SeriesRequest {
        item,
        range,
        params,
    })
}

/// A series is one item with exactly the parameter the item has: an item with a parameter
/// needs it (without it the selector would match every instance and the points of several
/// series would be mixed), an item without one takes none. Items that are not built in are
/// not known well enough to check. Parameter names are validated identifiers, so they can
/// be named in the message.
fn check_item_params(item: &str, params: &BTreeMap<String, String>) -> Result<(), ApiError> {
    let Some(spec) = builtin_item(item) else {
        return Ok(());
    };
    let wrong =
        |message: String| ApiError::with_message(StatusCode::BAD_REQUEST, "invalid", message);
    match spec.param {
        Some(want) => {
            if !params.contains_key(want) {
                return Err(wrong(format!("{item} needs the parameter p_{want}")));
            }
            if let Some(other) = params.keys().find(|name| name.as_str() != want) {
                return Err(wrong(format!(
                    "{item} has no parameter p_{other}, only p_{want}"
                )));
            }
        }
        None => {
            if let Some(other) = params.keys().next() {
                return Err(wrong(format!("{item} has no parameter p_{other}")));
            }
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct SeriesOut {
    key: String,
    params: BTreeMap<String, String>,
    from: String,
    to: String,
    step_secs: u32,
    /// `[ts_ms, value]` pairs in time order.
    points: Vec<(i64, f64)>,
    thresholds: Vec<Threshold>,
}

/// The history of one series of a host for a graph, with the limits of the triggers on it.
async fn series(
    State(app): State<AppState>,
    Viewer(_): Viewer,
    Path(name): Path<String>,
    query: Result<Query<Vec<(String, String)>>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<SeriesOut>, ApiError> {
    let Query(pairs) = query.map_err(|_| invalid("the query parameters are malformed"))?;
    let request = series_request(pairs)?;
    let not_found = || ApiError::new(StatusCode::NOT_FOUND, "not_found", "no such host");
    if validate_host_name(&name).is_err() {
        return Err(not_found());
    }
    let detail = app
        .store
        .host_detail(&name)
        .await
        .map_err(|e| store_failed("reading a host failed", &e))?
        .ok_or_else(not_found)?;
    // A host created by hand has no stored configuration: it collects the default items.
    let known = match &detail.config {
        Some(config) => config.items.iter().any(|i| i.key == request.item),
        None => default_agent_items().iter().any(|i| i.key == request.item),
    };
    if !known {
        return Err(invalid("item is not an item of this host"));
    }
    let thresholds = detail
        .config
        .as_ref()
        .map(|config| thresholds(config, &request.item, &request.params))
        .unwrap_or_default();

    let (secs, step_secs) = series_range(&request.range).unwrap_or((3_600, 15));
    let to = OffsetDateTime::now_utc();
    let from = to - time::Duration::seconds(secs);
    let range = range_query(
        name,
        request.item.clone(),
        request.params.clone(),
        from,
        to,
        step_secs,
    );
    let samples = match bounded(SERIES_TIMEOUT, app.metrics.query_range(&range)).await {
        Ok(samples) => samples,
        Err(e @ MetricsError::Unavailable(_)) => {
            log_metrics_failure("host series", &e);
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "metrics_unavailable",
                "the metrics store is not available",
            ));
        }
        // The input was checked, so the store refusing it is a bug of ours.
        Err(e) => {
            log_metrics_failure("host series", &e);
            return Err(ApiError::internal());
        }
    };
    Ok(Json(SeriesOut {
        key: request.item,
        params: request.params,
        from: rfc3339(from),
        to: rfc3339(to),
        step_secs,
        points: samples.into_iter().map(|s| (s.ts_ms, s.value)).collect(),
        thresholds,
    }))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::http::StatusCode;
    use cerbrix_common::MetricValue;
    use cerbrix_common::config::Severity;
    use serde_json::{Value, json};
    use sqlx::PgPool;
    use time::OffsetDateTime;

    use crate::alerting::{
        ActiveTrigger, Evaluation, OpenProblem, ProblemKind, active_triggers, eval::evaluate,
    };
    use crate::api::ui::testing::*;
    use crate::auth::Role;
    use crate::metrics::MetricsStore as _;

    const T0_MS: i64 = 1_800_000_000_000;

    async fn get(fx: &Fixture, uri: &str) -> Reply {
        let token = fx.session(Role::Viewer).await;
        fx.call("GET", uri, Some(&token), &[], None).await
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

    #[sqlx::test(migrations = "./migrations")]
    async fn hosts_list_shape(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.apply_example().await;
        fx.store.create_host("zz-bare").await.unwrap();
        open_problem(&fx, "web-01", "Disk almost full", Severity::High).await;

        let reply = get(&fx, "/api/v1/hosts").await;
        assert_eq!(reply.status, StatusCode::OK);
        let body = reply.json();
        assert_eq!(keys(&body), ["items", "metrics_available", "total"]);
        assert_eq!(body["metrics_available"], true);
        assert_eq!(body["total"], 3);
        let items = body["items"].as_array().unwrap();
        let names: Vec<_> = items.iter().map(|h| h["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["web-01", "db-01", "zz-bare"]);

        let web = &items[0];
        assert_eq!(
            keys(web),
            [
                "address",
                "cpu",
                "groups",
                "last_seen_at",
                "managed_by_git",
                "memory",
                "name",
                "state"
            ]
        );
        assert_eq!(web["address"], "10.0.4.12");
        assert_eq!(web["groups"], json!(["production", "web"]));
        assert_eq!(web["managed_by_git"], false);
        assert!(web["last_seen_at"].is_null());
        let state = &web["state"];
        assert_eq!(
            keys(state),
            [
                "acknowledged",
                "instance",
                "kind",
                "open_problems",
                "severity",
                "summary",
                "trigger"
            ]
        );
        assert_eq!(state["severity"], "high");
        assert_eq!(state["acknowledged"], false);
        assert_eq!(state["kind"], "compare");
        assert_eq!(state["trigger"], "Disk almost full");
        assert_eq!(state["open_problems"], 1);
        assert!(items[1]["state"].is_null());
        assert!(items[2]["address"].is_null());
        assert_eq!(items[2]["groups"], json!([]));

        // Filters and paging reach the store.
        let reply = get(&fx, "/api/v1/hosts?search=DB&limit=1").await;
        assert_eq!(reply.json()["total"], 1);
        assert_eq!(reply.json()["items"][0]["name"], "db-01");
        let reply = get(&fx, "/api/v1/hosts?group=web").await;
        assert_eq!(reply.json()["total"], 1);
        let reply = get(&fx, "/api/v1/hosts?state=problems").await;
        assert_eq!(reply.json()["items"].as_array().unwrap().len(), 1);
        let reply = get(&fx, "/api/v1/hosts?state=ok&limit=1&offset=1").await;
        assert_eq!(reply.json()["total"], 2);
        assert_eq!(reply.json()["items"][0]["name"], "zz-bare");

        let reply = get(&fx, "/api/v1/host-groups").await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.json(), json!({"items": ["db", "production", "web"]}));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn host_detail_shape(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.apply_example().await;
        let host = fx.store.host_by_name("web-01").await.unwrap().unwrap();
        fx.store.bind_credential(host.id).await.unwrap();

        // A value that makes `Disk almost full` (> 90 for 10m) fire: held for 11 minutes.
        for (ts, value) in [(T0_MS, 95.0), (T0_MS + 11 * 60_000, 96.0)] {
            fx.store
                .evaluate_host(host.id, false, |ctx| {
                    let triggers: Vec<ActiveTrigger> =
                        active_triggers(ctx.config.as_ref().unwrap());
                    evaluate(
                        &triggers,
                        &ctx.states,
                        &[MetricValue {
                            key: "disk.used_percent".into(),
                            params: [("mount".to_owned(), "/".to_owned())].into(),
                            ts_ms: ts,
                            value,
                        }],
                        ts,
                        60_000,
                    )
                })
                .await
                .unwrap();
        }

        let reply = get(&fx, "/api/v1/hosts/web-01").await;
        assert_eq!(reply.status, StatusCode::OK);
        let body = reply.json();
        assert_eq!(
            keys(&body),
            [
                "address",
                "agent_version",
                "dashboards",
                "enrolled_at",
                "groups",
                "items",
                "last_seen_at",
                "managed_by_git",
                "name",
                "open_problems",
                "source_file",
                "state",
                "templates",
                "triggers"
            ]
        );
        assert_eq!(body["name"], "web-01");
        assert_eq!(body["address"], "10.0.4.12");
        assert_eq!(body["groups"], json!(["production", "web"]));
        assert_eq!(body["templates"], json!(["linux-base"]));
        assert_eq!(body["source_file"], "hosts/web-01.json");
        assert_eq!(body["managed_by_git"], false);
        let enrolled = body["enrolled_at"].as_str().unwrap();
        assert!(enrolled.ends_with('Z'), "{enrolled}");

        let items = body["items"].as_array().unwrap();
        let cpu = items.iter().find(|i| i["key"] == "cpu.usage").unwrap();
        assert_eq!(keys(cpu), ["interval_secs", "key", "origin", "params"]);
        assert_eq!(cpu["interval_secs"], 10);
        assert_eq!(cpu["origin"], "host");
        assert!(cpu["params"].is_null());
        let memory = items
            .iter()
            .find(|i| i["key"] == "memory.used_percent")
            .unwrap();
        assert_eq!(memory["origin"], "template:linux-base");

        let triggers = body["triggers"].as_array().unwrap();
        let disk = triggers
            .iter()
            .find(|t| t["name"] == "Disk almost full")
            .unwrap();
        assert_eq!(
            keys(disk),
            ["expression", "firing", "name", "origin", "severity"]
        );
        assert_eq!(disk["firing"], true);
        assert_eq!(disk["severity"], "high");
        assert_eq!(disk["origin"], "template:linux-base");
        assert_eq!(disk["expression"], "disk.used_percent > 90 for 10m");
        let silent = triggers
            .iter()
            .find(|t| t["name"] == "Agent silent")
            .unwrap();
        assert_eq!(silent["firing"], false);

        let problems = body["open_problems"].as_array().unwrap();
        assert_eq!(problems.len(), 1);
        let p = &problems[0];
        assert_eq!(
            keys(p),
            [
                "ack_comment",
                "acknowledged_at",
                "acknowledged_by",
                "close_comment",
                "closed_by",
                "host",
                "id",
                "instance",
                "kind",
                "resolve_reason",
                "resolved_at",
                "severity",
                "started_at",
                "summary",
                "trigger"
            ]
        );
        assert_eq!(p["host"], "web-01");
        assert_eq!(p["trigger"], "Disk almost full");
        assert!(p["resolved_at"].is_null());
        assert_eq!(body["state"]["trigger"], "Disk almost full");
        assert_eq!(body["state"]["open_problems"], 1);

        // No credential material leaves the server.
        let text = String::from_utf8_lossy(&reply.body).to_lowercase();
        assert!(
            !text.contains("credential") && !text.contains("hash"),
            "{text}"
        );
    }

    async fn add_dashboard(fx: &Fixture, id: &str, body: &str) {
        sqlx::query(
            "INSERT INTO config_documents (kind, id, source_file, body)              VALUES ('dashboard', $1, $2, $3::jsonb)",
        )
        .bind(id)
        .bind(format!("dashboards/{id}.json"))
        .bind(body)
        .execute(&fx.pool)
        .await
        .unwrap();
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn host_detail_has_agent_version_and_dashboards(pool: PgPool) {
        let fx = Fixture::new(pool.clone()).await;
        fx.store.create_host("lonely").await.unwrap();
        sqlx::query(
            "UPDATE hosts SET groups = ARRAY['prod'], agent_version = '1.2.3' WHERE name = $1",
        )
        .bind(fx.host)
        .execute(&pool)
        .await
        .unwrap();
        // Sorted by name, not by id; one selects by group, one by name, one not at all.
        add_dashboard(
            &fx,
            "a-last",
            r#"{"name":"Zulu","hosts":{"groups":["prod"]}}"#,
        )
        .await;
        add_dashboard(
            &fx,
            "z-first",
            r#"{"name":"Alpha","hosts":{"names":["fixture-host"]}}"#,
        )
        .await;
        add_dashboard(
            &fx,
            "other",
            r#"{"name":"Other","hosts":{"groups":["dev"]}}"#,
        )
        .await;

        let body = get(&fx, "/api/v1/hosts/fixture-host").await.json();
        assert_eq!(body["agent_version"], "1.2.3");
        assert_eq!(
            body["dashboards"],
            json!([
                {"id": "z-first", "name": "Alpha"},
                {"id": "a-last", "name": "Zulu"}
            ])
        );

        let body = get(&fx, "/api/v1/hosts/lonely").await.json();
        assert!(body["agent_version"].is_null());
        assert_eq!(body["dashboards"], json!([]));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn host_without_configuration_has_empty_lists(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let reply = get(&fx, "/api/v1/hosts/fixture-host").await;
        assert_eq!(reply.status, StatusCode::OK);
        let body = reply.json();
        for key in [
            "templates",
            "items",
            "triggers",
            "open_problems",
            "groups",
            "dashboards",
        ] {
            assert_eq!(body[key], json!([]), "{key}");
        }
        for key in [
            "address",
            "source_file",
            "enrolled_at",
            "last_seen_at",
            "agent_version",
            "state",
        ] {
            assert!(body[key].is_null(), "{key}");
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn unknown_host_is_not_found(pool: PgPool) {
        let fx = Fixture::new(pool.clone()).await;
        for name in ["nope", "bad%20name", "..", "-x", "a%2Fb"] {
            let reply = get(&fx, &format!("/api/v1/hosts/{name}")).await;
            assert_eq!(reply.status, StatusCode::NOT_FOUND, "{name}");
            assert_eq!(reply.error_code(), "not_found", "{name}");
        }
        let long = "a".repeat(254);
        let reply = get(&fx, &format!("/api/v1/hosts/{long}")).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND);

        let host = fx
            .store
            .host_by_name("fixture-host")
            .await
            .unwrap()
            .unwrap();
        fx.store.set_host_active(host.id, false).await.unwrap();
        let reply = get(&fx, "/api/v1/hosts/fixture-host").await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn bad_query_parameters_are_invalid(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        let long = "a".repeat(201);
        for uri in [
            "/api/v1/hosts?state=bogus".to_owned(),
            "/api/v1/hosts?state=".to_owned(),
            "/api/v1/hosts?state=OK".to_owned(),
            "/api/v1/hosts?limit=0".to_owned(),
            "/api/v1/hosts?offset=-1".to_owned(),
            format!("/api/v1/hosts?search={long}"),
            "/api/v1/hosts?search=%00".to_owned(),
            "/api/v1/hosts?search=a%0Ab".to_owned(),
            "/api/v1/hosts?group=%00".to_owned(),
            "/api/v1/hosts?group=a%0Ab".to_owned(),
            format!("/api/v1/hosts?group={}", "g".repeat(64)),
        ] {
            let reply = get(&fx, &uri).await;
            assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{uri}");
            assert_eq!(reply.error_code(), "invalid", "{uri}");
        }
        let exact = "a".repeat(200);
        let reply = get(&fx, &format!("/api/v1/hosts?search={exact}")).await;
        assert_eq!(reply.status, StatusCode::OK);
        for state in ["problems", "no_data", "ok"] {
            let reply = get(&fx, &format!("/api/v1/hosts?state={state}")).await;
            assert_eq!(reply.status, StatusCode::OK, "{state}");
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn managed_by_git_follows_the_setting(pool: PgPool) {
        let mut config = (*config(true, None)).clone();
        config.gitops.enabled = true;
        let fx = Fixture::with_config(pool, Arc::new(config)).await;
        let list = get(&fx, "/api/v1/hosts").await.json();
        assert_eq!(list["items"][0]["managed_by_git"], true);
        let detail = get(&fx, "/api/v1/hosts/fixture-host").await.json();
        assert_eq!(detail["managed_by_git"], true);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn host_search_is_literal_over_http(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.store.create_host("a_b").await.unwrap();
        fx.store.create_host("axb").await.unwrap();
        let reply = get(&fx, "/api/v1/hosts?search=a_b").await;
        assert_eq!(reply.json()["total"], 1);
        let reply = get(&fx, "/api/v1/hosts?search=%25").await;
        assert_eq!(reply.json()["total"], 0);
    }

    // ---- current values and series ----

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

    #[sqlx::test(migrations = "./migrations")]
    async fn hosts_list_has_cpu_and_memory(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.apply_example().await;
        fx.store.create_host("zz-bare").await.unwrap();
        let now = now_ms();
        fx.metrics
            .write(
                "web-01",
                &[
                    mv("cpu.usage", &[], now - 20_000, 11.0),
                    mv("cpu.usage", &[], now - 5_000, 12.5),
                    mv("memory.used_percent", &[], now - 5_000, 60.0),
                ],
            )
            .await
            .unwrap();
        // Too old for the list (600 s), and a host that is not on the page.
        fx.metrics
            .write("db-01", &[mv("cpu.usage", &[], now - 700_000, 99.0)])
            .await
            .unwrap();
        fx.metrics
            .write(
                "zz-bare",
                &[mv("memory.used_percent", &[], now - 5_000, 33.0)],
            )
            .await
            .unwrap();
        fx.metrics
            .write("ghost", &[mv("cpu.usage", &[], now - 5_000, 1.0)])
            .await
            .unwrap();

        let body = get(&fx, "/api/v1/hosts").await.json();
        assert_eq!(body["metrics_available"], true);
        let items = body["items"].as_array().unwrap();
        let names: Vec<_> = items.iter().map(|h| h["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["db-01", "web-01", "zz-bare"]);
        assert_eq!(items[1]["cpu"], 12.5);
        assert_eq!(items[1]["memory"], 60.0);
        assert!(items[0]["cpu"].is_null() && items[0]["memory"].is_null());
        assert!(items[2]["cpu"].is_null());
        assert_eq!(items[2]["memory"], 33.0);

        // Only the hosts of the page are in the answer.
        let body = get(&fx, "/api/v1/hosts?limit=1&offset=1").await.json();
        assert_eq!(body["items"].as_array().unwrap().len(), 1);
        assert_eq!(body["items"][0]["name"], "web-01");
        assert_eq!(body["items"][0]["cpu"], 12.5);
        assert!(!String::from_utf8_lossy(&get(&fx, "/api/v1/hosts").await.body).contains("ghost"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn hosts_list_survives_metrics_outage(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.apply_example().await;
        fx.metrics
            .write("web-01", &[mv("cpu.usage", &[], now_ms() - 5_000, 12.5)])
            .await
            .unwrap();
        fx.metrics.set_failing(true);
        let reply = get(&fx, "/api/v1/hosts").await;
        assert_eq!(reply.status, StatusCode::OK);
        let body = reply.json();
        assert_eq!(body["metrics_available"], false);
        assert_eq!(body["total"], 2);
        for host in body["items"].as_array().unwrap() {
            assert!(host["cpu"].is_null() && host["memory"].is_null(), "{host}");
        }
        fx.metrics.set_failing(false);
        let body = get(&fx, "/api/v1/hosts").await.json();
        assert_eq!(body["metrics_available"], true);
        let web = body["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["name"] == "web-01");
        assert_eq!(web.unwrap()["cpu"], 12.5);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn hosts_list_does_not_wait_for_a_slow_metrics_store(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.apply_example().await;
        fx.metrics.set_delay(std::time::Duration::from_secs(120));
        let started = std::time::Instant::now();
        let reply = get(&fx, "/api/v1/hosts").await;
        let took = started.elapsed();
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.json()["metrics_available"], false);
        assert!(reply.json()["items"][0]["cpu"].is_null());
        // The two calls run concurrently, each with a 3 s limit: below the 6 s of waiting
        // one after another.
        assert!(
            took >= std::time::Duration::from_secs(3) && took < std::time::Duration::from_secs(6),
            "{took:?}"
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_refusing_metrics_store_is_an_error_only_for_the_series(pool: PgPool) {
        let mut fx = Fixture::new(pool).await;
        fx.use_metrics(Arc::new(Refusing));
        let reply = get(&fx, "/api/v1/hosts").await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.json()["metrics_available"], false);
        let reply = get(&fx, "/api/v1/hosts/fixture-host/values").await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.json()["metrics_available"], false);
        let reply = get(
            &fx,
            "/api/v1/hosts/fixture-host/series?item=cpu.usage&range=1h",
        )
        .await;
        assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(reply.error_code(), "internal");
        assert!(!String::from_utf8_lossy(&reply.body).contains("no\""));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn values_shape_and_order(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.apply_example().await;
        let now = now_ms() / 1000 * 1000;
        fx.metrics
            .write(
                "web-01",
                &[
                    mv("memory.used_percent", &[], now - 9_000, 61.0),
                    mv(
                        "disk.used_percent",
                        &[("mount", "/data")],
                        now - 8_000,
                        70.0,
                    ),
                    mv("disk.used_percent", &[("mount", "/")], now - 7_000, 50.0),
                    mv("disk.used_percent", &[("mount", "/")], now - 6_000, 51.0),
                    mv("cpu.usage", &[], now - 5_000, 12.5),
                    // Older than an hour, and not a built-in item: not shown.
                    mv("system.load1", &[], now - 3_700_000, 1.0),
                    mv("custom.thing", &[], now - 5_000, 1.0),
                ],
            )
            .await
            .unwrap();
        fx.metrics
            .write("db-01", &[mv("cpu.usage", &[], now - 5_000, 99.0)])
            .await
            .unwrap();

        let reply = get(&fx, "/api/v1/hosts/web-01/values").await;
        assert_eq!(reply.status, StatusCode::OK);
        let body = reply.json();
        assert_eq!(keys(&body), ["items", "metrics_available"]);
        assert_eq!(body["metrics_available"], true);
        let items = body["items"].as_array().unwrap();
        let shape: Vec<_> = items
            .iter()
            .map(|i| (i["key"].as_str().unwrap(), i["params"].clone()))
            .collect();
        assert_eq!(
            shape,
            [
                ("cpu.usage", json!({})),
                ("disk.used_percent", json!({"mount": "/"})),
                ("disk.used_percent", json!({"mount": "/data"})),
                ("memory.used_percent", json!({})),
            ]
        );
        assert_eq!(keys(&items[0]), ["at", "key", "params", "value"]);
        assert_eq!(items[0]["value"], 12.5);
        assert_eq!(items[1]["value"], 51.0, "the newest sample of the series");
        let at =
            |ms: i64| crate::view::rfc3339(OffsetDateTime::from_unix_timestamp(ms / 1000).unwrap());
        assert_eq!(items[0]["at"], at(now - 5_000));
        assert_eq!(items[1]["at"], at(now - 6_000));

        // A store outage still answers, with no values.
        fx.metrics.set_failing(true);
        let reply = get(&fx, "/api/v1/hosts/web-01/values").await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(
            reply.json(),
            json!({"items": [], "metrics_available": false})
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn values_of_an_unknown_host_are_not_found(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        for name in ["nope", "bad%20name", "..", "-x"] {
            for what in ["values", "series?item=cpu.usage&range=1h"] {
                let reply = get(&fx, &format!("/api/v1/hosts/{name}/{what}")).await;
                assert_eq!(reply.status, StatusCode::NOT_FOUND, "{name} {what}");
                assert_eq!(reply.error_code(), "not_found");
            }
        }
        // A host without values has none to show.
        let reply = get(&fx, "/api/v1/hosts/fixture-host/values").await;
        assert_eq!(
            reply.json(),
            json!({"items": [], "metrics_available": true})
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn series_returns_points_and_thresholds(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.apply_example().await;
        let now = now_ms();
        fx.metrics
            .write(
                "web-01",
                &[
                    mv("disk.used_percent", &[("mount", "/")], now - 600_000, 52.0),
                    mv(
                        "disk.used_percent",
                        &[("mount", "/")],
                        now - 1_200_000,
                        51.0,
                    ),
                    mv(
                        "disk.used_percent",
                        &[("mount", "/")],
                        now - 4 * 3_600_000,
                        40.0,
                    ),
                    mv(
                        "disk.used_percent",
                        &[("mount", "/data")],
                        now - 600_000,
                        99.0,
                    ),
                ],
            )
            .await
            .unwrap();
        let reply = get(
            &fx,
            "/api/v1/hosts/web-01/series?item=disk.used_percent&range=1h&p_mount=/",
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
        let body = reply.json();
        assert_eq!(
            keys(&body),
            [
                "from",
                "key",
                "params",
                "points",
                "step_secs",
                "thresholds",
                "to"
            ]
        );
        assert_eq!(body["key"], "disk.used_percent");
        assert_eq!(body["params"], json!({"mount": "/"}));
        assert_eq!(body["step_secs"], 15);
        let points = body["points"].as_array().unwrap();
        assert_eq!(points.len(), 2, "{points:?}");
        assert!(points[0][0].as_i64().unwrap() < points[1][0].as_i64().unwrap());
        assert_eq!(points[0][1], 51.0);
        assert_eq!(points[1][1], 52.0);
        let thresholds = body["thresholds"].as_array().unwrap();
        assert_eq!(thresholds.len(), 1, "{thresholds:?}");
        let t = &thresholds[0];
        assert_eq!(keys(t), ["op", "severity", "trigger", "value"]);
        assert_eq!(t["trigger"], "Disk almost full");
        assert_eq!(t["severity"], "high");
        assert_eq!(t["op"], ">");
        assert_eq!(t["value"].as_f64(), Some(90.0));

        // `from` and `to` are RFC 3339 in UTC, `to` is now and `from` an hour before.
        let parse = |v: &Value| {
            OffsetDateTime::parse(
                v.as_str().unwrap(),
                &time::format_description::well_known::Rfc3339,
            )
            .unwrap()
        };
        assert!(body["to"].as_str().unwrap().ends_with('Z'));
        let (from, to) = (parse(&body["from"]), parse(&body["to"]));
        assert_eq!((to - from).whole_seconds(), 3600);
        assert!((OffsetDateTime::now_utc() - to).whole_seconds().abs() < 30);

        // An item without a trigger has no thresholds; a series without data has no points.
        let body = get(&fx, "/api/v1/hosts/web-01/series?item=cpu.usage&range=1h")
            .await
            .json();
        assert_eq!(body["points"], json!([]));
        assert_eq!(body["thresholds"], json!([]));
        assert_eq!(body["params"], json!({}));
        // Another mount is another series.
        let body = get(
            &fx,
            "/api/v1/hosts/web-01/series?item=disk.used_percent&range=1h&p_mount=/data",
        )
        .await
        .json();
        assert_eq!(body["points"].as_array().unwrap().len(), 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn series_step_per_range(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.apply_example().await;
        for (range, step, secs) in [
            ("1h", 15, 3_600),
            ("6h", 90, 21_600),
            ("24h", 300, 86_400),
            ("7d", 1_800, 604_800),
        ] {
            let body = get(
                &fx,
                &format!("/api/v1/hosts/web-01/series?item=cpu.usage&range={range}"),
            )
            .await
            .json();
            assert_eq!(body["step_secs"], step, "{range}");
            let parse = |v: &Value| {
                OffsetDateTime::parse(
                    v.as_str().unwrap(),
                    &time::format_description::well_known::Rfc3339,
                )
                .unwrap()
            };
            let span = parse(&body["to"]) - parse(&body["from"]);
            assert_eq!(span.whole_seconds(), secs, "{range}");
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn series_unknown_item_is_invalid(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.apply_example().await;
        for item in [
            "disk.usedpercent",
            // Valid built-in key that web-01 does not collect.
            "net.rx_bytes_per_sec",
            "",
            "CPU.usage",
            "cpu.usage%7B",
            "cpu.usage%00",
        ] {
            let reply = get(
                &fx,
                &format!("/api/v1/hosts/web-01/series?item={item}&range=1h"),
            )
            .await;
            assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{item}");
            assert_eq!(reply.error_code(), "invalid", "{item}");
        }
        let reply = get(&fx, "/api/v1/hosts/web-01/series?range=1h").await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_host_without_configuration_has_the_default_items(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        // Known by the default items, with no trigger to draw.
        let reply = get(
            &fx,
            "/api/v1/hosts/fixture-host/series?item=net.rx_bytes_per_sec&range=1h&p_interface=eth0",
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.json()["thresholds"], json!([]));
        let reply = get(
            &fx,
            "/api/v1/hosts/fixture-host/series?item=not.a.default&range=1h",
        )
        .await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn series_bad_range_is_invalid(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.apply_example().await;
        for range in ["", "2h", "1H", "30d", "1h%20", "1h&range=6h", "-1h"] {
            let reply = get(
                &fx,
                &format!("/api/v1/hosts/web-01/series?item=cpu.usage&range={range}"),
            )
            .await;
            assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{range}");
            assert_eq!(reply.error_code(), "invalid", "{range}");
        }
        let reply = get(&fx, "/api/v1/hosts/web-01/series?item=cpu.usage").await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn series_needs_the_items_parameter(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.apply_example().await;
        // Without the mount the selector would match every mount and mix their points.
        let reply = get(
            &fx,
            "/api/v1/hosts/web-01/series?item=disk.used_percent&range=1h",
        )
        .await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
        assert_eq!(reply.error_code(), "invalid");
        assert!(String::from_utf8_lossy(&reply.body).contains("mount"));
        // The item has one parameter: no other.
        let reply = get(
            &fx,
            "/api/v1/hosts/web-01/series?item=disk.used_percent&range=1h&p_mount=/&p_interface=eth0",
        )
        .await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
        let reply = get(
            &fx,
            "/api/v1/hosts/web-01/series?item=disk.used_percent&range=1h&p_interface=eth0",
        )
        .await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn series_rejects_a_parameter_the_item_does_not_have(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.apply_example().await;
        let reply = get(
            &fx,
            "/api/v1/hosts/web-01/series?item=cpu.usage&range=1h&p_mount=/",
        )
        .await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
        assert_eq!(reply.error_code(), "invalid");
        assert!(String::from_utf8_lossy(&reply.body).contains("mount"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn series_bad_query_parameters_are_invalid(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.apply_example().await;
        let long = "a".repeat(257);
        let many: String = (0..17).map(|i| format!("&p_k{i}=v")).collect();
        for extra in [
            "&unknown=1".to_owned(),
            "&mount=/".to_owned(),
            "&p_=x".to_owned(),
            "&p_Mount=x".to_owned(),
            "&p_mo-unt=x".to_owned(),
            "&p_mo%7Bunt=x".to_owned(),
            "&p_mount=%00".to_owned(),
            "&p_mount=a%0Ab".to_owned(),
            "&p_mount=".to_owned(),
            format!("&p_mount={long}"),
            "&p_mount=/&p_mount=/data".to_owned(),
            "&item=cpu.usage".to_owned(),
            many,
        ] {
            // The mount is given unless the case is about the mount itself.
            let base = if extra.starts_with("&p_mount") {
                ""
            } else {
                "&p_mount=/"
            };
            let reply = get(
                &fx,
                &format!(
                    "/api/v1/hosts/web-01/series?item=disk.used_percent&range=1h{base}{extra}"
                ),
            )
            .await;
            assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{extra}");
            assert_eq!(reply.error_code(), "invalid", "{extra}");
        }
        let exact = "a".repeat(256);
        let reply = get(
            &fx,
            &format!("/api/v1/hosts/web-01/series?item=disk.used_percent&range=1h&p_mount={exact}"),
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn series_metrics_outage_is_503(pool: PgPool) {
        let fx = Fixture::new(pool).await;
        fx.apply_example().await;
        fx.metrics.set_failing(true);
        let reply = get(&fx, "/api/v1/hosts/web-01/series?item=cpu.usage&range=1h").await;
        assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(reply.error_code(), "metrics_unavailable");
        // Input is still checked first.
        let reply = get(&fx, "/api/v1/hosts/web-01/series?item=nope&range=1h").await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_hostile_parameter_value_stays_a_value_in_the_query(pool: PgPool) {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/query_range"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "success",
                "data": {"resultType": "matrix", "result": []}
            })))
            .mount(&server)
            .await;
        let mut fx = Fixture::new(pool).await;
        fx.apply_example().await;
        fx.use_metrics(Arc::new(
            crate::metrics::VictoriaMetrics::new(&server.uri()).unwrap(),
        ));
        // The value is `/"} or {__name__=~".+"} or x{a="`.
        let hostile = "%2F%22%7D%20or%20%7B__name__%3D~%22.%2B%22%7D%20or%20x%7Ba%3D%22";
        let reply = get(
            &fx,
            &format!(
                "/api/v1/hosts/web-01/series?item=disk.used_percent&range=1h&p_mount={hostile}"
            ),
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK);
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let query = requests[0]
            .url
            .query_pairs()
            .find(|(k, _)| k == "query")
            .map(|(_, v)| v.into_owned())
            .unwrap();
        assert_eq!(
            query,
            r#"cerbrix_disk_used_percent{host="web-01",p_mount="/\"} or {__name__=~\".+\"} or x{a=\""}"#
        );
    }
}
