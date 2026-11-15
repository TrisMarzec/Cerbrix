//! Start-up and TLS tests. They need PostgreSQL: set `DATABASE_URL` (the dev database is fine).

use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::time::Duration;

use cerbrix_server::config::{Config, DatabaseConfig, IngestConfig, MetricsConfig, ServerConfig};
use cerbrix_server::tls::serve;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

fn config(listen: SocketAddr, cert: Option<&Path>, key: Option<&Path>, insecure: bool) -> Config {
    Config {
        server: ServerConfig {
            listen,
            tls_cert: cert.map(Path::to_path_buf),
            tls_key: key.map(Path::to_path_buf),
            insecure_http: insecure,
        },
        database: DatabaseConfig {
            url: std::env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
        },
        metrics: MetricsConfig {
            url: "http://localhost:8428".to_owned(),
        },
        ingest: IngestConfig::default(),
        gitops: Default::default(),
    }
}

struct Running {
    port: u16,
    stop: oneshot::Sender<()>,
    task: JoinHandle<anyhow::Result<()>>,
}

impl Running {
    fn start(cert: Option<&Path>, key: Option<&Path>, insecure: bool) -> Running {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (stop, rx) = oneshot::channel::<()>();
        let cfg = config(addr, cert, key, insecure);
        let task = tokio::spawn(serve(cfg, listener, async move {
            let _ = rx.await;
        }));
        Running {
            port: addr.port(),
            stop,
            task,
        }
    }

    /// Stops the server through the shutdown future and waits for `serve` to return.
    async fn shutdown(self) {
        let _ = self.stop.send(());
        let result = tokio::time::timeout(Duration::from_secs(20), self.task)
            .await
            .expect("graceful shutdown did not return in time")
            .expect("server task panicked");
        result.expect("serve returned an error");
    }
}

struct Pki {
    _dir: tempfile::TempDir,
    cert: std::path::PathBuf,
    key: std::path::PathBuf,
    cert_pem: String,
}

fn self_signed() -> Pki {
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cert = dir.path().join("cert.pem");
    let key = dir.path().join("key.pem");
    let cert_pem = ck.cert.pem();
    std::fs::write(&cert, &cert_pem).unwrap();
    std::fs::write(&key, ck.signing_key.serialize_pem()).unwrap();
    Pki {
        _dir: dir,
        cert,
        key,
        cert_pem,
    }
}

async fn wait_for_get(client: &reqwest::Client, url: &str) -> reqwest::Response {
    let mut last = None;
    for _ in 0..50 {
        match client.get(url).send().await {
            Ok(resp) => return resp,
            Err(err) => last = Some(err),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("server never answered {url}: {last:?}");
}

#[tokio::test]
async fn https_healthz_works_with_supplied_certificate() {
    let pki = self_signed();
    let server = Running::start(Some(&pki.cert), Some(&pki.key), false);

    let root = reqwest::Certificate::from_pem(pki.cert_pem.as_bytes()).unwrap();
    let client = reqwest::Client::builder()
        .tls_certs_only([root])
        .build()
        .unwrap();
    let url = format!("https://localhost:{}/healthz", server.port);
    let resp = wait_for_get(&client, &url).await;
    assert_eq!(resp.status(), 200);

    server.shutdown().await;
}

#[tokio::test]
async fn plain_http_is_refused_when_tls_is_configured() {
    let pki = self_signed();
    let server = Running::start(Some(&pki.cert), Some(&pki.key), false);

    // Wait until the server is up by completing one HTTPS request first.
    let root = reqwest::Certificate::from_pem(pki.cert_pem.as_bytes()).unwrap();
    let https = reqwest::Client::builder()
        .tls_certs_only([root])
        .build()
        .unwrap();
    let ready = wait_for_get(
        &https,
        &format!("https://localhost:{}/healthz", server.port),
    )
    .await;
    assert_eq!(ready.status(), 200);

    let plain = reqwest::Client::new();
    let outcome = plain
        .get(format!("http://127.0.0.1:{}/healthz", server.port))
        .send()
        .await;
    if let Ok(resp) = outcome {
        assert!(!resp.status().is_success(), "got {}", resp.status());
    }

    server.shutdown().await;
}

#[tokio::test]
async fn missing_certificate_file_fails_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let cert = dir.path().join("does-not-exist-cert.pem");
    let key = dir.path().join("does-not-exist-key.pem");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let cfg = config(
        listener.local_addr().unwrap(),
        Some(&cert),
        Some(&key),
        false,
    );

    let err = serve(cfg, listener, std::future::pending())
        .await
        .expect_err("start must fail");
    let message = format!("{err:#}");
    assert!(
        message.contains(&cert.display().to_string()),
        "error does not name the certificate file: {message}"
    );
}

#[tokio::test]
async fn insecure_http_serves_plain_http_when_enabled() {
    let server = Running::start(None, None, true);

    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{}/healthz", server.port);
    let resp = wait_for_get(&client, &url).await;
    assert_eq!(resp.status(), 200);

    server.shutdown().await;
}

#[tokio::test]
async fn half_configured_tls_pair_fails_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let cert = dir.path().join("cert.pem");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let cfg = config(listener.local_addr().unwrap(), Some(&cert), None, true);

    let err = serve(cfg, listener, std::future::pending())
        .await
        .expect_err("start must fail");
    let message = format!("{err:#}");
    assert!(message.contains("server.tls_key"), "{message}");
}
