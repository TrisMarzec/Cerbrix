//! HTTP client for the server's agent API.

use std::time::Duration;

use anyhow::Context;
use cerbrix_common::{
    EnrollRequest, EnrollResponse, ErrorBody, IngestResponse, MetricValue, PROTOCOL_VERSION,
};
use reqwest::{Response, StatusCode, Url, redirect};
use serde::Serialize;

use crate::config::AgentConfig;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Why a request to the server did not succeed. Messages never contain the
/// enrolment key or the credential.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Worth trying again later: network trouble, timeouts, 5xx, 429.
    #[error("server not reachable or busy: {0}")]
    Retryable(String),
    /// The server says it does not know this credential (401, code `unauthorized`).
    #[error("the server rejected the credential")]
    Unauthorized,
    /// The server refused the request and will keep doing so.
    #[error("refused by the server ({code}): {message}")]
    Refused { code: String, message: String },
}

pub struct ServerClient {
    http: reqwest::Client,
    enroll_url: Url,
    values_url: Url,
}

impl ServerClient {
    /// Builds a client for `cfg.server_url`; `cfg.ca_file` is trusted in addition
    /// to the system roots.
    pub fn new(cfg: &AgentConfig) -> anyhow::Result<Self> {
        let mut builder = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            // A redirect would carry the credential to wherever it points.
            .redirect(redirect::Policy::none())
            .user_agent(concat!("cerbrix-agent/", env!("CARGO_PKG_VERSION")));
        if let Some(path) = &cfg.ca_file {
            let pem = std::fs::read(path)
                .with_context(|| format!("cannot read ca_file {}", path.display()))?;
            let certs = reqwest::Certificate::from_pem_bundle(&pem)
                .with_context(|| format!("ca_file {} is not valid PEM", path.display()))?;
            anyhow::ensure!(
                !certs.is_empty(),
                "ca_file {} contains no certificate",
                path.display()
            );
            builder = builder.tls_certs_merge(certs);
        }
        let http = builder.build().context("cannot build the HTTP client")?;

        let mut base = cfg.server_url.clone();
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        let endpoint = |path: &str| {
            base.join(path)
                .with_context(|| format!("cannot build the {path} URL from server_url"))
        };
        Ok(Self {
            http,
            enroll_url: endpoint("api/agent/v1/enroll")?,
            values_url: endpoint("api/agent/v1/values")?,
        })
    }

    /// Exchanges the enrolment key for a credential.
    pub async fn enroll(&self, host: &str, key: &str) -> Result<String, ClientError> {
        let body = EnrollRequest {
            host: host.to_owned(),
            enrollment_key: key.to_owned(),
            agent_version: env!("CARGO_PKG_VERSION").to_owned(),
        };
        let request = self.http.post(self.enroll_url.clone()).json(&body);
        let response = send_request(request).await?;
        // On this route a 401 means "invalid_key", not an expired credential.
        let response = check_status(response, StatusCode::OK, false).await?;
        let parsed: EnrollResponse = response
            .json()
            .await
            .map_err(|e| ClientError::Retryable(format!("unreadable enrolment answer: {e}")))?;
        Ok(parsed.credential)
    }

    /// Sends a batch of values. Success is a 202 answer.
    pub async fn send(
        &self,
        credential: &str,
        values: &[MetricValue],
    ) -> Result<IngestResponse, ClientError> {
        #[derive(Serialize)]
        struct Borrowed<'a> {
            protocol_version: u32,
            values: &'a [MetricValue],
        }
        // Same JSON as `ValueBatch`, but borrows the values instead of owning them.
        let body = Borrowed {
            protocol_version: PROTOCOL_VERSION,
            values,
        };
        let request = self
            .http
            .post(self.values_url.clone())
            .bearer_auth(credential)
            .json(&body);
        let response = send_request(request).await?;
        let response = check_status(response, StatusCode::ACCEPTED, true).await?;
        response
            .json()
            .await
            .map_err(|e| ClientError::Retryable(format!("unreadable server answer: {e}")))
    }
}

async fn send_request(request: reqwest::RequestBuilder) -> Result<Response, ClientError> {
    request
        .send()
        .await
        .map_err(|e| ClientError::Retryable(e.without_url().to_string()))
}

/// Passes `response` on if it has the `expected` status, otherwise maps it to an error.
///
/// Only an answer that carries Cerbrix's `ErrorBody` is a verdict of the server
/// (`Refused`, or `Unauthorized` for the code `unauthorized`). A 4xx without one
/// came from something else on the path (proxy, wrong URL) and, like 408 and
/// 429, is transient from the agent's point of view.
async fn check_status(
    response: Response,
    expected: StatusCode,
    unauthorized_is_credential: bool,
) -> Result<Response, ClientError> {
    let status = response.status();
    if status == expected {
        return Ok(response);
    }
    let transient = status.is_server_error()
        || status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::REQUEST_TIMEOUT;
    if transient || !status.is_client_error() {
        return Err(ClientError::Retryable(format!("server answered {status}")));
    }
    match response.json::<ErrorBody>().await {
        Ok(body)
            if status == StatusCode::UNAUTHORIZED
                && unauthorized_is_credential
                && body.error == "unauthorized" =>
        {
            Err(ClientError::Unauthorized)
        }
        Ok(body) => Err(ClientError::Refused {
            code: body.error,
            message: body.message,
        }),
        Err(_) => Err(ClientError::Retryable(format!(
            "answer {status} did not come from a Cerbrix server"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::net::SocketAddr;

    use axum::Router;
    use axum::routing::post;
    use cerbrix_common::ValueBatch;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn cfg(url: &str) -> AgentConfig {
        AgentConfig::for_tests(url, std::path::Path::new("unused"))
    }

    fn value() -> MetricValue {
        MetricValue {
            key: "cpu.usage".into(),
            params: BTreeMap::new(),
            ts_ms: 1_700_000_000_000,
            value: 1.0,
        }
    }

    fn error_body(code: &str) -> ResponseTemplate {
        ResponseTemplate::new(409).set_body_json(serde_json::json!({
            "error": code, "message": "nope"
        }))
    }

    #[tokio::test]
    async fn enroll_returns_credential() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/agent/v1/enroll"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "credential": "cred-123" })),
            )
            .expect(1)
            .mount(&server)
            .await;
        let client = ServerClient::new(&cfg(&server.uri())).unwrap();
        assert_eq!(client.enroll("web-01", "key").await.unwrap(), "cred-123");

        let requests = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["host"], "web-01");
        assert_eq!(body["enrollment_key"], "key");
        assert_eq!(body["agent_version"], env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn client_maps_statuses() {
        let server = MockServer::start().await;
        let client = ServerClient::new(&cfg(&server.uri())).unwrap();
        let respond = |template: ResponseTemplate| {
            let server = &server;
            async move {
                server.reset().await;
                Mock::given(method("POST"))
                    .respond_with(template)
                    .mount(server)
                    .await;
            }
        };

        respond(ResponseTemplate::new(503)).await;
        assert!(matches!(
            client.send("c", &[value()]).await,
            Err(ClientError::Retryable(_))
        ));
        respond(ResponseTemplate::new(429)).await;
        assert!(matches!(
            client.send("c", &[value()]).await,
            Err(ClientError::Retryable(_))
        ));
        respond(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": "unauthorized", "message": "missing or invalid credential"
        })))
        .await;
        assert!(matches!(
            client.send("c", &[value()]).await,
            Err(ClientError::Unauthorized)
        ));
        respond(error_body("already_bound")).await;
        match client.enroll("web-01", "key").await {
            Err(ClientError::Refused { code, .. }) => assert_eq!(code, "already_bound"),
            other => panic!("expected Refused, got {other:?}"),
        }
        // On enrolment a 401 is a wrong key, not a stale credential.
        respond(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": "invalid_key", "message": "invalid enrolment key"
        })))
        .await;
        match client.enroll("web-01", "key").await {
            Err(ClientError::Refused { code, .. }) => assert_eq!(code, "invalid_key"),
            other => panic!("expected Refused, got {other:?}"),
        }
        respond(ResponseTemplate::new(202).set_body_json(serde_json::json!({
            "accepted": 1, "rejected": 0
        })))
        .await;
        let ok = client.send("c", &[value()]).await.unwrap();
        assert_eq!((ok.accepted, ok.rejected), (1, 0));
    }

    async fn answer_once(template: ResponseTemplate) -> (MockServer, ServerClient) {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(template)
            .mount(&server)
            .await;
        let client = ServerClient::new(&cfg(&server.uri())).unwrap();
        (server, client)
    }

    #[tokio::test]
    async fn bodyless_4xx_is_retryable() {
        for template in [
            ResponseTemplate::new(404),
            ResponseTemplate::new(403),
            ResponseTemplate::new(405),
            ResponseTemplate::new(400),
            ResponseTemplate::new(404).set_body_string("<html>Not Found</html>"),
            ResponseTemplate::new(403).set_body_string("<html>Forbidden</html>"),
            // Another service's 401 is not a verdict on the credential.
            ResponseTemplate::new(401),
            ResponseTemplate::new(401).set_body_string("<html>login</html>"),
        ] {
            let (_server, client) = answer_once(template).await;
            assert!(matches!(
                client.send("c", &[value()]).await,
                Err(ClientError::Retryable(_))
            ));
            assert!(matches!(
                client.enroll("web-01", "k").await,
                Err(ClientError::Retryable(_))
            ));
        }
    }

    #[tokio::test]
    async fn request_timeout_408_is_retryable() {
        for status in [408, 429] {
            let body = serde_json::json!({ "error": "slow", "message": "later" });
            let (_server, client) =
                answer_once(ResponseTemplate::new(status).set_body_json(body)).await;
            assert!(matches!(
                client.send("c", &[value()]).await,
                Err(ClientError::Retryable(_))
            ));
        }
    }

    #[tokio::test]
    async fn refused_requires_an_error_body() {
        let body = serde_json::json!({ "error": "batch_too_large", "message": "too many" });
        let (_server, client) = answer_once(ResponseTemplate::new(413).set_body_json(body)).await;
        match client.send("c", &[value()]).await {
            Err(ClientError::Refused { code, .. }) => assert_eq!(code, "batch_too_large"),
            other => panic!("expected Refused, got {other:?}"),
        }
        // A 401 with another error code is not "unauthorized" either.
        let body = serde_json::json!({ "error": "invalid_key", "message": "x" });
        let (_server, client) = answer_once(ResponseTemplate::new(401).set_body_json(body)).await;
        assert!(matches!(
            client.send("c", &[value()]).await,
            Err(ClientError::Refused { .. })
        ));
    }
    #[tokio::test]
    async fn send_carries_bearer_credential_and_protocol_version() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/agent/v1/values"))
            .and(header("authorization", "Bearer cred-9"))
            .respond_with(
                ResponseTemplate::new(202)
                    .set_body_json(serde_json::json!({ "accepted": 1, "rejected": 0 })),
            )
            .expect(1)
            .mount(&server)
            .await;
        let client = ServerClient::new(&cfg(&server.uri())).unwrap();
        client.send("cred-9", &[value()]).await.unwrap();
        let requests = server.received_requests().await.unwrap();
        let body: ValueBatch = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body.protocol_version, PROTOCOL_VERSION);
        assert_eq!(body.values, vec![value()]);
    }

    #[tokio::test]
    async fn unreachable_server_is_retryable_and_base_path_is_kept() {
        // Nothing listens on port 1.
        let client = ServerClient::new(&cfg("http://127.0.0.1:1")).unwrap();
        assert!(matches!(
            client.send("c", &[value()]).await,
            Err(ClientError::Retryable(_))
        ));
        let prefixed = ServerClient::new(&cfg("http://example.com/cerbrix")).unwrap();
        assert_eq!(
            prefixed.values_url.as_str(),
            "http://example.com/cerbrix/api/agent/v1/values"
        );
    }

    #[tokio::test]
    async fn client_trusts_custom_ca_file() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cert = dir.path().join("cert.pem");
        let key = dir.path().join("key.pem");
        std::fs::write(&cert, ck.cert.pem()).unwrap();
        std::fs::write(&key, ck.signing_key.serialize_pem()).unwrap();

        let app = Router::new().route(
            "/api/agent/v1/enroll",
            post(|| async { axum::Json(serde_json::json!({ "credential": "tls-cred" })) }),
        );
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert, &key)
            .await
            .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = axum_server::Handle::new();
        let server_handle = handle.clone();
        tokio::spawn(async move {
            axum_server::from_tcp_rustls(listener, tls)
                .unwrap()
                .handle(server_handle)
                .serve(app.into_make_service_with_connect_info::<SocketAddr>())
                .await
        });
        let url = format!("https://localhost:{port}");

        // Without the CA the self-signed certificate is not trusted.
        let plain = ServerClient::new(&cfg(&url)).unwrap();
        assert!(matches!(
            plain.enroll("web-01", "k").await,
            Err(ClientError::Retryable(_))
        ));

        let mut with_ca = cfg(&url);
        with_ca.ca_file = Some(cert.clone());
        let trusting = ServerClient::new(&with_ca).unwrap();
        assert_eq!(trusting.enroll("web-01", "k").await.unwrap(), "tls-cred");
        handle.shutdown();

        // A missing or non-PEM ca_file is a start-up error.
        let mut broken = cfg(&url);
        broken.ca_file = Some(dir.path().join("missing.pem"));
        assert!(ServerClient::new(&broken).is_err());
        std::fs::write(dir.path().join("junk.pem"), "not a certificate").unwrap();
        broken.ca_file = Some(dir.path().join("junk.pem"));
        assert!(ServerClient::new(&broken).is_err());
    }
}
