//! Delivery of spooled values to the server, and the stored credential.
//!
//! Delivery is at-least-once: a batch is acknowledged in the spool only after
//! the server answered 202, so a crash in between sends it a second time.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use cerbrix_common::MetricValue;
use tokio_util::sync::CancellationToken;

use crate::client::{ClientError, ServerClient};
use crate::spool::Spool;

/// Most values in one request.
const MAX_BATCH_VALUES: usize = 1000;
/// Serialised size one request stays below. The server accepts 2 MiB; this
/// leaves room for the envelope and for a server configured a bit lower.
const MAX_BODY_BYTES: usize = 1_500_000;

/// Delays between attempts that follow each other: 1 s, 2 s, 4 s, ... up to 60 s.
#[derive(Debug, Clone)]
pub struct Backoff {
    start: Duration,
    max: Duration,
    next: Duration,
}

impl Backoff {
    pub fn new() -> Self {
        Self::with_limits(Duration::from_secs(1), Duration::from_secs(60))
    }

    pub fn with_limits(start: Duration, max: Duration) -> Self {
        Self {
            start,
            max,
            next: start,
        }
    }

    /// The delay to wait now; the one after it is twice as long, up to the cap.
    pub fn next_delay(&mut self) -> Duration {
        let delay = self.next;
        self.next = (self.next * 2).min(self.max);
        delay
    }

    /// Starts over at the initial delay, after a success.
    pub fn reset(&mut self) {
        self.next = self.start;
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

/// The agent credential, kept in `<state_dir>/credential`.
#[derive(Debug, Clone)]
pub struct CredentialStore {
    path: PathBuf,
}

impl CredentialStore {
    pub fn new(state_dir: &Path) -> Self {
        Self {
            path: state_dir.join("credential"),
        }
    }

    /// The stored credential, or `None` if there is none.
    pub fn load(&self) -> io::Result<Option<String>> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => {
                let text = text.trim();
                Ok((!text.is_empty()).then(|| text.to_owned()))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Stores the credential atomically; on Unix the file is created readable
    /// by its owner only.
    pub fn save(&self, credential: &str) -> io::Result<()> {
        use std::io::Write;

        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("tmp");
        match std::fs::remove_file(&tmp) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let result = (|| {
            let mut file = options.open(&tmp)?;
            file.write_all(credential.as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&tmp, &self.path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }

    /// Keeps a credential the server rejected as `credential.rejected`
    /// (replacing an older one) so that it is no longer used but not lost.
    /// Succeeds if there is no credential.
    pub fn reject(&self) -> io::Result<()> {
        match std::fs::rename(&self.path, self.path.with_extension("rejected")) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Deletes the stored credential. Succeeds if there is none.
    pub fn clear(&self) -> io::Result<()> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// What the sender needs to enrol.
#[derive(Clone)]
pub struct Enrollment {
    pub host: String,
    pub key: Option<String>,
}

// Manual Debug: the enrolment key must never reach logs.
impl fmt::Debug for Enrollment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Enrollment")
            .field("host", &self.host)
            .field("key", &self.key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Timing of the sender; the defaults are the production values.
#[derive(Debug, Clone)]
pub struct Tuning {
    pub backoff_start: Duration,
    pub backoff_max: Duration,
    /// Pause between enrolment attempts that the server refused.
    pub refused_retry: Duration,
    /// Pause when the spool has nothing to send.
    pub idle: Duration,
    /// Pause after the server refused a batch, before the next one is sent.
    pub refused_pause: Duration,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            backoff_start: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            refused_retry: Duration::from_secs(60),
            idle: Duration::from_secs(1),
            refused_pause: Duration::from_secs(1),
        }
    }
}

/// Enrols if needed, then sends spooled values until `shutdown` is cancelled.
/// Values stay in the spool whenever the server cannot take them.
pub async fn run_sender(
    client: ServerClient,
    spool: Arc<Mutex<Spool>>,
    credential: CredentialStore,
    enrollment: Enrollment,
    shutdown: CancellationToken,
) {
    run_sender_with(
        client,
        spool,
        credential,
        enrollment,
        shutdown,
        Tuning::default(),
    )
    .await;
}

pub async fn run_sender_with(
    client: ServerClient,
    spool: Arc<Mutex<Spool>>,
    credential: CredentialStore,
    enrollment: Enrollment,
    shutdown: CancellationToken,
    tuning: Tuning,
) {
    let sender = Sender {
        client,
        spool,
        store: credential,
        enrollment,
        shutdown,
        tuning,
    };
    sender.run().await;
}

struct Sender {
    client: ServerClient,
    spool: Arc<Mutex<Spool>>,
    store: CredentialStore,
    enrollment: Enrollment,
    shutdown: CancellationToken,
    tuning: Tuning,
}

enum Outcome {
    Shutdown,
    Reenrol,
}

impl Sender {
    async fn run(&self) {
        loop {
            let Some(credential) = self.obtain_credential().await else {
                return;
            };
            match self.deliver(&credential).await {
                Outcome::Shutdown => return,
                Outcome::Reenrol => {}
            }
        }
    }

    /// The stored credential, or a new one from enrolment. `None` on shutdown.
    async fn obtain_credential(&self) -> Option<String> {
        let mut backoff = Backoff::with_limits(self.tuning.backoff_start, self.tuning.backoff_max);
        loop {
            match self.store.load() {
                Ok(Some(credential)) => return Some(credential),
                Ok(None) => {}
                Err(e) => {
                    tracing::error!(error = %e, "cannot read the stored credential");
                    if self.sleep(self.tuning.refused_retry).await {
                        return None;
                    }
                    continue;
                }
            }
            let Some(key) = &self.enrollment.key else {
                tracing::error!(
                    "no stored credential and no enrollment_key configured; \
                     set enrollment_key to enrol this host"
                );
                if self.sleep(self.tuning.refused_retry).await {
                    return None;
                }
                continue;
            };
            let enrolled = tokio::select! {
                () = self.shutdown.cancelled() => return None,
                result = self.client.enroll(&self.enrollment.host, key) => result,
            };
            let wait = match enrolled {
                Ok(credential) => {
                    if let Err(e) = self.store.save(&credential) {
                        tracing::error!(
                            error = %e,
                            "cannot store the credential; it is kept in memory until restart"
                        );
                    }
                    tracing::info!(host = %self.enrollment.host, "enrolled with the server");
                    return Some(credential);
                }
                Err(ClientError::Retryable(reason)) => {
                    tracing::warn!(%reason, "enrolment failed, will retry");
                    backoff.next_delay()
                }
                Err(ClientError::Refused { code, message }) => {
                    // An administrator may still add the host or issue a new key.
                    tracing::warn!(%code, %message, "enrolment refused, will retry");
                    self.tuning.refused_retry
                }
                Err(ClientError::Unauthorized) => {
                    tracing::warn!("enrolment refused, will retry");
                    self.tuning.refused_retry
                }
            };
            if self.sleep(wait).await {
                return None;
            }
        }
    }

    /// Sends spooled values until the credential is rejected or shutdown.
    async fn deliver(&self, credential: &str) -> Outcome {
        let mut backoff = Backoff::with_limits(self.tuning.backoff_start, self.tuning.backoff_max);
        loop {
            if self.shutdown.is_cancelled() {
                return Outcome::Shutdown;
            }
            let batch = match self.with_spool(|s| s.peek(MAX_BATCH_VALUES)) {
                Ok(batch) => batch,
                Err(e) => {
                    tracing::error!(error = %e, "cannot read the spool");
                    if self.sleep(self.tuning.idle).await {
                        return Outcome::Shutdown;
                    }
                    continue;
                }
            };
            if batch.is_empty() {
                if self.sleep(self.tuning.idle).await {
                    return Outcome::Shutdown;
                }
                continue;
            }
            let count = fit_batch(&batch);
            let result = tokio::select! {
                () = self.shutdown.cancelled() => return Outcome::Shutdown,
                result = self.client.send(credential, &batch[..count]) => result,
            };
            match result {
                Ok(response) => {
                    if response.rejected > 0 {
                        tracing::warn!(
                            rejected = response.rejected,
                            accepted = response.accepted,
                            "the server rejected some values of a batch"
                        );
                    }
                    backoff.reset();
                    if self.acknowledge(count).await {
                        return Outcome::Shutdown;
                    }
                }
                Err(ClientError::Retryable(reason)) => {
                    let delay = backoff.next_delay();
                    tracing::warn!(%reason, retry_in = ?delay, "cannot deliver values, keeping them");
                    if self.sleep(delay).await {
                        return Outcome::Shutdown;
                    }
                }
                Err(ClientError::Unauthorized) => {
                    tracing::error!(
                        host = %self.enrollment.host,
                        "the server rejected this host's credential; an administrator must release \n                         the host's binding on the server or provide an enrollment_key; the \n                         rejected credential is kept as credential.rejected"
                    );
                    if let Err(e) = self.store.reject() {
                        tracing::error!(error = %e, "cannot set the rejected credential aside");
                    }
                    if self.sleep(self.tuning.idle).await {
                        return Outcome::Shutdown;
                    }
                    return Outcome::Reenrol;
                }
                Err(ClientError::Refused { code, message }) => {
                    // The same batch would be refused every time.
                    tracing::warn!(%code, %message, dropped = count, "the server refused a batch, dropping it");
                    backoff.reset();
                    if self.acknowledge(count).await {
                        return Outcome::Shutdown;
                    }
                    // A server that refuses everything must not drain the spool at full speed.
                    if self.sleep(self.tuning.refused_pause).await {
                        return Outcome::Shutdown;
                    }
                }
            }
        }
    }

    /// Acknowledges `count` values in the spool. True if shutdown was requested meanwhile.
    async fn acknowledge(&self, count: usize) -> bool {
        match self.with_spool(|s| s.ack(count)) {
            Ok(()) => false,
            // The spool dropped old segments while the batch was in flight; the
            // next peek starts from what is left.
            Err(e) if e.kind() == io::ErrorKind::InvalidInput => {
                tracing::debug!(error = %e, "spool changed during the send, peeking again");
                false
            }
            Err(e) => {
                tracing::error!(error = %e, "cannot acknowledge delivered values; they may be sent again");
                self.sleep(self.tuning.idle).await
            }
        }
    }

    fn with_spool<R>(&self, f: impl FnOnce(&mut Spool) -> R) -> R {
        let mut guard: MutexGuard<'_, Spool> =
            self.spool.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut guard)
    }

    /// Sleeps for `duration`. True if shutdown was requested first.
    async fn sleep(&self, duration: Duration) -> bool {
        tokio::select! {
            () = self.shutdown.cancelled() => true,
            () = tokio::time::sleep(duration) => false,
        }
    }
}

/// How many of the leading `values` go into one request: at most
/// `MAX_BATCH_VALUES`, and few enough to stay below `MAX_BODY_BYTES`. At least one.
fn fit_batch(values: &[MetricValue]) -> usize {
    let mut size = 0usize;
    let mut count = 0usize;
    for value in values.iter().take(MAX_BATCH_VALUES) {
        let len = serde_json::to_vec(value).map_or(0, |v| v.len()) + 1;
        if count > 0 && size + len > MAX_BODY_BYTES {
            break;
        }
        size += len;
        count += 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::Instant;

    use cerbrix_common::ValueBatch;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::config::AgentConfig;

    fn fast() -> Tuning {
        Tuning {
            backoff_start: Duration::from_millis(5),
            backoff_max: Duration::from_millis(40),
            refused_retry: Duration::from_millis(30),
            idle: Duration::from_millis(10),
            refused_pause: Duration::from_millis(10),
        }
    }

    fn values(range: std::ops::Range<i64>) -> Vec<MetricValue> {
        range
            .map(|i| MetricValue {
                key: "cpu.usage".into(),
                params: BTreeMap::new(),
                ts_ms: 1_700_000_000_000 + i,
                value: i as f64,
            })
            .collect()
    }

    struct Fixture {
        dir: tempfile::TempDir,
        spool: Arc<Mutex<Spool>>,
        store: CredentialStore,
        client: ServerClient,
    }

    fn fixture(server: &MockServer) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let spool = Spool::open(&dir.path().join("spool"), 8 * 1024 * 1024).unwrap();
        let client = ServerClient::new(&AgentConfig::for_tests(&server.uri(), dir.path())).unwrap();
        Fixture {
            store: CredentialStore::new(dir.path()),
            spool: Arc::new(Mutex::new(spool)),
            client,
            dir,
        }
    }

    impl Fixture {
        fn push(&self, values: &[MetricValue]) {
            self.spool.lock().unwrap().push(values).unwrap();
        }

        fn spool_is_empty(&self) -> bool {
            let mut spool = self.spool.lock().unwrap();
            spool.peek(1).unwrap().is_empty()
        }

        fn start(self, key: Option<&str>) -> Running {
            let shutdown = CancellationToken::new();
            let task = tokio::spawn(run_sender_with(
                self.client,
                Arc::clone(&self.spool),
                self.store.clone(),
                Enrollment {
                    host: "web-01".into(),
                    key: key.map(str::to_owned),
                },
                shutdown.clone(),
                fast(),
            ));
            Running {
                spool: self.spool,
                store: self.store,
                _dir: self.dir,
                shutdown,
                task,
            }
        }
    }

    struct Running {
        spool: Arc<Mutex<Spool>>,
        store: CredentialStore,
        _dir: tempfile::TempDir,
        shutdown: CancellationToken,
        task: tokio::task::JoinHandle<()>,
    }

    impl Running {
        async fn wait_until_spool_is_empty(&self) {
            let start = Instant::now();
            loop {
                if self.spool.lock().unwrap().peek(1).unwrap().is_empty() {
                    return;
                }
                assert!(
                    start.elapsed() < Duration::from_secs(10),
                    "spool never drained"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }

        /// Stops the sender; the returned directory keeps the spool files alive.
        async fn stop(self) -> tempfile::TempDir {
            self.shutdown.cancel();
            tokio::time::timeout(Duration::from_secs(5), self.task)
                .await
                .expect("sender did not stop")
                .unwrap();
            self._dir
        }
    }

    fn accepted(n: usize) -> ResponseTemplate {
        ResponseTemplate::new(202)
            .set_body_json(serde_json::json!({ "accepted": n, "rejected": 0 }))
    }

    fn value_requests(requests: &[wiremock::Request]) -> Vec<ValueBatch> {
        requests
            .iter()
            .filter(|r| r.url.path().ends_with("/values"))
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .collect()
    }

    #[test]
    fn backoff_doubles_up_to_the_cap_and_resets() {
        let mut backoff = Backoff::new();
        let delays: Vec<u64> = (0..9).map(|_| backoff.next_delay().as_secs()).collect();
        assert_eq!(delays, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
        backoff.reset();
        assert_eq!(backoff.next_delay(), Duration::from_secs(1));
        assert_eq!(backoff.next_delay(), Duration::from_secs(2));
    }

    #[test]
    fn credential_store_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(&dir.path().join("state"));
        assert_eq!(store.load().unwrap(), None);
        store.save("cred-1").unwrap();
        assert_eq!(store.load().unwrap().as_deref(), Some("cred-1"));
        store.save("cred-2").unwrap();
        assert_eq!(store.load().unwrap().as_deref(), Some("cred-2"));
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
        store.clear().unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("state"))
            .unwrap()
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[cfg(unix)]
    #[test]
    fn credential_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path());
        store.save("cred").unwrap();
        store.save("cred-again").unwrap();
        let mode = std::fs::metadata(dir.path().join("credential"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "mode is {mode:o}");
    }

    #[test]
    fn debug_hides_the_enrollment_key() {
        let shown = format!(
            "{:?}",
            Enrollment {
                host: "web-01".into(),
                key: Some("s3cret-key".into())
            }
        );
        assert!(!shown.contains("s3cret-key") && shown.contains("web-01"));
    }

    #[test]
    fn fit_batch_limits_count_and_size() {
        assert_eq!(fit_batch(&values(0..10)), 10);
        assert_eq!(fit_batch(&values(0..1500)), MAX_BATCH_VALUES);
        let mut big = values(0..1000);
        for (i, v) in big.iter_mut().enumerate() {
            for p in 0..8 {
                v.params.insert(format!("p{p}"), "x".repeat(256 + i % 2));
            }
        }
        let n = fit_batch(&big);
        assert!((1..1000).contains(&n), "{n}");
        let bytes: usize = big[..n]
            .iter()
            .map(|v| serde_json::to_vec(v).unwrap().len() + 1)
            .sum();
        assert!(bytes <= MAX_BODY_BYTES);
        // One oversized value is still sent alone.
        assert_eq!(fit_batch(&big[..1]), 1);
    }

    #[tokio::test]
    async fn sender_acks_only_after_202() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/agent/v1/values"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/agent/v1/values"))
            .respond_with(accepted(5))
            .mount(&server)
            .await;
        let fx = fixture(&server);
        fx.store.save("cred").unwrap();
        let pushed = values(0..5);
        fx.push(&pushed);
        let running = fx.start(None);
        running.wait_until_spool_is_empty().await;
        running.stop().await;

        let batches = value_requests(&server.received_requests().await.unwrap());
        assert_eq!(batches.len(), 3, "two failures, then one success");
        for batch in &batches {
            assert_eq!(batch.values, pushed);
        }
    }

    #[tokio::test]
    async fn sender_keeps_values_while_server_is_down() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let fx = fixture(&server);
        fx.store.save("cred").unwrap();
        let pushed = values(0..20);
        fx.push(&pushed);
        let spool = Arc::clone(&fx.spool);
        let running = fx.start(None);
        tokio::time::sleep(Duration::from_millis(400)).await;
        let _dir = running.stop().await;

        let attempts = server.received_requests().await.unwrap().len();
        assert!((3..=30).contains(&attempts), "{attempts} attempts");
        assert_eq!(spool.lock().unwrap().peek(100).unwrap(), pushed);
    }

    #[tokio::test]
    async fn sender_enrols_when_there_is_no_credential() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/agent/v1/enroll"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "credential": "fresh" })),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/agent/v1/values"))
            .and(header("authorization", "Bearer fresh"))
            .respond_with(accepted(3))
            .mount(&server)
            .await;
        let fx = fixture(&server);
        fx.push(&values(0..3));
        let running = fx.start(Some("the-key"));
        running.wait_until_spool_is_empty().await;
        assert_eq!(running.store.load().unwrap().as_deref(), Some("fresh"));
        running.stop().await;
    }

    #[tokio::test]
    async fn refused_enrolment_is_retried() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/agent/v1/enroll"))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "error": "unknown_host", "message": "unknown host"
            })))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/agent/v1/enroll"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "credential": "late" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/agent/v1/values"))
            .respond_with(accepted(1))
            .mount(&server)
            .await;
        let fx = fixture(&server);
        fx.push(&values(0..1));
        let running = fx.start(Some("the-key"));
        running.wait_until_spool_is_empty().await;
        assert_eq!(running.store.load().unwrap().as_deref(), Some("late"));
        running.stop().await;
    }

    #[tokio::test]
    async fn sender_reenrols_after_unauthorized() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/agent/v1/enroll"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "credential": "cred-2" })),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/agent/v1/values"))
            .and(header("authorization", "Bearer cred-2"))
            .respond_with(accepted(4))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/agent/v1/values"))
            .respond_with(unauthorized())
            .mount(&server)
            .await;
        let fx = fixture(&server);
        fx.store.save("cred-1").unwrap();
        let pushed = values(0..4);
        fx.push(&pushed);
        let running = fx.start(Some("the-key"));
        running.wait_until_spool_is_empty().await;
        assert_eq!(running.store.load().unwrap().as_deref(), Some("cred-2"));
        running.stop().await;

        let batches = value_requests(&server.received_requests().await.unwrap());
        assert_eq!(batches.last().unwrap().values, pushed);
    }

    fn unauthorized() -> ResponseTemplate {
        ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": "unauthorized", "message": "missing or invalid credential"
        }))
    }

    #[tokio::test]
    async fn proxy_404_keeps_values_in_the_spool() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(404).set_body_string("<html>nginx</html>"))
            .mount(&server)
            .await;
        let fx = fixture(&server);
        fx.store.save("cred").unwrap();
        let pushed = values(0..20);
        fx.push(&pushed);
        let spool = Arc::clone(&fx.spool);
        let running = fx.start(None);
        tokio::time::sleep(Duration::from_millis(400)).await;
        let _dir = running.stop().await;
        assert_eq!(spool.lock().unwrap().peek(100).unwrap(), pushed);
    }

    #[tokio::test]
    async fn foreign_401_does_not_touch_the_credential() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_string("<html>login</html>"))
            .mount(&server)
            .await;
        let fx = fixture(&server);
        fx.store.save("cred").unwrap();
        let pushed = values(0..3);
        fx.push(&pushed);
        let spool = Arc::clone(&fx.spool);
        let store = fx.store.clone();
        let dir = fx.dir.path().to_path_buf();
        let running = fx.start(Some("the-key"));
        tokio::time::sleep(Duration::from_millis(300)).await;
        let _dir = running.stop().await;
        assert_eq!(store.load().unwrap().as_deref(), Some("cred"));
        assert!(!dir.join("credential.rejected").exists());
        assert_eq!(spool.lock().unwrap().peek(10).unwrap(), pushed);
        let requests = server.received_requests().await.unwrap();
        assert!(requests.iter().all(|r| r.url.path().ends_with("/values")));
    }

    #[tokio::test]
    async fn rejected_credential_is_kept_as_credential_rejected() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/agent/v1/enroll"))
            .respond_with(ResponseTemplate::new(409).set_body_json(serde_json::json!({
                "error": "already_bound", "message": "host is already enrolled"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/agent/v1/values"))
            .respond_with(unauthorized())
            .mount(&server)
            .await;
        let fx = fixture(&server);
        fx.store.save("old-cred").unwrap();
        fx.push(&values(0..2));
        let spool = Arc::clone(&fx.spool);
        let running = fx.start(None);
        tokio::time::sleep(Duration::from_millis(300)).await;
        let dir = running.stop().await;
        let rejected = std::fs::read_to_string(dir.path().join("credential.rejected")).unwrap();
        assert_eq!(rejected, "old-cred");
        assert!(!dir.path().join("credential").exists());
        assert_eq!(spool.lock().unwrap().peek(10).unwrap().len(), 2);
    }

    #[test]
    fn reject_replaces_an_older_rejected_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path());
        store.reject().unwrap();
        store.save("one").unwrap();
        store.reject().unwrap();
        store.save("two").unwrap();
        store.reject().unwrap();
        assert_eq!(store.load().unwrap(), None);
        let kept = std::fs::read_to_string(dir.path().join("credential.rejected")).unwrap();
        assert_eq!(kept, "two");
    }
    #[tokio::test]
    async fn refused_batch_is_dropped_and_sending_continues() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/agent/v1/values"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "malformed", "message": "request body is not valid"
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/agent/v1/values"))
            .respond_with(
                ResponseTemplate::new(202)
                    .set_body_json(serde_json::json!({ "accepted": 2, "rejected": 1 })),
            )
            .mount(&server)
            .await;
        let fx = fixture(&server);
        fx.store.save("cred").unwrap();
        let first = values(0..3);
        let second = values(10..12);
        fx.push(&first);
        let running = fx.start(None);
        running.wait_until_spool_is_empty().await;
        running.spool.lock().unwrap().push(&second).unwrap();
        running.wait_until_spool_is_empty().await;
        running.stop().await;

        let batches = value_requests(&server.received_requests().await.unwrap());
        assert_eq!(batches.len(), 2, "refused batch must not be sent again");
        assert_eq!(batches[0].values, first);
        assert_eq!(batches[1].values, second);
    }

    #[tokio::test]
    async fn idle_sender_does_not_spin_and_stops_on_shutdown() {
        let server = MockServer::start().await;
        let fx = fixture(&server);
        fx.store.save("cred").unwrap();
        assert!(fx.spool_is_empty());
        let running = fx.start(None);
        tokio::time::sleep(Duration::from_millis(100)).await;
        running.stop().await;
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
