//! VictoriaMetrics backend: JSON line import and the Prometheus query API.

use std::collections::BTreeMap;
use std::time::Duration;

use cerbrix_common::{MetricValue, validate_item_key, validate_param_name};
use reqwest::{Client, Response, StatusCode, Url};
use serde::Deserialize;

use super::{MetricsError, MetricsStore, RangeQuery, Sample};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Metrics store backed by a VictoriaMetrics single-node server.
#[derive(Debug, Clone)]
pub struct VictoriaMetrics {
    client: Client,
    base: Url,
}

impl VictoriaMetrics {
    /// `base_url` is the server root, e.g. `http://localhost:8428`.
    pub fn new(base_url: &str) -> Result<Self, MetricsError> {
        let mut base = Url::parse(base_url)
            .map_err(|e| MetricsError::Rejected(format!("invalid base URL: {e}")))?;
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        let client = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| MetricsError::Unavailable(format!("cannot build HTTP client: {e}")))?;
        Ok(Self { client, base })
    }

    fn endpoint(&self, path: &str) -> Result<Url, MetricsError> {
        self.base
            .join(path)
            .map_err(|e| MetricsError::Rejected(format!("invalid endpoint URL: {e}")))
    }
}

/// `disk.used_percent` becomes `cerbrix_disk_used_percent`.
fn series_name(key: &str) -> String {
    format!("cerbrix_{}", key.replace('.', "_"))
}

/// Escapes a label value for use inside a double-quoted PromQL string.
fn escape_selector_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out
}

/// The store does not trust its callers: key and param names end up in series
/// names and in PromQL selectors, so they must match the shared wire rules.
fn validate_series<'a>(
    key: &str,
    param_names: impl Iterator<Item = &'a String>,
) -> Result<(), MetricsError> {
    validate_item_key(key).map_err(|e| MetricsError::Rejected(e.to_string()))?;
    for name in param_names {
        validate_param_name(name).map_err(|e| MetricsError::Rejected(e.to_string()))?;
    }
    Ok(())
}

fn selector(query: &RangeQuery) -> Result<String, MetricsError> {
    validate_series(&query.key, query.params.keys())?;
    let mut s = format!(
        "{}{{host=\"{}\"",
        series_name(&query.key),
        escape_selector_value(&query.host)
    );
    for (name, value) in &query.params {
        s.push_str(&format!(",p_{name}=\"{}\"", escape_selector_value(value)));
    }
    s.push('}');
    Ok(s)
}

fn import_line(host: &str, v: &MetricValue) -> Result<String, MetricsError> {
    validate_series(&v.key, v.params.keys())?;
    if !v.value.is_finite() {
        return Err(MetricsError::Rejected(format!(
            "value of {} is not finite",
            v.key
        )));
    }
    let mut metric = BTreeMap::new();
    metric.insert("__name__".to_owned(), series_name(&v.key));
    metric.insert("host".to_owned(), host.to_owned());
    for (name, value) in &v.params {
        metric.insert(format!("p_{name}"), value.clone());
    }
    let line = serde_json::json!({
        "metric": metric,
        "values": [v.value],
        "timestamps": [v.ts_ms],
    });
    Ok(line.to_string())
}

/// Seconds as VictoriaMetrics expects them; may be fractional.
fn secs(ms: i64) -> String {
    (ms as f64 / 1000.0).to_string()
}

fn send_error(e: reqwest::Error) -> MetricsError {
    MetricsError::Unavailable(e.without_url().to_string())
}

/// Maps a non-success status to the error kind; passes success through.
async fn check_status(response: Response) -> Result<Response, MetricsError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    let body: String = body.chars().take(500).collect();
    let msg = format!("HTTP {status}: {body}");
    if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
        Err(MetricsError::Unavailable(msg))
    } else {
        Err(MetricsError::Rejected(msg))
    }
}

#[derive(Deserialize)]
struct QueryResponse {
    status: String,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    data: Option<QueryData>,
}

#[derive(Deserialize)]
struct QueryData {
    result: Vec<Series>,
}

#[derive(Deserialize)]
struct Series {
    values: Vec<(f64, String)>,
}

#[async_trait::async_trait]
impl MetricsStore for VictoriaMetrics {
    async fn write(&self, host: &str, values: &[MetricValue]) -> Result<(), MetricsError> {
        if values.is_empty() {
            return Ok(());
        }
        let mut body = String::new();
        for v in values {
            body.push_str(&import_line(host, v)?);
            body.push('\n');
        }
        let response = self
            .client
            .post(self.endpoint("api/v1/import")?)
            .body(body)
            .send()
            .await
            .map_err(send_error)?;
        check_status(response).await?;
        Ok(())
    }

    async fn query_range(&self, query: &RangeQuery) -> Result<Vec<Sample>, MetricsError> {
        let response = self
            .client
            .get(self.endpoint("api/v1/query_range")?)
            .query(&[
                ("query", selector(query)?),
                ("start", secs(query.from_ms)),
                ("end", secs(query.to_ms)),
                ("step", query.step_secs.to_string()),
            ])
            .send()
            .await
            .map_err(send_error)?;
        let parsed: QueryResponse = check_status(response)
            .await?
            .json()
            .await
            .map_err(|e| MetricsError::Unavailable(format!("unreadable response: {e}")))?;
        if parsed.status != "success" {
            return Err(MetricsError::Rejected(
                parsed.error.unwrap_or_else(|| "query failed".into()),
            ));
        }
        let mut samples: Vec<Sample> = parsed
            .data
            .map(|d| d.result)
            .unwrap_or_default()
            .into_iter()
            .flat_map(|s| s.values)
            .filter_map(|(ts, value)| {
                let value: f64 = value.parse().ok().filter(|v: &f64| v.is_finite())?;
                Some(Sample {
                    ts_ms: (ts * 1000.0).round() as i64,
                    value,
                })
            })
            .collect();
        samples.sort_by_key(|s| s.ts_ms);
        Ok(samples)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use cerbrix_common::MetricValue;
    use serde_json::{Value, json};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::metrics::{MetricsError, MetricsStore, RangeQuery, Sample, VictoriaMetrics};

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

    fn range(params: &[(&str, &str)]) -> RangeQuery {
        RangeQuery {
            host: "web-01".into(),
            key: "disk.used_percent".into(),
            params: params
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>(),
            from_ms: 1_700_000_000_000,
            to_ms: 1_700_000_060_000,
            step_secs: 30,
        }
    }

    async fn import_lines(server: &MockServer) -> Vec<Value> {
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        std::str::from_utf8(&requests[0].body)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    async fn import_server(status: u16) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/import"))
            .respond_with(ResponseTemplate::new(status))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn write_sends_one_import_line_per_value() {
        let server = import_server(204).await;
        let vm = VictoriaMetrics::new(&server.uri()).unwrap();
        vm.write(
            "web-01",
            &[
                mv("cpu.usage", &[], 1_700_000_000_000, 12.5),
                mv("cpu.usage", &[], 1_700_000_015_000, 13.0),
            ],
        )
        .await
        .unwrap();
        let lines = import_lines(&server).await;
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0],
            json!({
                "metric": {"__name__": "cerbrix_cpu_usage", "host": "web-01"},
                "values": [12.5],
                "timestamps": [1_700_000_000_000_i64]
            })
        );
    }

    #[tokio::test]
    async fn params_become_prefixed_labels() {
        let server = import_server(204).await;
        let vm = VictoriaMetrics::new(&server.uri()).unwrap();
        vm.write(
            "web-01",
            &[mv("disk.used_percent", &[("mount", "/")], 1, 50.0)],
        )
        .await
        .unwrap();
        let lines = import_lines(&server).await;
        assert_eq!(
            lines[0]["metric"],
            json!({"__name__": "cerbrix_disk_used_percent", "host": "web-01", "p_mount": "/"})
        );
    }

    #[tokio::test]
    async fn odd_label_values_survive_write_and_query() {
        let mount = r#"/mnt/"my" disk\x"#;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/import"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/query_range"))
            .and(query_param(
                "query",
                r#"cerbrix_disk_used_percent{host="web-01",p_mount="/mnt/\"my\" disk\\x"}"#,
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "success",
                "data": {"resultType": "matrix", "result": []}
            })))
            .expect(1)
            .mount(&server)
            .await;
        let vm = VictoriaMetrics::new(&server.uri()).unwrap();
        vm.write(
            "web-01",
            &[mv("disk.used_percent", &[("mount", mount)], 1, 1.0)],
        )
        .await
        .unwrap();
        let requests = server.received_requests().await.unwrap();
        let line: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(line["metric"]["p_mount"], mount);
        let got = vm.query_range(&range(&[("mount", mount)])).await.unwrap();
        assert!(got.is_empty());
    }

    #[tokio::test]
    async fn query_range_parses_matrix_result() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/query_range"))
            .and(query_param("start", "1700000000"))
            .and(query_param("end", "1700000060"))
            .and(query_param("step", "30"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": "success",
                "data": {"resultType": "matrix", "result": [{
                    "metric": {"__name__": "cerbrix_disk_used_percent"},
                    "values": [[1_700_000_000, "12.5"], [1_700_000_030.5, "13"]]
                }]}
            })))
            .mount(&server)
            .await;
        let vm = VictoriaMetrics::new(&server.uri()).unwrap();
        let got = vm.query_range(&range(&[])).await.unwrap();
        assert_eq!(
            got,
            vec![
                Sample {
                    ts_ms: 1_700_000_000_000,
                    value: 12.5
                },
                Sample {
                    ts_ms: 1_700_000_030_500,
                    value: 13.0
                }
            ]
        );
    }

    #[tokio::test]
    async fn server_error_is_unavailable() {
        let server = import_server(500).await;
        let vm = VictoriaMetrics::new(&server.uri()).unwrap();
        let err = vm
            .write("web-01", &[mv("cpu.usage", &[], 1, 1.0)])
            .await
            .unwrap_err();
        assert!(matches!(err, MetricsError::Unavailable(_)), "{err:?}");
    }

    #[tokio::test]
    async fn bad_request_is_rejected() {
        let server = import_server(400).await;
        let vm = VictoriaMetrics::new(&server.uri()).unwrap();
        let err = vm
            .write("web-01", &[mv("cpu.usage", &[], 1, 1.0)])
            .await
            .unwrap_err();
        assert!(matches!(err, MetricsError::Rejected(_)), "{err:?}");
    }

    #[tokio::test]
    async fn empty_write_makes_no_request() {
        let server = MockServer::start().await;
        let vm = VictoriaMetrics::new(&server.uri()).unwrap();
        vm.write("web-01", &[]).await.unwrap();
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    async fn no_request_server() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::any())
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn hostile_key_never_reaches_the_server() {
        let server = no_request_server().await;
        let vm = VictoriaMetrics::new(&server.uri()).unwrap();
        for key in [r#"x{host=~".+"} or cerbrix_y"#, "a}", "A.b", "a..b", ""] {
            let err = vm
                .write("web-01", &[mv(key, &[], 1, 1.0)])
                .await
                .unwrap_err();
            assert!(matches!(err, MetricsError::Rejected(_)), "{key}: {err:?}");
            let mut q = range(&[]);
            q.key = key.into();
            let err = vm.query_range(&q).await.unwrap_err();
            assert!(matches!(err, MetricsError::Rejected(_)), "{key}: {err:?}");
        }
        server.verify().await;
    }

    #[tokio::test]
    async fn hostile_param_name_never_reaches_the_server() {
        let server = no_request_server().await;
        let vm = VictoriaMetrics::new(&server.uri()).unwrap();
        for name in [r#"m=~".+",x"#, "a}", "a,b", "a\"b", "Mount", ""] {
            let err = vm
                .write("web-01", &[mv("cpu.usage", &[(name, "v")], 1, 1.0)])
                .await
                .unwrap_err();
            assert!(matches!(err, MetricsError::Rejected(_)), "{name}: {err:?}");
            let err = vm.query_range(&range(&[(name, "v")])).await.unwrap_err();
            assert!(matches!(err, MetricsError::Rejected(_)), "{name}: {err:?}");
        }
        server.verify().await;
    }

    #[tokio::test]
    async fn unreachable_server_is_unavailable() {
        // Bind an ephemeral port, then release it so nothing listens there.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let vm = VictoriaMetrics::new(&format!("http://127.0.0.1:{port}")).unwrap();
        let err = vm
            .write("web-01", &[mv("cpu.usage", &[], 1, 1.0)])
            .await
            .unwrap_err();
        assert!(matches!(err, MetricsError::Unavailable(_)), "{err:?}");
    }

    /// Needs the dev compose stack: `docker compose -f docker-compose.dev.yml up -d`.
    #[tokio::test]
    #[ignore = "needs VictoriaMetrics on localhost:8428"]
    async fn roundtrip_against_real_victoriametrics() {
        let base = "http://localhost:8428";
        let vm = VictoriaMetrics::new(base).unwrap();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        // Unique per run so earlier runs cannot satisfy the assertions.
        let host = format!("it-{now_ms}");
        let mount = r#"/mnt/"my" disk\x"#;
        // VictoriaMetrics hides the newest 30 s from queries (-search.latencyOffset),
        // so use samples older than that.
        let t1 = now_ms - 120_000;
        let t2 = now_ms - 90_000;
        vm.write(
            &host,
            &[
                mv("disk.used_percent", &[("mount", mount)], t1, 41.5),
                mv("disk.used_percent", &[("mount", mount)], t2, 42.5),
            ],
        )
        .await
        .unwrap();

        // Freshly imported data becomes searchable only after a short delay;
        // force a flush instead of sleeping.
        reqwest::get(format!("{base}/internal/force_flush"))
            .await
            .unwrap()
            .error_for_status()
            .unwrap();

        let query = RangeQuery {
            host: host.clone(),
            key: "disk.used_percent".into(),
            params: BTreeMap::from([("mount".to_owned(), mount.to_owned())]),
            from_ms: t1,
            to_ms: t2 + 10_000,
            step_secs: 5,
        };
        let samples = vm.query_range(&query).await.unwrap();
        assert!(samples.windows(2).all(|w| w[0].ts_ms <= w[1].ts_ms));
        assert_eq!(samples.first().map(|s| s.value), Some(41.5), "{samples:?}");
        assert_eq!(samples.last().map(|s| s.value), Some(42.5), "{samples:?}");
        assert!(samples.iter().all(|s| s.value == 41.5 || s.value == 42.5));

        // A different host sees nothing.
        let other = RangeQuery {
            host: format!("{host}-other"),
            ..query
        };
        assert!(vm.query_range(&other).await.unwrap().is_empty());
    }
}
