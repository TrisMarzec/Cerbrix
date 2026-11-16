//! Enrolment keys and unknown agents.

use time::OffsetDateTime;
use uuid::Uuid;

use super::{StateError, StateStore};
use crate::tokens;

/// An agent that tried to enrol under a name no host has.
#[derive(Debug, Clone, PartialEq)]
pub struct UnknownAgent {
    pub host_name: String,
    pub first_seen: OffsetDateTime,
    pub last_seen: OffsetDateTime,
    pub attempts: i64,
}

/// An enrolment key as stored; the key itself is known only to its holder.
#[derive(Debug, Clone, PartialEq)]
pub struct EnrollmentKey {
    pub id: Uuid,
    pub description: String,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub revoked_at: Option<OffsetDateTime>,
}

type KeyRow = (
    Uuid,
    String,
    OffsetDateTime,
    OffsetDateTime,
    Option<OffsetDateTime>,
);

fn key_from(row: KeyRow) -> EnrollmentKey {
    EnrollmentKey {
        id: row.0,
        description: row.1,
        created_at: row.2,
        expires_at: row.3,
        revoked_at: row.4,
    }
}

impl StateStore {
    /// Creates an enrolment key and returns its id and the plain key (shown once).
    pub async fn create_enrollment_key(
        &self,
        description: &str,
        expires_at: OffsetDateTime,
    ) -> Result<(Uuid, String), StateError> {
        let (key, plain) = self
            .create_enrollment_key_full(description, expires_at)
            .await?;
        Ok((key.id, plain))
    }

    /// Like `create_enrollment_key`, but returns the whole key as stored, read by the one
    /// statement that creates it: once it exists, nothing can fail before the plain key is
    /// handed out.
    pub async fn create_enrollment_key_full(
        &self,
        description: &str,
        expires_at: OffsetDateTime,
    ) -> Result<(EnrollmentKey, String), StateError> {
        let id = Uuid::new_v4();
        let plain = tokens::generate(tokens::ENROLLMENT_KEY_PREFIX);
        let row: KeyRow = sqlx::query_as(
            "INSERT INTO enrollment_keys (id, key_hash, description, expires_at) \
             VALUES ($1, $2, $3, $4) \
             RETURNING id, description, created_at, expires_at, revoked_at",
        )
        .bind(id)
        .bind(tokens::hash(&plain).as_slice())
        .bind(description)
        .bind(expires_at)
        .fetch_one(&self.pool)
        .await?;
        Ok((key_from(row), plain))
    }

    /// Revokes a key; revoking an already revoked key keeps its first revocation time.
    pub async fn revoke_enrollment_key(&self, id: Uuid) -> Result<(), StateError> {
        let done = sqlx::query(
            "UPDATE enrollment_keys SET revoked_at = COALESCE(revoked_at, now()) WHERE id = $1",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StateError::NotFound);
        }
        Ok(())
    }

    /// True when the key exists, is not revoked and `now` is before its expiry.
    pub async fn enrollment_key_valid(
        &self,
        plain: &str,
        now: OffsetDateTime,
    ) -> Result<bool, StateError> {
        let found: Option<i32> = sqlx::query_scalar(
            "SELECT 1 FROM enrollment_keys \
             WHERE key_hash = $1 AND revoked_at IS NULL AND expires_at > $2",
        )
        .bind(tokens::hash(plain).as_slice())
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;
        Ok(found.is_some())
    }

    /// Records an enrolment attempt of an unknown agent, counting repeats.
    pub async fn record_unknown_agent(
        &self,
        host_name: &str,
        now: OffsetDateTime,
    ) -> Result<(), StateError> {
        sqlx::query(
            "INSERT INTO unknown_agents (host_name, first_seen, last_seen, attempts) \
             VALUES ($1, $2, $2, 1) \
             ON CONFLICT (host_name) DO UPDATE \
             SET last_seen = EXCLUDED.last_seen, attempts = unknown_agents.attempts + 1",
        )
        .bind(host_name)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Every enrolment key, newest first. Never the key or its hash.
    pub async fn enrollment_keys(&self) -> Result<Vec<EnrollmentKey>, StateError> {
        let rows: Vec<KeyRow> = sqlx::query_as(
            "SELECT id, description, created_at, expires_at, revoked_at FROM enrollment_keys \
             ORDER BY created_at DESC, id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(key_from).collect())
    }

    /// The agents that tried to enrol under an unknown name, most recent first, at most
    /// `limit`.
    pub async fn unknown_agents_recent(&self, limit: i64) -> Result<Vec<UnknownAgent>, StateError> {
        let rows: Vec<(String, OffsetDateTime, OffsetDateTime, i64)> = sqlx::query_as(
            "SELECT host_name, first_seen, last_seen, attempts FROM unknown_agents \
             ORDER BY last_seen DESC, host_name LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(host_name, first_seen, last_seen, attempts)| UnknownAgent {
                    host_name,
                    first_seen,
                    last_seen,
                    attempts,
                },
            )
            .collect())
    }

    pub async fn unknown_agents(&self) -> Result<Vec<UnknownAgent>, StateError> {
        let rows: Vec<(String, OffsetDateTime, OffsetDateTime, i64)> = sqlx::query_as(
            "SELECT host_name, first_seen, last_seen, attempts FROM unknown_agents \
             ORDER BY last_seen DESC, host_name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(host_name, first_seen, last_seen, attempts)| UnknownAgent {
                    host_name,
                    first_seen,
                    last_seen,
                    attempts,
                },
            )
            .collect())
    }
}
