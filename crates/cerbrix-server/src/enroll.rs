//! Agent enrolment rules.

use cerbrix_common::{EnrollRequest, EnrollResponse, validate_host_name};
use time::OffsetDateTime;

use crate::state::{StateError, StateStore};

/// Why an enrolment was refused.
#[derive(Debug, thiserror::Error)]
pub enum EnrollError {
    #[error("invalid host name")]
    InvalidHostName,
    #[error("invalid enrolment key")]
    InvalidKey,
    #[error("unknown host")]
    UnknownHost,
    #[error("host is already enrolled")]
    AlreadyBound,
    #[error(transparent)]
    State(#[from] StateError),
}

impl EnrollError {
    /// Stable machine-readable code for the HTTP layer.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidHostName => "invalid_host_name",
            Self::InvalidKey => "invalid_key",
            Self::UnknownHost => "unknown_host",
            Self::AlreadyBound => "already_bound",
            Self::State(_) => "internal",
        }
    }
}

/// The longest agent version stored.
const MAX_AGENT_VERSION_CHARS: usize = 64;

/// True for an agent version that may be stored: 1 to 64 characters of `[0-9A-Za-z.+-]`.
/// The `User-Agent` of the agent's requests and the enrolment request are held to this rule.
pub fn valid_agent_version(version: &str) -> bool {
    (1..=MAX_AGENT_VERSION_CHARS).contains(&version.len())
        && version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'))
}

/// Enrols a host and returns its new credential. The version the agent reports is stored when
/// it is valid; an invalid one is stored as NULL and does not fail the enrolment.
///
/// Checks run in this order: host name format, key validity, host lookup. Only a request with
/// a valid name and a valid key is recorded as an unknown agent, so callers without a key
/// cannot fill the list.
pub async fn enroll(
    store: &StateStore,
    req: &EnrollRequest,
    now: OffsetDateTime,
) -> Result<EnrollResponse, EnrollError> {
    validate_host_name(&req.host).map_err(|_| EnrollError::InvalidHostName)?;
    if !store.enrollment_key_valid(&req.enrollment_key, now).await? {
        return Err(EnrollError::InvalidKey);
    }
    let host = match store.host_by_name(&req.host).await? {
        Some(host) if host.active => host,
        _ => {
            store.record_unknown_agent(&req.host, now).await?;
            return Err(EnrollError::UnknownHost);
        }
    };
    // The version is informational: it is written in the same statement as the binding, so a
    // failure there cannot leave a bound credential that was never returned to the agent.
    let version = valid_agent_version(&req.agent_version).then_some(req.agent_version.as_str());
    match store.bind_credential_with_version(host.id, version).await {
        Ok(credential) => Ok(EnrollResponse { credential }),
        Err(StateError::Conflict(_)) => Err(EnrollError::AlreadyBound),
        Err(StateError::NotFound) => {
            store.record_unknown_agent(&req.host, now).await?;
            Err(EnrollError::UnknownHost)
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::StateStore;
    use cerbrix_common::EnrollRequest;
    use sqlx::PgPool;
    use time::{Duration, OffsetDateTime};

    fn now() -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }

    fn req(host: &str, key: &str) -> EnrollRequest {
        EnrollRequest {
            host: host.to_owned(),
            enrollment_key: key.to_owned(),
            agent_version: "0.1.0".to_owned(),
        }
    }

    async fn setup(pool: PgPool) -> (StateStore, String) {
        let store = StateStore::from_pool(pool);
        let (_, key) = store
            .create_enrollment_key("t", now() + Duration::hours(1))
            .await
            .unwrap();
        (store, key)
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn enrols_known_host_with_valid_key(pool: PgPool) {
        let (store, key) = setup(pool).await;
        let host = store.create_host("web-01").await.unwrap();
        let resp = enroll(&store, &req("web-01", &key), now()).await.unwrap();
        assert!(resp.credential.starts_with("cbx_ac_"));
        let found = store.host_for_credential(&resp.credential).await.unwrap();
        assert_eq!(found, Some(host));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn inactive_host_cannot_enrol(pool: PgPool) {
        let (store, key) = setup(pool).await;
        let host = store.create_host("web-01").await.unwrap();
        store.set_host_active(host.id, false).await.unwrap();
        let r = enroll(&store, &req("web-01", &key), now()).await;
        assert!(matches!(r, Err(EnrollError::UnknownHost)));
        assert!(store.host_for_credential("x").await.unwrap().is_none());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn expired_key_is_invalid(pool: PgPool) {
        let (store, key) = setup(pool).await;
        store.create_host("web-01").await.unwrap();
        let later = now() + Duration::hours(2);
        let r = enroll(&store, &req("web-01", &key), later).await;
        assert!(matches!(r, Err(EnrollError::InvalidKey)));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn revoked_key_is_invalid(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        store.create_host("web-01").await.unwrap();
        let (id, key) = store
            .create_enrollment_key("t", now() + Duration::hours(1))
            .await
            .unwrap();
        store.revoke_enrollment_key(id).await.unwrap();
        let r = enroll(&store, &req("web-01", &key), now()).await;
        assert!(matches!(r, Err(EnrollError::InvalidKey)));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn second_enrolment_is_rejected_and_first_credential_still_works(pool: PgPool) {
        let (store, key) = setup(pool).await;
        store.create_host("web-01").await.unwrap();
        let first = enroll(&store, &req("web-01", &key), now()).await.unwrap();
        let r = enroll(&store, &req("web-01", &key), now()).await;
        assert!(matches!(r, Err(EnrollError::AlreadyBound)));
        let found = store.host_for_credential(&first.credential).await.unwrap();
        assert_eq!(found.map(|h| h.name), Some("web-01".to_owned()));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn release_allows_new_enrolment_and_invalidates_old_credential(pool: PgPool) {
        let (store, key) = setup(pool).await;
        let host = store.create_host("web-01").await.unwrap();
        let first = enroll(&store, &req("web-01", &key), now()).await.unwrap();
        store.release_binding(host.id).await.unwrap();
        assert!(
            store
                .host_for_credential(&first.credential)
                .await
                .unwrap()
                .is_none()
        );
        let second = enroll(&store, &req("web-01", &key), now()).await.unwrap();
        assert_ne!(first.credential, second.credential);
        assert!(
            store
                .host_for_credential(&second.credential)
                .await
                .unwrap()
                .is_some()
        );
    }

    async fn stored_version(pool: &PgPool, name: &str) -> Option<String> {
        sqlx::query_scalar("SELECT agent_version FROM hosts WHERE name = $1")
            .bind(name)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn enrolment_stores_the_reported_version_and_release_clears_it(pool: PgPool) {
        let (store, key) = setup(pool.clone()).await;
        let host = store.create_host("web-01").await.unwrap();
        assert_eq!(stored_version(&pool, "web-01").await, None);
        enroll(&store, &req("web-01", &key), now()).await.unwrap();
        assert_eq!(
            stored_version(&pool, "web-01").await.as_deref(),
            Some("0.1.0")
        );
        store.release_binding(host.id).await.unwrap();
        assert_eq!(stored_version(&pool, "web-01").await, None);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn an_invalid_version_is_stored_as_null_and_enrols(pool: PgPool) {
        let (store, key) = setup(pool.clone()).await;
        let host = store.create_host("web-01").await.unwrap();
        store
            .set_agent_version(host.id, Some("9.9.9"))
            .await
            .unwrap();
        let mut bad = req("web-01", &key);
        bad.agent_version = "1.0 <b>".to_owned();
        let resp = enroll(&store, &bad, now()).await.unwrap();
        assert!(resp.credential.starts_with("cbx_ac_"));
        assert_eq!(stored_version(&pool, "web-01").await, None);
    }

    // A failing version write cannot be provoked without invasive changes, so this proves the
    // version is written atomically with the binding: a refused binding leaves the stored
    // version untouched, and a successful one stores it in the same statement.
    #[sqlx::test(migrations = "./migrations")]
    async fn the_version_is_written_atomically_with_the_binding(pool: PgPool) {
        let (store, key) = setup(pool.clone()).await;
        let host = store.create_host("web-01").await.unwrap();
        enroll(&store, &req("web-01", &key), now()).await.unwrap();
        let mut second = req("web-01", &key);
        second.agent_version = "2.0.0".to_owned();
        assert!(matches!(
            enroll(&store, &second, now()).await,
            Err(EnrollError::AlreadyBound)
        ));
        assert_eq!(
            stored_version(&pool, "web-01").await.as_deref(),
            Some("0.1.0")
        );
        assert!(matches!(
            store
                .bind_credential_with_version(host.id, Some("3.0.0"))
                .await,
            Err(crate::state::StateError::Conflict(_))
        ));
        assert_eq!(
            stored_version(&pool, "web-01").await.as_deref(),
            Some("0.1.0")
        );
    }

    #[test]
    fn the_version_rule() {
        assert!(valid_agent_version("1.2.3"));
        assert!(valid_agent_version("0.1.0-rc.1+b5"));
        assert!(valid_agent_version(&"a".repeat(64)));
        for bad in [
            "",
            "1 2",
            "1.0
",
            "é",
            &"a".repeat(65),
        ] {
            assert!(!valid_agent_version(bad), "{bad:?}");
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn unknown_host_is_recorded_only_with_valid_key(pool: PgPool) {
        let (store, key) = setup(pool).await;
        let r = enroll(&store, &req("ghost", &key), now()).await;
        assert!(matches!(r, Err(EnrollError::UnknownHost)));
        assert_eq!(store.unknown_agents().await.unwrap().len(), 1);
        let r = enroll(&store, &req("ghost2", "cbx_ek_wrong"), now()).await;
        assert!(matches!(r, Err(EnrollError::InvalidKey)));
        assert_eq!(store.unknown_agents().await.unwrap().len(), 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn inactive_host_counts_as_unknown(pool: PgPool) {
        let (store, key) = setup(pool).await;
        let host = store.create_host("web-01").await.unwrap();
        store.set_host_active(host.id, false).await.unwrap();
        let r = enroll(&store, &req("web-01", &key), now()).await;
        assert!(matches!(r, Err(EnrollError::UnknownHost)));
        let list = store.unknown_agents().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].host_name, "web-01");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn bad_host_name_is_rejected_before_key_check(pool: PgPool) {
        let (store, _) = setup(pool).await;
        let r = enroll(&store, &req("web 01", "cbx_ek_wrong"), now()).await;
        assert!(matches!(r, Err(EnrollError::InvalidHostName)));
        assert!(store.unknown_agents().await.unwrap().is_empty());
    }

    #[test]
    fn error_codes() {
        assert_eq!(EnrollError::InvalidHostName.code(), "invalid_host_name");
        assert_eq!(EnrollError::InvalidKey.code(), "invalid_key");
        assert_eq!(EnrollError::UnknownHost.code(), "unknown_host");
        assert_eq!(EnrollError::AlreadyBound.code(), "already_bound");
        assert_eq!(
            EnrollError::State(crate::state::StateError::NotFound).code(),
            "internal"
        );
    }
}
