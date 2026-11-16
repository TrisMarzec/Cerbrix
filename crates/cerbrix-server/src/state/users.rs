//! Users of the web UI. The password hash lives in the table but never in [`User`]; only
//! [`StateStore::user_password_hash`] reads it, for the code that verifies a login.

use sqlx::{PgConnection, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{StateError, StateStore, is_unique_violation};
use crate::auth::Role;

const LAST_ADMIN: &str = "the last enabled admin cannot be changed";

/// A user. Carries no password hash.
#[derive(Debug, Clone, PartialEq)]
pub struct User {
    pub id: Uuid,
    pub name: String,
    pub role: Role,
    pub disabled: bool,
    pub created_at: OffsetDateTime,
    pub password_changed_at: OffsetDateTime,
}

type UserRow = (Uuid, String, String, bool, OffsetDateTime, OffsetDateTime);

fn user_from(row: UserRow) -> Result<User, StateError> {
    let role = row
        .2
        .parse::<Role>()
        .map_err(|e| StateError::Db(sqlx::Error::Decode(e.into())))?;
    Ok(User {
        id: row.0,
        name: row.1,
        role,
        disabled: row.3,
        created_at: row.4,
        password_changed_at: row.5,
    })
}

/// Locks every enabled admin (in a fixed order, so concurrent changes cannot deadlock) and
/// then the target user. Because the admin rows stay locked until the transaction ends, two
/// concurrent changes cannot both remove the last two admins. Returns the target and whether
/// it is the only enabled admin.
async fn lock_target(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<(User, bool), StateError> {
    let admins: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM users WHERE role = 'admin' AND NOT disabled ORDER BY id FOR UPDATE",
    )
    .fetch_all(&mut **tx)
    .await?;
    let row: Option<UserRow> = sqlx::query_as(
        "SELECT id, name, role, disabled, created_at, password_changed_at \
         FROM users WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?;
    let user = user_from(row.ok_or(StateError::NotFound)?)?;
    let only_admin = admins.len() == 1 && admins[0] == id;
    Ok((user, only_admin))
}

async fn end_sessions(conn: &mut PgConnection, user: Uuid) -> Result<(), StateError> {
    sqlx::query("DELETE FROM sessions WHERE user_id = $1")
        .bind(user)
        .execute(conn)
        .await?;
    Ok(())
}

impl StateStore {
    /// Creates an enabled user; `Conflict` when the name is taken.
    pub async fn create_user(
        &self,
        name: &str,
        role: Role,
        password_hash: &str,
        now: OffsetDateTime,
    ) -> Result<User, StateError> {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (id, name, role, password_hash, created_at, password_changed_at) \
             VALUES ($1, $2, $3, $4, $5, $5)",
        )
        .bind(id)
        .bind(name)
        .bind(role.as_str())
        .bind(password_hash)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            if is_unique_violation(&e) {
                StateError::Conflict(format!("user {name:?} already exists"))
            } else {
                e.into()
            }
        })?;
        let row: UserRow = sqlx::query_as("SELECT id, name, role, disabled, created_at, password_changed_at FROM users WHERE id = $1")
            .bind(id)
            .fetch_one(&self.pool)
            .await?;
        user_from(row)
    }

    /// All users, by name.
    pub async fn users(&self) -> Result<Vec<User>, StateError> {
        let rows: Vec<UserRow> =
            sqlx::query_as("SELECT id, name, role, disabled, created_at, password_changed_at FROM users ORDER BY name")
                .fetch_all(&self.pool)
                .await?;
        rows.into_iter().map(user_from).collect()
    }

    pub async fn user_by_name(&self, name: &str) -> Result<Option<User>, StateError> {
        let row: Option<UserRow> =
            sqlx::query_as("SELECT id, name, role, disabled, created_at, password_changed_at FROM users WHERE name = $1")
                .bind(name)
                .fetch_optional(&self.pool)
                .await?;
        row.map(user_from).transpose()
    }

    /// The stored Argon2id hash of a user, for verifying a login. Never put it in a log,
    /// an error or a response.
    pub async fn user_password_hash(&self, id: Uuid) -> Result<Option<String>, StateError> {
        Ok(
            sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    /// Changes the role and ends the user's sessions. `Conflict` when that would leave no
    /// enabled admin.
    pub async fn set_user_role(&self, id: Uuid, role: Role) -> Result<User, StateError> {
        let mut tx = self.pool.begin().await?;
        let (user, only_admin) = lock_target(&mut tx, id).await?;
        if only_admin && role != Role::Admin {
            return Err(StateError::Conflict(LAST_ADMIN.to_owned()));
        }
        sqlx::query("UPDATE users SET role = $2 WHERE id = $1")
            .bind(id)
            .bind(role.as_str())
            .execute(&mut *tx)
            .await?;
        end_sessions(&mut tx, id).await?;
        tx.commit().await?;
        Ok(User { role, ..user })
    }

    /// Disables or enables a user. Disabling ends the user's sessions and is refused for the
    /// last enabled admin; enabling is always allowed.
    pub async fn set_user_disabled(&self, id: Uuid, disabled: bool) -> Result<User, StateError> {
        let mut tx = self.pool.begin().await?;
        let (user, only_admin) = lock_target(&mut tx, id).await?;
        if only_admin && disabled {
            return Err(StateError::Conflict(LAST_ADMIN.to_owned()));
        }
        sqlx::query("UPDATE users SET disabled = $2 WHERE id = $1")
            .bind(id)
            .bind(disabled)
            .execute(&mut *tx)
            .await?;
        if disabled {
            end_sessions(&mut tx, id).await?;
        }
        tx.commit().await?;
        Ok(User { disabled, ..user })
    }

    /// Stores a new password hash, clears the failed-login state and ends the user's
    /// sessions.
    pub async fn set_user_password(
        &self,
        id: Uuid,
        password_hash: &str,
        now: OffsetDateTime,
    ) -> Result<(), StateError> {
        let mut tx = self.pool.begin().await?;
        let done = sqlx::query(
            "UPDATE users SET password_hash = $2, password_changed_at = $3, \
             failed_logins = 0, locked_until = NULL WHERE id = $1",
        )
        .bind(id)
        .bind(password_hash)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StateError::NotFound);
        }
        end_sessions(&mut tx, id).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Deletes a user and the user's sessions. `Conflict` for the last enabled admin.
    pub async fn delete_user(&self, id: Uuid) -> Result<(), StateError> {
        let mut tx = self.pool.begin().await?;
        let (_, only_admin) = lock_target(&mut tx, id).await?;
        if only_admin {
            return Err(StateError::Conflict(LAST_ADMIN.to_owned()));
        }
        end_sessions(&mut tx, id).await?;
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;
    use time::OffsetDateTime;

    fn now() -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }

    async fn add(store: &StateStore, name: &str, role: Role) -> User {
        store
            .create_user(name, role, "$argon2id$stored-hash", now())
            .await
            .unwrap()
    }

    async fn add_session(pool: &PgPool, user: Uuid) {
        let t = now();
        sqlx::query(
            "INSERT INTO sessions (token_hash, user_id, created_at, last_seen_at, expires_at) \
             VALUES ($1, $2, $3, $3, $3)",
        )
        .bind(Uuid::new_v4().as_bytes().to_vec())
        .bind(user)
        .bind(t)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn sessions(pool: &PgPool) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM sessions")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn create_and_find_user(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let t = now();
        let created = store
            .create_user("zed", Role::Operator, "$argon2id$h", t)
            .await
            .unwrap();
        assert_eq!(created.name, "zed");
        assert_eq!(created.role, Role::Operator);
        assert!(!created.disabled);
        assert_eq!(created.password_changed_at, created.created_at);
        add(&store, "amy", Role::Viewer).await;
        assert_eq!(
            store.user_by_name("zed").await.unwrap(),
            Some(created.clone())
        );
        assert_eq!(store.user_by_name("nobody").await.unwrap(), None);
        let names: Vec<_> = store
            .users()
            .await
            .unwrap()
            .into_iter()
            .map(|u| u.name)
            .collect();
        assert_eq!(names, ["amy", "zed"]);
        assert_eq!(
            store
                .user_password_hash(created.id)
                .await
                .unwrap()
                .as_deref(),
            Some("$argon2id$h")
        );
        assert_eq!(
            store.user_password_hash(Uuid::new_v4()).await.unwrap(),
            None
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn duplicate_name_conflicts(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        add(&store, "amy", Role::Viewer).await;
        assert!(matches!(
            store.create_user("amy", Role::Admin, "h", now()).await,
            Err(StateError::Conflict(_))
        ));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn last_admin_is_protected(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let root = add(&store, "root", Role::Admin).await;
        add(&store, "viewer", Role::Viewer).await;
        let refused = |r: Result<_, StateError>| match r {
            Err(StateError::Conflict(m)) => {
                assert_eq!(m, "the last enabled admin cannot be changed")
            }
            Err(e) => panic!("{e}"),
            Ok(_) => panic!("must be refused"),
        };
        refused(store.set_user_role(root.id, Role::Operator).await.map(drop));
        refused(store.set_user_disabled(root.id, true).await.map(drop));
        refused(store.delete_user(root.id).await);
        assert_eq!(
            store.user_by_name("root").await.unwrap(),
            Some(root.clone())
        );

        // Promoting to admin and keeping the role never hit the rule.
        assert_eq!(
            store.set_user_role(root.id, Role::Admin).await.unwrap(),
            root
        );
        let second = add(&store, "second", Role::Admin).await;
        store.set_user_role(root.id, Role::Operator).await.unwrap();
        // `second` is now the only one again.
        refused(store.set_user_disabled(second.id, true).await.map(drop));
        store.set_user_role(root.id, Role::Admin).await.unwrap();
        let disabled = store.set_user_disabled(second.id, true).await.unwrap();
        assert!(disabled.disabled);
        // A disabled admin does not count: root is the only enabled one again.
        refused(store.delete_user(root.id).await);
        // Enabling is always allowed, and then everything works.
        assert!(
            !store
                .set_user_disabled(second.id, false)
                .await
                .unwrap()
                .disabled
        );
        store.delete_user(root.id).await.unwrap();
        assert_eq!(store.user_by_name("root").await.unwrap(), None);
        // Non-admins and disabled admins can be changed freely.
        store.set_user_disabled(second.id, true).await.unwrap_err();
        let viewer = store.user_by_name("viewer").await.unwrap().unwrap();
        store.set_user_disabled(viewer.id, true).await.unwrap();
        store.delete_user(viewer.id).await.unwrap();
        assert!(matches!(
            store.delete_user(viewer.id).await,
            Err(StateError::NotFound)
        ));
        assert!(matches!(
            store.set_user_role(Uuid::new_v4(), Role::Viewer).await,
            Err(StateError::NotFound)
        ));
        assert!(matches!(
            store.set_user_disabled(Uuid::new_v4(), true).await,
            Err(StateError::NotFound)
        ));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn concurrent_removal_of_two_admins_keeps_one(pool: PgPool) {
        let store = StateStore::from_pool(pool);
        let a = add(&store, "a", Role::Admin).await;
        let b = add(&store, "b", Role::Admin).await;
        for round in 0..10 {
            let (s1, s2) = (store.clone(), store.clone());
            let (r1, r2) = tokio::join!(
                tokio::spawn(async move { s1.set_user_role(a.id, Role::Viewer).await }),
                tokio::spawn(async move { s2.set_user_role(b.id, Role::Viewer).await }),
            );
            let results = [r1.unwrap(), r2.unwrap()];
            let oks = results.iter().filter(|r| r.is_ok()).count();
            let conflicts = results
                .iter()
                .filter(|r| matches!(r, Err(StateError::Conflict(_))))
                .count();
            assert_eq!((oks, conflicts), (1, 1), "round {round}");
            let admins = store
                .users()
                .await
                .unwrap()
                .into_iter()
                .filter(|u| u.role == Role::Admin && !u.disabled)
                .count();
            assert_eq!(admins, 1, "round {round}");
            // Promoting is always allowed: back to two admins for the next round.
            store.set_user_role(a.id, Role::Admin).await.unwrap();
            store.set_user_role(b.id, Role::Admin).await.unwrap();
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn changes_end_sessions(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        add(&store, "boss", Role::Admin).await;
        let u = add(&store, "amy", Role::Operator).await;

        add_session(&pool, u.id).await;
        let t = now() + time::Duration::minutes(1);
        store
            .set_user_password(u.id, "$argon2id$new", t)
            .await
            .unwrap();
        assert_eq!(sessions(&pool).await, 0, "password change");
        assert_eq!(
            store.user_password_hash(u.id).await.unwrap().as_deref(),
            Some("$argon2id$new")
        );
        let changed = store.user_by_name("amy").await.unwrap().unwrap();
        assert!(changed.password_changed_at > changed.created_at);

        add_session(&pool, u.id).await;
        store.set_user_role(u.id, Role::Viewer).await.unwrap();
        assert_eq!(sessions(&pool).await, 0, "role change");

        add_session(&pool, u.id).await;
        store.set_user_disabled(u.id, true).await.unwrap();
        assert_eq!(sessions(&pool).await, 0, "disable");

        add_session(&pool, u.id).await;
        store.delete_user(u.id).await.unwrap();
        assert_eq!(sessions(&pool).await, 0, "delete");

        assert!(matches!(
            store.set_user_password(Uuid::new_v4(), "h", now()).await,
            Err(StateError::NotFound)
        ));
    }
}
