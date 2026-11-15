//! Hosts and agent credentials.

use uuid::Uuid;

use super::{Host, StateError, StateStore, is_unique_violation};
use crate::tokens;

type HostRow = (Uuid, String, bool);

fn host_from(row: HostRow) -> Host {
    Host {
        id: row.0,
        name: row.1,
        active: row.2,
    }
}

impl StateStore {
    /// Creates an active host; `Conflict` when the name already exists (case-sensitive).
    pub async fn create_host(&self, name: &str) -> Result<Host, StateError> {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO hosts (id, name) VALUES ($1, $2)")
            .bind(id)
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                if is_unique_violation(&e) {
                    StateError::Conflict(format!("host {name:?} already exists"))
                } else {
                    e.into()
                }
            })?;
        Ok(Host {
            id,
            name: name.to_owned(),
            active: true,
        })
    }

    pub async fn host_by_name(&self, name: &str) -> Result<Option<Host>, StateError> {
        let row: Option<HostRow> =
            sqlx::query_as("SELECT id, name, active FROM hosts WHERE name = $1")
                .bind(name)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(host_from))
    }

    pub async fn set_host_active(&self, id: Uuid, active: bool) -> Result<(), StateError> {
        let done = sqlx::query("UPDATE hosts SET active = $2 WHERE id = $1")
            .bind(id)
            .bind(active)
            .execute(&self.pool)
            .await?;
        if done.rows_affected() == 0 {
            return Err(StateError::NotFound);
        }
        Ok(())
    }

    /// Binds a new credential to the host and returns the plain value (shown once).
    ///
    /// The primary key on `host_id` decides the race: `Conflict` when the host is already bound.
    pub async fn bind_credential(&self, host_id: Uuid) -> Result<String, StateError> {
        let plain = tokens::generate(tokens::AGENT_CREDENTIAL_PREFIX);
        let done = sqlx::query(
            "INSERT INTO agent_credentials (host_id, credential_hash) \n             SELECT id, $2 FROM hosts WHERE id = $1 AND active FOR SHARE",
        )
            .bind(host_id)
            .bind(tokens::hash(&plain).as_slice())
            .execute(&self.pool)
            .await
            .map_err(|e| {
                if is_unique_violation(&e) {
                    StateError::Conflict("host is already bound to a credential".to_owned())
                } else if e
                    .as_database_error()
                    .is_some_and(|d| d.is_foreign_key_violation())
                {
                    StateError::NotFound
                } else {
                    e.into()
                }
            })?;
        if done.rows_affected() == 0 {
            return Err(StateError::NotFound);
        }
        Ok(plain)
    }

    /// Removes the host's credential so it can enrol again; idempotent.
    pub async fn release_binding(&self, host_id: Uuid) -> Result<(), StateError> {
        sqlx::query("DELETE FROM agent_credentials WHERE host_id = $1")
            .bind(host_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Resolves a credential to its host; only active hosts resolve.
    pub async fn host_for_credential(&self, plain: &str) -> Result<Option<Host>, StateError> {
        let row: Option<HostRow> = sqlx::query_as(
            "SELECT h.id, h.name, h.active FROM agent_credentials c \
             JOIN hosts h ON h.id = c.host_id \
             WHERE c.credential_hash = $1 AND h.active",
        )
        .bind(tokens::hash(plain).as_slice())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(host_from))
    }
}
