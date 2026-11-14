//! Wires the collectors, the spool and the sender into the running agent.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::Context;
use cerbrix_common::MetricValue;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::client::ServerClient;
use crate::collectors::{
    CpuCollector, DiskCollector, LoadCollector, MemoryCollector, NetCollector, Patterns,
    UptimeCollector,
};
use crate::config::AgentConfig;
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

/// Runs the agent until `shutdown` is cancelled: collects values, spools them
/// and delivers them to the server. On shutdown the values still queued are
/// written to the spool before this returns.
pub async fn run(config: AgentConfig, shutdown: CancellationToken) -> anyhow::Result<()> {
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
    let scheduler = build_scheduler(&config, tx)?;
    let spool_dir = config.state_dir.join("spool");
    let spool = Spool::open(&spool_dir, config.spool_max_bytes)
        .with_context(|| format!("cannot open the spool in {}", spool_dir.display()))?;
    let spool = Arc::new(Mutex::new(spool));
    let client = ServerClient::new(&config)?;

    tracing::info!(host = %config.host, server = %config.server_url, "agent started");

    let writer = tokio::spawn(run_spool_writer(rx, Arc::clone(&spool)));
    let sender = tokio::spawn(run_sender(
        client,
        spool,
        credential,
        Enrollment {
            host: config.host.clone(),
            key: config.enrollment_key.clone(),
        },
        shutdown.clone(),
    ));

    scheduler.run(shutdown.clone()).await;
    // The scheduler is done and dropped its senders, so the writer drains what
    // is left in the channel and ends. Stop the sender too if the scheduler
    // ended on its own.
    shutdown.cancel();
    if let Err(e) = writer.await {
        tracing::error!(error = %e, "spool writer failed");
    }
    if let Err(e) = sender.await {
        tracing::error!(error = %e, "sender failed");
    }
    tracing::info!("agent stopped");
    Ok(())
}

fn build_scheduler(
    config: &AgentConfig,
    tx: mpsc::Sender<MetricValue>,
) -> anyhow::Result<Scheduler> {
    let i = &config.intervals;
    let exclude_mounts =
        Patterns::new(&config.disk.exclude_mounts).context("disk.exclude_mounts is not valid")?;
    let exclude_interfaces = Patterns::new(&config.net.exclude_interfaces)
        .context("net.exclude_interfaces is not valid")?;
    let mut scheduler = Scheduler::new(tx);
    scheduler.add(Box::new(CpuCollector::new()), i.cpu);
    scheduler.add(Box::new(LoadCollector::new()), i.load);
    scheduler.add(Box::new(MemoryCollector::new()), i.memory);
    scheduler.add(Box::new(UptimeCollector::new()), i.uptime);
    scheduler.add(
        Box::new(DiskCollector::new(
            config.disk.exclude_fs.clone(),
            exclude_mounts,
        )),
        i.disk,
    );
    scheduler.add(Box::new(NetCollector::new(exclude_interfaces)), i.net);
    Ok(scheduler)
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
        let dir = tempfile::tempdir().unwrap();
        let mut config = AgentConfig::for_tests(&server.uri(), dir.path());
        config.intervals.cpu = Duration::from_millis(200);
        config.intervals.load = Duration::from_millis(200);
        config.intervals.memory = Duration::from_millis(200);
        config.intervals.uptime = Duration::from_millis(200);
        config.intervals.disk = Duration::from_millis(200);
        config.intervals.net = Duration::from_millis(200);
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
