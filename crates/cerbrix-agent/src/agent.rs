//! Wires the collectors, the spool and the sender into the running agent.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::Context;
use cerbrix_common::MetricValue;
use cerbrix_common::agent_config::ItemConfig;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::client::ServerClient;
use crate::collector::Collector;
use crate::collectors::{
    CpuCollector, DiskCollector, FilteredCollector, LoadCollector, MemoryCollector, NetCollector,
    Patterns, UptimeCollector,
};
use crate::config::AgentConfig;
use crate::remote_config::{CollectorKind, ConfigCache, Group, plan};
use crate::scheduler::Scheduler;
use crate::sender::{CredentialStore, Enrollment, run_sender};
use crate::spool::Spool;

/// Values the scheduler may have queued before the spool writer takes them.
const CHANNEL_CAPACITY: usize = 1024;
/// Most values the spool writer stores in one call.
const WRITE_GROUP_VALUES: usize = 500;
/// Longest the spool writer waits for more values before storing a group.
const WRITE_GROUP_WAIT: Duration = Duration::from_secs(1);

/// The agent has no credential and no way to get one.
#[derive(Debug, thiserror::Error)]
#[error(
    "this host is not enrolled and no enrollment_key is configured: \
     set enrollment_key (or CERBRIX_AGENT_ENROLLMENT_KEY) for the first start"
)]
pub struct NotEnrolled;

/// How often the agent asks the server for its configuration.
pub const CONFIG_POLL: Duration = Duration::from_secs(60);
/// How long the poller waits before looking for a credential again while the
/// agent has none (the sender is still enrolling).
const NO_CREDENTIAL_RETRY: Duration = Duration::from_secs(1);

/// Runs the agent until `shutdown` is cancelled: collects what the server
/// configures, spools the values and delivers them to the server. On shutdown
/// the values still queued are written to the spool before this returns.
pub async fn run(config: AgentConfig, shutdown: CancellationToken) -> anyhow::Result<()> {
    run_with(config, CONFIG_POLL, shutdown).await
}

/// [`run`] with the configuration poll interval given.
pub async fn run_with(
    config: AgentConfig,
    poll: Duration,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let credential = CredentialStore::new(&config.state_dir);
    let stored = credential.load().with_context(|| {
        format!(
            "cannot read the credential in {}",
            config.state_dir.display()
        )
    })?;
    if stored.is_none() && config.enrollment_key.is_none() {
        return Err(NotEnrolled.into());
    }

    let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
    let spool_dir = config.state_dir.join("spool");
    let spool = Spool::open(&spool_dir, config.spool_max_bytes)
        .with_context(|| format!("cannot open the spool in {}", spool_dir.display()))?;
    let spool = Arc::new(Mutex::new(spool));
    let client = ServerClient::new(&config)?;
    let cache = ConfigCache::new(&config.state_dir);
    let (current_tx, current_rx) = watch::channel(cache.load());

    tracing::info!(host = %config.host, server = %config.server_url, "agent started");

    let writer = tokio::spawn(run_spool_writer(rx, Arc::clone(&spool)));
    let sender = tokio::spawn(run_sender(
        client.clone(),
        spool,
        credential.clone(),
        Enrollment {
            host: config.host.clone(),
            key: config.enrollment_key.clone(),
        },
        shutdown.clone(),
    ));
    let poller = tokio::spawn(run_config_poller(
        client,
        credential,
        cache,
        current_tx,
        poll,
        shutdown.clone(),
    ));

    // The writer and the channel to it outlive every scheduler; the channel
    // closes only when the supervisor returns and drops `tx`.
    supervise(&config, tx, current_rx, &shutdown).await;
    // Stop the sender and the poller too if the supervisor ended on its own.
    // The writer drains what is left in the channel and ends.
    shutdown.cancel();
    if let Err(e) = writer.await {
        tracing::error!(error = %e, "spool writer failed");
    }
    if let Err(e) = sender.await {
        tracing::error!(error = %e, "sender failed");
    }
    if let Err(e) = poller.await {
        tracing::error!(error = %e, "configuration poller failed");
    }
    tracing::info!("agent stopped");
    Ok(())
}

/// A scheduler running as a task, with the token that stops it.
struct RunningScheduler {
    stop: CancellationToken,
    task: JoinHandle<()>,
}

impl RunningScheduler {
    async fn stop(self) {
        self.stop.cancel();
        if let Err(e) = self.task.await {
            tracing::error!(error = %e, "scheduler failed");
        }
    }
}

/// Runs a scheduler for the current configuration and replaces it whenever the
/// configuration changes to another version. Returns after `shutdown`, once the
/// scheduler has stopped.
async fn supervise(
    config: &AgentConfig,
    tx: mpsc::Sender<MetricValue>,
    mut current: watch::Receiver<Option<ItemConfig>>,
    shutdown: &CancellationToken,
) {
    let mut running: Option<RunningScheduler> = None;
    let mut version: Option<String> = None;
    if current.borrow().is_none() {
        tracing::info!("waiting for the configuration from the server");
    }
    loop {
        let latest = current.borrow_and_update().clone();
        if let Some(item_config) = latest
            && version.as_deref() != Some(item_config.version.as_str())
        {
            let groups = plan(&item_config.items);
            if groups.is_empty() {
                tracing::warn!(
                    version = %item_config.version,
                    "the configuration has nothing the agent can collect, collection stopped"
                );
                if let Some(old) = running.take() {
                    old.stop().await;
                }
                version = Some(item_config.version);
            } else {
                match build_scheduler(config, &groups, tx.clone()) {
                    Ok(scheduler) => {
                        if let Some(old) = running.take() {
                            old.stop().await;
                        }
                        let stop = shutdown.child_token();
                        let task = tokio::spawn(scheduler.run(stop.clone()));
                        tracing::info!(
                            version = %item_config.version,
                            collectors = groups.len(),
                            "collecting according to the configuration"
                        );
                        running = Some(RunningScheduler { stop, task });
                        version = Some(item_config.version);
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "cannot apply the new configuration, keeping the running one");
                    }
                }
            }
        }
        tokio::select! {
            () = shutdown.cancelled() => break,
            changed = current.changed() => {
                if changed.is_err() {
                    // The poller is gone; keep collecting until shutdown.
                    shutdown.cancelled().await;
                    break;
                }
            }
        }
    }
    if let Some(old) = running.take() {
        old.stop().await;
    }
}

/// Builds a scheduler with one filtered collector per group. Every group gets
/// its own collector instance, so rate-based collectors keep separate state per
/// interval. `config` is the local file: it supplies the exclusion patterns.
pub fn build_scheduler(
    config: &AgentConfig,
    groups: &[Group],
    tx: mpsc::Sender<MetricValue>,
) -> anyhow::Result<Scheduler> {
    let mut scheduler = Scheduler::new(tx);
    for group in groups {
        let collector: Box<dyn Collector> = match group.collector {
            CollectorKind::Cpu => Box::new(CpuCollector::new()),
            CollectorKind::Load => Box::new(LoadCollector::new()),
            CollectorKind::Memory => Box::new(MemoryCollector::new()),
            CollectorKind::Uptime => Box::new(UptimeCollector::new()),
            CollectorKind::Disk => Box::new(DiskCollector::new(
                config.disk.exclude_fs.clone(),
                Patterns::new(&config.disk.exclude_mounts)
                    .context("disk.exclude_mounts is not valid")?,
            )),
            CollectorKind::Net => Box::new(NetCollector::new(
                Patterns::new(&config.net.exclude_interfaces)
                    .context("net.exclude_interfaces is not valid")?,
            )),
        };
        scheduler.add(
            Box::new(FilteredCollector::new(collector, group.wanted.clone())),
            group.interval,
        );
    }
    Ok(scheduler)
}

/// Fetches the configuration at start and then every `poll`, and publishes a
/// new one on `current` after saving it to `cache`.
///
/// The round is skipped while the agent has no credential (the sender enrols);
/// the poller then looks again after a second. Errors leave the current
/// configuration in place; a streak of failures is logged once.
pub async fn run_config_poller(
    client: ServerClient,
    credential: CredentialStore,
    cache: ConfigCache,
    current: watch::Sender<Option<ItemConfig>>,
    poll: Duration,
    shutdown: CancellationToken,
) {
    let mut failing = false;
    loop {
        let wait = match credential.load() {
            Ok(Some(credential)) => {
                let known = current.borrow().as_ref().map(|c| c.version.clone());
                let fetched = tokio::select! {
                    () = shutdown.cancelled() => return,
                    fetched = client.fetch_config(&credential, known.as_deref()) => fetched,
                };
                match fetched {
                    Ok(Some(received)) => {
                        failing = false;
                        let unchanged = current
                            .borrow()
                            .as_ref()
                            .is_some_and(|c| c.version == received.version);
                        if !unchanged {
                            if let Err(e) = cache.save(&received) {
                                tracing::warn!(error = %e, "cannot cache the configuration");
                            }
                            current.send_replace(Some(received));
                        }
                    }
                    Ok(None) => {
                        failing = false;
                        if known.is_none() {
                            tracing::warn!(
                                "the server reports no change but the agent has no configuration, fetching again"
                            );
                        }
                    }
                    Err(e) => {
                        if !failing {
                            tracing::warn!(error = %e, "cannot fetch the configuration, keeping the current one");
                        }
                        failing = true;
                    }
                }
                poll
            }
            Ok(None) => NO_CREDENTIAL_RETRY.min(poll),
            Err(e) => {
                tracing::warn!(error = %e, "cannot read the credential");
                poll
            }
        };
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(wait) => {}
        }
    }
}

/// Moves values from the channel into the spool, in groups, until the channel
/// is closed and empty.
///
/// A failed write drops that group: the spool stores nothing in that case and
/// retrying in a loop would only back the collectors up.
pub async fn run_spool_writer(mut rx: mpsc::Receiver<MetricValue>, spool: Arc<Mutex<Spool>>) {
    let mut group = Vec::with_capacity(WRITE_GROUP_VALUES);
    loop {
        let Some(first) = rx.recv().await else {
            return;
        };
        group.push(first);
        let deadline = tokio::time::sleep(WRITE_GROUP_WAIT);
        tokio::pin!(deadline);
        let mut closed = false;
        while group.len() < WRITE_GROUP_VALUES {
            tokio::select! {
                value = rx.recv() => match value {
                    Some(value) => group.push(value),
                    None => {
                        closed = true;
                        break;
                    }
                },
                () = &mut deadline => break,
            }
        }
        let result = spool
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(&group);
        if let Err(e) = result {
            tracing::error!(error = %e, dropped = group.len(), "cannot write to the spool, dropping values");
        }
        group.clear();
        if closed {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::Instant;

    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn value(i: i64) -> MetricValue {
        MetricValue {
            key: "cpu.usage".into(),
            params: BTreeMap::new(),
            ts_ms: 1_700_000_000_000 + i,
            value: i as f64,
        }
    }

    #[tokio::test]
    async fn spool_writer_stores_everything_and_flushes_on_close() {
        let dir = tempfile::tempdir().unwrap();
        let spool = Arc::new(Mutex::new(
            Spool::open(&dir.path().join("spool"), 8 * 1024 * 1024).unwrap(),
        ));
        let (tx, rx) = mpsc::channel(16);
        let writer = tokio::spawn(run_spool_writer(rx, Arc::clone(&spool)));
        for i in 0..1200 {
            tx.send(value(i)).await.unwrap();
        }
        drop(tx);
        tokio::time::timeout(Duration::from_secs(5), writer)
            .await
            .expect("writer did not finish")
            .unwrap();
        let stored = spool.lock().unwrap().peek(2000).unwrap();
        let expected: Vec<_> = (0..1200).map(value).collect();
        assert_eq!(stored, expected);
    }

    #[tokio::test]
    async fn build_scheduler_one_entry_per_group() {
        use crate::remote_config::{CollectorKind, Group, Wanted};

        let dir = tempfile::tempdir().unwrap();
        let config = AgentConfig::for_tests("http://127.0.0.1:1", dir.path());
        let wanted = |key: &str| Wanted {
            key: key.to_owned(),
            params: None,
            except: Vec::new(),
        };
        let groups = [
            Group {
                collector: CollectorKind::Cpu,
                interval: Duration::from_secs(30),
                wanted: vec![wanted("cpu.usage")],
            },
            Group {
                collector: CollectorKind::Cpu,
                interval: Duration::from_secs(60),
                wanted: vec![wanted("cpu.usage")],
            },
        ];
        let (tx, _rx) = mpsc::channel(4);
        let scheduler = build_scheduler(&config, &groups, tx).unwrap();
        assert_eq!(scheduler.len(), 2);
    }
    #[tokio::test]
    async fn run_refuses_to_start_without_credential_and_key() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = AgentConfig::for_tests("http://127.0.0.1:1", dir.path());
        config.enrollment_key = None;
        let err = run(config, CancellationToken::new()).await.unwrap_err();
        assert!(err.downcast_ref::<NotEnrolled>().is_some());
        assert!(err.to_string().contains("enrollment_key"));
    }

    #[tokio::test]
    async fn agent_enrols_collects_and_delivers() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/agent/v1/enroll"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "credential": "agent-cred" })),
            )
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
        Mock::given(method("GET"))
            .and(path("/api/agent/v1/config"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "version": "v1",
                "items": [{ "key": "memory.used_percent", "params": null, "interval_secs": 1 }]
            })))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let config = AgentConfig::for_tests(&server.uri(), dir.path());
        let shutdown = CancellationToken::new();
        let agent = tokio::spawn(run(config, shutdown.clone()));

        let start = Instant::now();
        let keys = loop {
            let requests = server.received_requests().await.unwrap();
            let keys: Vec<String> = requests
                .iter()
                .filter(|r| r.url.path().ends_with("/values"))
                .flat_map(|r| {
                    let batch: cerbrix_common::ValueBatch =
                        serde_json::from_slice(&r.body).unwrap();
                    batch.values.into_iter().map(|v| v.key)
                })
                .collect();
            if keys.iter().any(|k| k == "memory.used_percent")
                || start.elapsed() > Duration::from_secs(15)
            {
                break keys;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        assert!(
            keys.iter().any(|k| k.starts_with("memory.")),
            "no memory values delivered: {keys:?}"
        );
        let requests = server.received_requests().await.unwrap();
        let sent = requests
            .iter()
            .find(|r| r.url.path().ends_with("/values"))
            .unwrap();
        assert_eq!(
            sent.headers.get("authorization").unwrap().to_str().unwrap(),
            "Bearer agent-cred"
        );

        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(10), agent)
            .await
            .expect("agent did not stop")
            .unwrap()
            .unwrap();
        assert_eq!(
            CredentialStore::new(dir.path()).load().unwrap().as_deref(),
            Some("agent-cred")
        );
    }
}
