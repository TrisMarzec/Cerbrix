//! The agent measures what the server configures and follows changes while running.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cerbrix_agent::agent::run_with;
use cerbrix_agent::config::AgentConfig;
use cerbrix_agent::remote_config::ConfigCache;
use cerbrix_agent::sender::CredentialStore;
use cerbrix_agent::spool::Spool;
use cerbrix_common::agent_config::{ConfiguredItem, ItemConfig};
use cerbrix_common::{MetricValue, ValueBatch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const POLL: Duration = Duration::from_millis(200);

/// What the mock server answers on the configuration route.
#[derive(Clone)]
enum Answer {
    Config(ItemConfig),
    Fail,
}

struct ConfigRoute(Arc<Mutex<Answer>>);

impl Respond for ConfigRoute {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        match &*self.0.lock().unwrap() {
            Answer::Fail => ResponseTemplate::new(500),
            Answer::Config(config) => {
                let known = request
                    .headers
                    .get("if-none-match")
                    .and_then(|v| v.to_str().ok());
                if known == Some(format!("\"{}\"", config.version).as_str()) {
                    ResponseTemplate::new(304)
                } else {
                    ResponseTemplate::new(200).set_body_json(config)
                }
            }
        }
    }
}

fn config(version: &str, items: &[(&str, u64)]) -> ItemConfig {
    ItemConfig {
        version: version.to_owned(),
        items: items
            .iter()
            .map(|(key, secs)| ConfiguredItem {
                key: (*key).to_owned(),
                params: None,
                interval_secs: *secs,
            })
            .collect(),
    }
}

async fn server_with(initial: Answer) -> (MockServer, Arc<Mutex<Answer>>) {
    let server = MockServer::start().await;
    let answer = Arc::new(Mutex::new(initial));
    Mock::given(method("GET"))
        .and(path("/api/agent/v1/config"))
        .respond_with(ConfigRoute(Arc::clone(&answer)))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/agent/v1/values"))
        .respond_with(
            ResponseTemplate::new(202)
                .set_body_json(serde_json::json!({ "accepted": 1, "rejected": 0 })),
        )
        .mount(&server)
        .await;
    (server, answer)
}

fn agent_config(server_url: &str, state_dir: &Path) -> AgentConfig {
    let file = state_dir.join("agent.toml");
    std::fs::write(
        &file,
        format!(
            "server_url = \"{server_url}\"\nhost = \"web-01\"\nallow_insecure_http = true\n\
             state_dir = '{}'\n",
            state_dir.display()
        ),
    )
    .unwrap();
    AgentConfig::load(Some(&file)).unwrap()
}

struct Running {
    shutdown: CancellationToken,
    agent: JoinHandle<anyhow::Result<()>>,
}

impl Running {
    async fn stop(self) {
        self.shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(30), self.agent)
            .await
            .expect("agent did not stop")
            .unwrap()
            .unwrap();
    }
}

fn start(server_url: &str, state_dir: &Path) -> Running {
    CredentialStore::new(state_dir).save("agent-cred").unwrap();
    let shutdown = CancellationToken::new();
    let agent = tokio::spawn(run_with(
        agent_config(server_url, state_dir),
        POLL,
        shutdown.clone(),
    ));
    Running { shutdown, agent }
}

async fn delivered(server: &MockServer) -> Vec<MetricValue> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path().ends_with("/values"))
        .flat_map(|r| {
            serde_json::from_slice::<ValueBatch>(&r.body)
                .unwrap()
                .values
        })
        .collect()
}

async fn wait_for<F: Fn(&[MetricValue]) -> bool>(
    server: &MockServer,
    what: &str,
    condition: F,
) -> Vec<MetricValue> {
    let start = Instant::now();
    loop {
        let values = delivered(server).await;
        if condition(&values) {
            return values;
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "timed out waiting for {what}: {values:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn has(values: &[MetricValue], key: &str) -> bool {
    values.iter().any(|v| v.key == key)
}

#[tokio::test]
async fn collects_what_the_server_configures() {
    let (server, _) = server_with(Answer::Config(config(
        "v1",
        &[("system.uptime_seconds", 1)],
    )))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let running = start(&server.uri(), dir.path());

    let values = wait_for(&server, "uptime", |v| has(v, "system.uptime_seconds")).await;
    assert!(!has(&values, "cpu.usage"), "{values:?}");
    running.stop().await;
    let values = delivered(&server).await;
    assert!(values.iter().all(|v| v.key == "system.uptime_seconds"));
}

#[tokio::test]
async fn reconfigures_without_restart() {
    let (server, answer) = server_with(Answer::Config(config(
        "v1",
        &[("system.uptime_seconds", 1)],
    )))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let running = start(&server.uri(), dir.path());
    wait_for(&server, "uptime", |v| has(v, "system.uptime_seconds")).await;

    let switched = now_ms();
    *answer.lock().unwrap() = Answer::Config(config("v2", &[("memory.used_percent", 1)]));
    // Memory values collected 2.5 s after the switch prove the new scheduler
    // runs; a leftover uptime collector would have produced values by then too.
    let values = wait_for(&server, "memory after the switch", |v| {
        v.iter()
            .any(|x| x.key == "memory.used_percent" && x.ts_ms > switched + 2500)
    })
    .await;
    running.stop().await;

    let late_uptime: Vec<_> = values
        .iter()
        .filter(|v| v.key == "system.uptime_seconds" && v.ts_ms > switched + 2000)
        .collect();
    assert!(late_uptime.is_empty(), "{late_uptime:?}");
    assert_eq!(ConfigCache::new(dir.path()).load().unwrap().version, "v2");
}

#[tokio::test]
async fn keeps_last_configuration_when_server_fails() {
    let (server, answer) = server_with(Answer::Config(config(
        "v1",
        &[("system.uptime_seconds", 1)],
    )))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let running = start(&server.uri(), dir.path());
    wait_for(&server, "uptime", |v| has(v, "system.uptime_seconds")).await;

    *answer.lock().unwrap() = Answer::Fail;
    let failed = now_ms();
    wait_for(&server, "values after the failure", |v| {
        v.iter()
            .any(|x| x.key == "system.uptime_seconds" && x.ts_ms > failed + 1500)
    })
    .await;
    running.stop().await;
}

#[tokio::test]
async fn starts_from_cache_when_server_is_down() {
    let dir = tempfile::tempdir().unwrap();
    ConfigCache::new(dir.path())
        .save(&config("cached", &[("system.uptime_seconds", 1)]))
        .unwrap();
    let running = start("http://127.0.0.1:1", dir.path());
    tokio::time::sleep(Duration::from_millis(2500)).await;
    running.stop().await;

    let mut spool = Spool::open(&dir.path().join("spool"), 8 * 1024 * 1024).unwrap();
    let stored = spool.peek(100).unwrap();
    assert!(!stored.is_empty());
    assert!(stored.iter().all(|v| v.key == "system.uptime_seconds"));
}

#[tokio::test]
async fn no_cache_and_no_server_collects_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let running = start("http://127.0.0.1:1", dir.path());
    tokio::time::sleep(Duration::from_secs(3)).await;
    running.stop().await;

    let mut spool = Spool::open(&dir.path().join("spool"), 8 * 1024 * 1024).unwrap();
    assert!(spool.peek(100).unwrap().is_empty());
}

#[tokio::test]
async fn unknown_item_from_newer_server_is_skipped() {
    let (server, _) = server_with(Answer::Config(config(
        "v1",
        &[("gpu.usage", 1), ("system.uptime_seconds", 1)],
    )))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let running = start(&server.uri(), dir.path());
    let values = wait_for(&server, "uptime", |v| has(v, "system.uptime_seconds")).await;
    assert!(!has(&values, "gpu.usage"));
    running.stop().await;
}
