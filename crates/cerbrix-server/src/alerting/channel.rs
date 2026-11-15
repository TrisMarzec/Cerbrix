//! Notification channels. The first one sends e-mail over SMTP.

use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use lettre::message::header::{ContentType, Header, HeaderName, HeaderValue};
use lettre::message::{Mailbox, MessageBuilder};
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::transport::smtp::{AsyncSmtpTransport, authentication::Credentials};
use lettre::{AsyncTransport, Tokio1Executor};

use super::message::Notification;
use crate::config::{SmtpConfig, SmtpEncryption};

/// Longest error text, in characters.
const MAX_ERROR_CHARS: usize = 300;

/// Why a notification could not be delivered.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChannelError {
    /// Trying again later may succeed (connection, timeout, TLS, SMTP 4xx).
    #[error("{0}")]
    Retryable(String),
    /// Trying again will not help (SMTP 5xx, an address that does not parse).
    #[error("{0}")]
    Permanent(String),
}

/// A way to deliver a notification to a recipient.
#[async_trait]
pub trait Channel: Send + Sync {
    fn name(&self) -> &'static str;
    async fn send(&self, to: &str, notification: &Notification) -> Result<(), ChannelError>;
}

/// `Auto-Submitted: auto-generated` (RFC 3834): tells auto-responders not to answer.
#[derive(Clone)]
struct AutoSubmitted;

impl Header for AutoSubmitted {
    fn name() -> HeaderName {
        HeaderName::new_from_ascii_str("Auto-Submitted")
    }

    fn parse(_: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(AutoSubmitted)
    }

    fn display(&self) -> HeaderValue {
        HeaderValue::new(Self::name(), "auto-generated".to_owned())
    }
}

/// Sends notifications as e-mail through one SMTP server.
///
/// Every `send` opens its own connection (no pool), so a failed send cannot affect
/// the next one.
pub struct SmtpChannel {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
    timeout: Duration,
    /// Values removed from error texts, as a second line of defence.
    secrets: Vec<String>,
}

impl fmt::Debug for SmtpChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SmtpChannel")
            .field("from", &self.from)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl SmtpChannel {
    /// Builds the transport without connecting. Fails only for settings that cannot be
    /// turned into a transport.
    pub fn new(cfg: &SmtpConfig) -> anyhow::Result<Self> {
        let host = cfg
            .host
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("smtp.host is not set"))?;
        let from: Mailbox = cfg
            .from
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("smtp.from is not set"))?
            .parse()
            .map_err(|_| anyhow::anyhow!("smtp.from is not a valid e-mail address"))?;
        let credentials = match (&cfg.username, &cfg.password) {
            (Some(user), Some(password)) => Some(Credentials::new(user.clone(), password.clone())),
            (None, None) => None,
            _ => anyhow::bail!("smtp.username and smtp.password must be set together"),
        };
        if credentials.is_some() && cfg.encryption == SmtpEncryption::None {
            anyhow::bail!("smtp.encryption = \"none\" would send the password unencrypted");
        }
        let tls = || {
            TlsParameters::new(host.to_owned())
                .map_err(|_| anyhow::anyhow!("smtp.host cannot be used for TLS"))
        };
        let tls = match cfg.encryption {
            // `Required`: the dialogue stops before AUTH and MAIL if STARTTLS is not offered.
            SmtpEncryption::Starttls => Tls::Required(tls()?),
            SmtpEncryption::Tls => Tls::Wrapper(tls()?),
            SmtpEncryption::None => Tls::None,
        };
        let timeout = Duration::from_secs(cfg.timeout_secs);
        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
            .port(cfg.port)
            .tls(tls)
            .timeout(Some(timeout));
        let mut secrets = Vec::new();
        if let Some(credentials) = credentials {
            builder = builder.credentials(credentials);
            secrets.extend(cfg.username.clone());
            secrets.extend(cfg.password.clone());
        }
        Ok(Self {
            transport: builder.build(),
            from,
            timeout,
            secrets,
        })
    }

    fn build_message(
        &self,
        to: &str,
        notification: &Notification,
    ) -> Result<lettre::Message, ChannelError> {
        let permanent = |why: &str| ChannelError::Permanent(why.to_owned());
        // One plain mailbox: a list, a line break or another control character is refused.
        if to.chars().any(char::is_control) {
            return Err(permanent("recipient contains control characters"));
        }
        let to: Mailbox = to
            .parse()
            .map_err(|_| permanent("recipient is not a single valid e-mail address"))?;
        let domain = self.from.email.domain().to_owned();
        let message_id = format!("<{}@{domain}>", uuid::Uuid::new_v4().simple());
        MessageBuilder::new()
            .from(self.from.clone())
            .to(to)
            .subject(notification.subject())
            .date_now()
            .message_id(Some(message_id))
            .header(AutoSubmitted)
            .header(ContentType::TEXT_PLAIN)
            .body(notification.body())
            .map_err(|e| ChannelError::Permanent(self.describe(&e.to_string())))
    }

    fn describe(&self, text: &str) -> String {
        let secrets: Vec<&str> = self.secrets.iter().map(String::as_str).collect();
        describe(text, &secrets)
    }
}

#[async_trait]
impl Channel for SmtpChannel {
    fn name(&self) -> &'static str {
        "email"
    }

    async fn send(&self, to: &str, notification: &Notification) -> Result<(), ChannelError> {
        let message = self.build_message(to, notification)?;
        // The transport's own timeout covers single steps; this bounds the whole send.
        match tokio::time::timeout(self.timeout, self.transport.send(message)).await {
            Err(_) => Err(ChannelError::Retryable(format!(
                "timed out after {} s",
                self.timeout.as_secs()
            ))),
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => {
                let text = self.describe(&e.to_string());
                if e.is_permanent() {
                    Err(ChannelError::Permanent(text))
                } else {
                    // Transient answers, timeouts, connection and TLS errors.
                    Err(ChannelError::Retryable(text))
                }
            }
        }
    }
}

/// Error text safe to store and log: secrets removed, control characters replaced by
/// spaces, at most 300 characters.
fn describe(text: &str, secrets: &[&str]) -> String {
    let mut text = text.to_owned();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        text = text.replace(secret, "<redacted>");
    }
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_ERROR_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::time::Duration;

    use cerbrix_common::config::Severity;
    use time::OffsetDateTime;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    use super::*;
    use crate::alerting::EventKind;
    use crate::config::{SmtpConfig, SmtpEncryption};

    /// Scripted answers of the in-test SMTP server. `None` as greeting: never answers.
    #[derive(Clone)]
    struct Script {
        greeting: Option<&'static str>,
        ehlo: &'static str,
        mail: &'static str,
        rcpt: &'static str,
        data: &'static str,
        end: &'static str,
    }

    impl Default for Script {
        fn default() -> Self {
            Script {
                greeting: Some("220 test ESMTP ready"),
                ehlo: "250-test\r\n250 8BITMIME",
                mail: "250 ok",
                rcpt: "250 ok",
                data: "354 go ahead",
                end: "250 queued",
            }
        }
    }

    /// What the server saw on one connection.
    #[derive(Debug, Default)]
    struct Captured {
        commands: Vec<String>,
        data: String,
    }

    /// Accepts one connection per script, in order, on 127.0.0.1.
    async fn smtp_server(scripts: Vec<Script>) -> (SocketAddr, JoinHandle<Vec<Captured>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut all = Vec::new();
            for script in scripts {
                let (stream, _) = listener.accept().await.unwrap();
                all.push(serve(stream, script).await);
            }
            all
        });
        (addr, handle)
    }

    async fn serve(stream: tokio::net::TcpStream, script: Script) -> Captured {
        let mut captured = Captured::default();
        let (read, mut write) = stream.into_split();
        let mut read = BufReader::new(read);
        let Some(greeting) = script.greeting else {
            // Never answer; wait until the client gives up and closes.
            let mut sink = Vec::new();
            let _ = read.read_to_end(&mut sink).await;
            return captured;
        };
        let say = |text: &str| format!("{text}\r\n");
        write.write_all(say(greeting).as_bytes()).await.unwrap();
        let mut in_data = false;
        let mut line = String::new();
        loop {
            line.clear();
            if read.read_line(&mut line).await.unwrap_or(0) == 0 {
                break;
            }
            if in_data {
                if line == ".\r\n" {
                    in_data = false;
                    write.write_all(say(script.end).as_bytes()).await.unwrap();
                } else {
                    captured.data.push_str(&line);
                }
                continue;
            }
            let command = line.trim_end().to_owned();
            let upper = command.to_ascii_uppercase();
            captured.commands.push(command);
            let reply = if upper.starts_with("EHLO") || upper.starts_with("HELO") {
                script.ehlo
            } else if upper.starts_with("MAIL FROM") {
                script.mail
            } else if upper.starts_with("RCPT TO") {
                script.rcpt
            } else if upper == "DATA" {
                if script.data.starts_with("354") {
                    in_data = true;
                }
                script.data
            } else if upper == "QUIT" {
                let _ = write.write_all(say("221 bye").as_bytes()).await;
                break;
            } else {
                "250 ok"
            };
            write.write_all(say(reply).as_bytes()).await.unwrap();
        }
        captured
    }

    async fn finished(handle: JoinHandle<Vec<Captured>>) -> Vec<Captured> {
        tokio::time::timeout(Duration::from_secs(30), handle)
            .await
            .expect("server task did not finish")
            .expect("server task panicked")
    }

    fn config(addr: SocketAddr) -> SmtpConfig {
        SmtpConfig {
            host: Some(addr.ip().to_string()),
            port: addr.port(),
            encryption: SmtpEncryption::None,
            from: Some("Cerbrix <cerbrix@example.com>".to_owned()),
            timeout_secs: 10,
            ..SmtpConfig::default()
        }
    }

    fn notification() -> Notification {
        Notification {
            kind: EventKind::Started,
            host: "db-02".to_owned(),
            trigger: "disk_free".to_owned(),
            instance: String::new(),
            severity: Severity::Critical,
            summary: "Disk almost full".to_owned(),
            started_at: OffsetDateTime::from_unix_timestamp(1_794_752_400).unwrap(),
            resolved_at: None,
            resolve_reason: None,
        }
    }

    const TO: &str = "ops@example.com";

    #[tokio::test]
    async fn sends_one_message() {
        let (addr, server) = smtp_server(vec![Script::default()]).await;
        let channel = SmtpChannel::new(&config(addr)).unwrap();
        assert_eq!(channel.name(), "email");
        channel.send(TO, &notification()).await.unwrap();
        let captured = finished(server).await;
        let data = &captured[0].data;
        assert!(
            data.contains("From: \"Cerbrix\" <cerbrix@example.com>")
                || data.contains("From: Cerbrix <cerbrix@example.com>"),
            "{data}"
        );
        assert!(data.contains("To: ops@example.com"), "{data}");
        assert!(
            data.contains("Subject: [Cerbrix] CRITICAL db-02: Disk almost full"),
            "{data}"
        );
        assert!(data.contains("Auto-Submitted: auto-generated"), "{data}");
        assert!(data.contains("Host: db-02"), "{data}");
        assert!(data.contains("Date: "), "{data}");
        assert!(
            data.lines()
                .any(|l| l.starts_with("Message-ID: <") && l.ends_with("@example.com>")),
            "{data}"
        );
        let commands = &captured[0].commands;
        assert!(
            !commands
                .iter()
                .any(|c| c.to_ascii_uppercase().starts_with("AUTH")),
            "{commands:?}"
        );
    }

    #[tokio::test]
    async fn rcpt_550_is_permanent() {
        let script = Script {
            rcpt: "550 no such user here",
            ..Script::default()
        };
        let (addr, server) = smtp_server(vec![script]).await;
        let channel = SmtpChannel::new(&config(addr)).unwrap();
        match channel.send(TO, &notification()).await {
            Err(ChannelError::Permanent(text)) => {
                assert!(
                    text.contains("550") && text.contains("no such user"),
                    "{text}"
                );
            }
            other => panic!("expected Permanent, got {other:?}"),
        }
        finished(server).await;
    }

    #[tokio::test]
    async fn data_451_is_retryable() {
        let script = Script {
            end: "451 try again later",
            ..Script::default()
        };
        let (addr, server) = smtp_server(vec![script]).await;
        let channel = SmtpChannel::new(&config(addr)).unwrap();
        match channel.send(TO, &notification()).await {
            Err(ChannelError::Retryable(text)) => {
                assert!(text.contains("451") && text.contains("try again"), "{text}");
            }
            other => panic!("expected Retryable, got {other:?}"),
        }
        finished(server).await;
    }

    #[tokio::test]
    async fn connection_refused_is_retryable() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let channel = SmtpChannel::new(&config(addr)).unwrap();
        let result = channel.send(TO, &notification()).await;
        assert!(
            matches!(result, Err(ChannelError::Retryable(_))),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn timeout_is_retryable() {
        let script = Script {
            greeting: None,
            ..Script::default()
        };
        let (addr, server) = smtp_server(vec![script]).await;
        let cfg = SmtpConfig {
            timeout_secs: 1,
            ..config(addr)
        };
        let channel = SmtpChannel::new(&cfg).unwrap();
        let started = std::time::Instant::now();
        let result = channel.send(TO, &notification()).await;
        assert!(
            matches!(&result, Err(ChannelError::Retryable(t)) if t.contains("timed out")),
            "{result:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(20));
        finished(server).await;
    }

    #[tokio::test]
    async fn invalid_recipient_is_permanent() {
        // No server: a connection attempt would be refused and be Retryable instead.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let channel = SmtpChannel::new(&config(addr)).unwrap();
        for bad in [
            "not an address",
            "",
            "a@example.com, b@example.com",
            "a@example.com\r\nBcc: evil@example.com",
            "a@example.com\nb@example.com",
        ] {
            let result = channel.send(bad, &notification()).await;
            assert!(
                matches!(result, Err(ChannelError::Permanent(_))),
                "{bad:?}: {result:?}"
            );
        }
    }

    #[tokio::test]
    async fn header_injection_is_impossible() {
        let (addr, server) = smtp_server(vec![Script::default()]).await;
        let channel = SmtpChannel::new(&config(addr)).unwrap();
        let evil = "x\r\nBcc: evil@example.com\r\n\r\nInjected";
        let n = Notification {
            host: evil.to_owned(),
            trigger: evil.to_owned(),
            summary: evil.to_owned(),
            instance: evil.to_owned(),
            ..notification()
        };
        channel.send(TO, &n).await.unwrap();
        let captured = finished(server).await;
        let data = &captured[0].data;
        let (headers, _) = data.split_once("\r\n\r\n").expect("header block");
        // Header names are the text before ":" on lines that do not continue a folded one.
        let names: Vec<String> = headers
            .lines()
            .filter(|l| !l.starts_with([' ', '\t']))
            .filter_map(|l| l.split_once(':').map(|(n, _)| n.to_ascii_lowercase()))
            .collect();
        assert!(!names.iter().any(|n| n == "bcc"), "{names:?}");
        assert!(names.iter().any(|n| n == "subject"), "{names:?}");
        assert!(
            !data
                .lines()
                .any(|l| l.to_ascii_lowercase().starts_with("bcc:")),
            "{data}"
        );
        assert!(
            !captured[0]
                .commands
                .iter()
                .any(|c| c.contains("evil@example.com")),
            "{:?}",
            captured[0].commands
        );
    }

    #[tokio::test]
    async fn failed_send_does_not_poison_the_next() {
        let failing = Script {
            rcpt: "451 greylisted",
            ..Script::default()
        };
        let (addr, server) = smtp_server(vec![failing, Script::default()]).await;
        let channel = SmtpChannel::new(&config(addr)).unwrap();
        assert!(matches!(
            channel.send(TO, &notification()).await,
            Err(ChannelError::Retryable(_))
        ));
        channel.send(TO, &notification()).await.unwrap();
        let captured = finished(server).await;
        assert!(captured[1].data.contains("Host: db-02"));
    }

    #[tokio::test]
    async fn starttls_is_required_not_optional() {
        // The server's EHLO answer does not offer STARTTLS.
        let (addr, server) = smtp_server(vec![Script::default()]).await;
        let cfg = SmtpConfig {
            encryption: SmtpEncryption::Starttls,
            ..config(addr)
        };
        let channel = SmtpChannel::new(&cfg).unwrap();
        let result = channel.send(TO, &notification()).await;
        assert!(
            matches!(result, Err(ChannelError::Retryable(_))),
            "{result:?}"
        );
        let captured = finished(server).await;
        let commands = &captured[0].commands;
        assert!(
            !commands.iter().any(|c| {
                let c = c.to_ascii_uppercase();
                c.starts_with("AUTH") || c.starts_with("MAIL") || c.starts_with("RCPT")
            }),
            "{commands:?}"
        );
    }

    #[test]
    fn new_does_not_connect_and_rejects_unusable_settings() {
        // Nothing listens there; building the channel must still work.
        let cfg = SmtpConfig {
            host: Some("127.0.0.1".to_owned()),
            port: 1,
            from: Some("cerbrix@example.com".to_owned()),
            ..SmtpConfig::default()
        };
        assert!(SmtpChannel::new(&cfg).is_ok());
        let no_host = SmtpConfig {
            host: None,
            ..cfg.clone()
        };
        assert!(SmtpChannel::new(&no_host).is_err());
        let bad_from = SmtpConfig {
            from: Some("nope".to_owned()),
            ..cfg.clone()
        };
        assert!(SmtpChannel::new(&bad_from).is_err());
        let plain_creds = SmtpConfig {
            encryption: SmtpEncryption::None,
            username: Some("bob".to_owned()),
            password: Some("pw".to_owned()),
            ..cfg
        };
        assert!(SmtpChannel::new(&plain_creds).is_err());
    }

    #[test]
    fn error_text_is_cleaned_cut_and_scrubbed() {
        let text = describe("line one\r\nBcc: x\u{7}\tend", &[]);
        assert!(!text.contains(char::is_control), "{text:?}");
        let long = "x".repeat(1000);
        assert_eq!(describe(&long, &[]).chars().count(), 300);
        let text = describe("auth failed for bob with hunter2", &["hunter2", "bob"]);
        assert!(!text.contains("hunter2") && !text.contains("bob"), "{text}");
    }
}
