//! Wall displays: the display files of the active configuration and their tokens.
//!
//! A display token is stored only as its SHA-256 hash, one per display. It works only while
//! the display exists in the active configuration; `apply_config` deletes the tokens of
//! displays that are gone.

use std::collections::HashMap;

use cerbrix_common::DisplayFile;
use sqlx::types::Json;
use time::OffsetDateTime;

use super::{StateError, StateStore};
use crate::tokens;

fn decode(body: serde_json::Value) -> Result<DisplayFile, StateError> {
    serde_json::from_value(body).map_err(|e| StateError::Db(sqlx::Error::Decode(e.into())))
}

impl StateStore {
    /// Every display of the active configuration as (id, source file, content), by id.
    pub async fn displays(&self) -> Result<Vec<(String, String, DisplayFile)>, StateError> {
        let rows: Vec<(String, String, Json<serde_json::Value>)> = sqlx::query_as(
            "SELECT id, source_file, body FROM config_documents \
             WHERE kind = 'display' ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|(id, file, body)| Ok((id, file, decode(body.0)?)))
            .collect()
    }

    /// One display of the active configuration: its source file and content.
    pub async fn display(&self, id: &str) -> Result<Option<(String, DisplayFile)>, StateError> {
        let row: Option<(String, Json<serde_json::Value>)> = sqlx::query_as(
            "SELECT source_file, body FROM config_documents WHERE kind = 'display' AND id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|(file, body)| Ok((file, decode(body.0)?)))
            .transpose()
    }

    /// The ids of all dashboards of the active configuration, sorted.
    pub async fn dashboard_ids(&self) -> Result<Vec<String>, StateError> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT id FROM config_documents WHERE kind = 'dashboard' ORDER BY id")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    /// Creates the token of a display and returns it; it is shown once and stored only as a
    /// hash. A token the display already has stops working. `NotFound` for a display that is
    /// not in the active configuration.
    ///
    /// Guarantee: a token is never stored for a display that a committed configuration
    /// removed. The display's `config_documents` row is locked (`FOR KEY SHARE`) before the
    /// token is written, so a concurrent `apply_config` (which deletes and re-inserts every
    /// document, then deletes the tokens of displays it no longer has) either waits for this
    /// call or is waited for by it, and this call then sees the committed truth. During a sync
    /// that keeps the display the old row is deleted, so the call can answer a spurious
    /// `NotFound`; it never hangs: `apply_config` takes the document rows before the token
    /// rows, this call takes them in the same order, and nothing waits for a token row.
    pub async fn create_display_token(
        &self,
        display_id: &str,
        now: OffsetDateTime,
    ) -> Result<String, StateError> {
        let plain = tokens::generate(tokens::DISPLAY_TOKEN_PREFIX);
        let mut tx = self.pool.begin().await?;
        let exists: Option<(i32,)> = sqlx::query_as(
            "SELECT 1 FROM config_documents WHERE kind = 'display' AND id = $1 FOR KEY SHARE",
        )
        .bind(display_id)
        .fetch_optional(&mut *tx)
        .await?;
        if exists.is_none() {
            return Err(StateError::NotFound);
        }
        sqlx::query(
            "INSERT INTO display_tokens (display_id, token_hash, created_at) \
             VALUES ($1, $2, $3) \
             ON CONFLICT (display_id) DO UPDATE \
             SET token_hash = EXCLUDED.token_hash, created_at = EXCLUDED.created_at",
        )
        .bind(display_id)
        .bind(tokens::hash(&plain).as_slice())
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(plain)
    }

    /// Deletes the token of a display. A display without one, or an unknown display, is not an
    /// error.
    pub async fn revoke_display_token(&self, display_id: &str) -> Result<(), StateError> {
        sqlx::query("DELETE FROM display_tokens WHERE display_id = $1")
            .bind(display_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The display a token belongs to; `None` for an unknown or revoked token, or one whose
    /// display has left the configuration.
    pub async fn display_for_token(&self, token: &str) -> Result<Option<String>, StateError> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT t.display_id FROM display_tokens t \
             JOIN config_documents d ON d.kind = 'display' AND d.id = t.display_id \
             WHERE t.token_hash = $1",
        )
        .bind(tokens::hash(token).as_slice())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(id,)| id))
    }

    /// When the token of a display was created; `None` without a token.
    pub async fn display_token_created(
        &self,
        display_id: &str,
    ) -> Result<Option<OffsetDateTime>, StateError> {
        let row: Option<(OffsetDateTime,)> =
            sqlx::query_as("SELECT created_at FROM display_tokens WHERE display_id = $1")
                .bind(display_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(at,)| at))
    }

    /// When the token of each display that has one was created.
    pub async fn display_tokens_created(
        &self,
    ) -> Result<HashMap<String, OffsetDateTime>, StateError> {
        let rows: Vec<(String, OffsetDateTime)> =
            sqlx::query_as("SELECT display_id, created_at FROM display_tokens")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;
    use time::Duration;

    use super::*;
    use crate::gitops::{compile, read_tree};

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
    }

    async fn store_with_display(pool: PgPool) -> StateStore {
        let store = StateStore::from_pool(pool.clone());
        sqlx::query(
            "INSERT INTO config_documents (kind, id, source_file, body) VALUES \
             ('display', 'noc', 'displays/noc.json', \
              '{\"name\": \"NOC\", \"show\": {\"dashboard\": \"main\"}}'::jsonb)",
        )
        .execute(&pool)
        .await
        .unwrap();
        store
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn token_is_stored_hashed_and_resolves(pool: PgPool) {
        let store = store_with_display(pool.clone()).await;
        let token = store.create_display_token("noc", now()).await.unwrap();
        assert!(token.starts_with("cbx_dt_"));
        assert_eq!(
            store.display_for_token(&token).await.unwrap().as_deref(),
            Some("noc")
        );
        assert_eq!(store.display_for_token("cbx_dt_wrong").await.unwrap(), None);
        let (hash, created): (Vec<u8>, OffsetDateTime) =
            sqlx::query_as("SELECT token_hash, created_at FROM display_tokens")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(hash, tokens::hash(&token));
        assert_ne!(hash, token.as_bytes());
        assert_eq!(created, now());
        assert_eq!(
            store.display_token_created("noc").await.unwrap(),
            Some(now())
        );
        assert_eq!(store.display_tokens_created().await.unwrap().len(), 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn new_token_replaces_the_old(pool: PgPool) {
        let store = store_with_display(pool).await;
        let old = store.create_display_token("noc", now()).await.unwrap();
        let later = now() + Duration::hours(1);
        let new = store.create_display_token("noc", later).await.unwrap();
        assert_ne!(old, new);
        assert_eq!(store.display_for_token(&old).await.unwrap(), None);
        assert_eq!(
            store.display_for_token(&new).await.unwrap().as_deref(),
            Some("noc")
        );
        assert_eq!(
            store.display_token_created("noc").await.unwrap(),
            Some(later)
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn revoke_ends_it(pool: PgPool) {
        let store = store_with_display(pool).await;
        let token = store.create_display_token("noc", now()).await.unwrap();
        store.revoke_display_token("noc").await.unwrap();
        assert_eq!(store.display_for_token(&token).await.unwrap(), None);
        assert_eq!(store.display_token_created("noc").await.unwrap(), None);
        // Idempotent, also for a display without a token and an unknown one.
        store.revoke_display_token("noc").await.unwrap();
        store.revoke_display_token("nobody").await.unwrap();
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn token_for_unknown_display_is_not_found(pool: PgPool) {
        let store = store_with_display(pool.clone()).await;
        assert!(matches!(
            store.create_display_token("nobody", now()).await,
            Err(StateError::NotFound)
        ));
        let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM display_tokens")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn removed_display_loses_its_token(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/config-repo");
        let compiled = compile(&read_tree(&dir, "c1").unwrap()).unwrap();
        store
            .apply_config("c1", &compiled, now(), 50, false)
            .await
            .unwrap();
        let displays = store.displays().await.unwrap();
        assert!(!displays.is_empty(), "the example has a display");
        let id = displays[0].0.clone();
        let token = store.create_display_token(&id, now()).await.unwrap();
        assert_eq!(
            store.display_for_token(&token).await.unwrap().as_deref(),
            Some(id.as_str())
        );

        // The next revision has no displays.
        let mut without = compiled.clone();
        without.documents.retain(|d| d.kind.as_str() != "display");
        store
            .apply_config("c2", &without, now(), 50, false)
            .await
            .unwrap();
        assert_eq!(store.display_for_token(&token).await.unwrap(), None);
        let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM display_tokens")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);

        // A display that stays keeps its token.
        store
            .apply_config("c3", &compiled, now(), 50, false)
            .await
            .unwrap();
        let token = store.create_display_token(&id, now()).await.unwrap();
        store
            .apply_config("c4", &compiled, now(), 50, false)
            .await
            .unwrap();
        assert!(store.display_for_token(&token).await.unwrap().is_some());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn displays_are_read_with_their_file(pool: PgPool) {
        let store = store_with_display(pool).await;
        let all = store.displays().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(
            (all[0].0.as_str(), all[0].1.as_str()),
            ("noc", "displays/noc.json")
        );
        assert_eq!(all[0].2.name, "NOC");
        assert!(store.display("noc").await.unwrap().is_some());
        assert!(store.display("x").await.unwrap().is_none());
    }

    /// A token created while a sync that removes the display is running must not survive it.
    /// The second transaction does what `apply_config` does to this display before its commit.
    #[sqlx::test(migrations = "./migrations")]
    async fn token_creation_waits_for_a_sync_that_removes_the_display(pool: PgPool) {
        let store = store_with_display(pool.clone()).await;
        let mut sync = pool.begin().await.unwrap();
        sqlx::query("DELETE FROM config_documents WHERE kind = 'display' AND id = 'noc'")
            .execute(&mut *sync)
            .await
            .unwrap();
        sqlx::query("DELETE FROM display_tokens WHERE display_id = 'noc'")
            .execute(&mut *sync)
            .await
            .unwrap();

        let creating = {
            let store = store.clone();
            tokio::spawn(async move { store.create_display_token("noc", now()).await })
        };
        // The call is blocked on a lock held by the open transaction: wait until PostgreSQL
        // reports a backend waiting for one, then check the call has not completed.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let (waiting,): (i64,) = sqlx::query_as(
                "SELECT count(*) FROM pg_stat_activity \
                 WHERE wait_event_type = 'Lock' AND datname = current_database()",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            if waiting > 0 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the call never waited for the lock"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(!creating.is_finished());

        sync.commit().await.unwrap();
        let result = creating.await.unwrap();
        assert!(matches!(result, Err(StateError::NotFound)), "{result:?}");
        let (n,): (i64,) =
            sqlx::query_as("SELECT count(*) FROM display_tokens WHERE display_id = 'noc'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(n, 0);
    }
}
