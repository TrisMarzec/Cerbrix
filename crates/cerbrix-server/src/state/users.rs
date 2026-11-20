//! Users of the web UI. The password hash lives in the table but never in [`User`]; only
//! [`StateStore::user_password_hash`] reads it, for the code that verifies a login.

use std::fmt;

use sqlx::{PgConnection, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{StateError, StateStore, is_unique_violation};
use crate::auth::{Busy, PasswordGate, Role};
use crate::config::AuthConfig;
use crate::tokens;

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

/// When a session started at `now` ends at the latest.
fn session_end(now: OffsetDateTime, cfg: &AuthConfig) -> OffsetDateTime {
    let max = i64::try_from(cfg.session_max_secs).unwrap_or(i64::MAX);
    now + time::Duration::seconds(max)
}

/// Stores a new session of `user` that starts at `now`.
async fn insert_session(
    conn: &mut PgConnection,
    token_hash: &[u8],
    user: Uuid,
    now: OffsetDateTime,
    cfg: &AuthConfig,
) -> Result<(), StateError> {
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, created_at, last_seen_at, expires_at) \
         VALUES ($1, $2, $3, $3, $4)",
    )
    .bind(token_hash)
    .bind(user)
    .bind(now)
    .bind(session_end(now, cfg))
    .execute(conn)
    .await?;
    Ok(())
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

    /// One page of users ordered by name, and the number of all users.
    pub async fn users_page(
        &self,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<User>, i64), StateError> {
        let rows: Vec<UserRow> = sqlx::query_as(
            "SELECT id, name, role, disabled, created_at, password_changed_at \
             FROM users ORDER BY name LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        let total: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
            .fetch_one(&self.pool)
            .await?;
        let users = rows.into_iter().map(user_from).collect::<Result<_, _>>()?;
        Ok((users, total))
    }

    /// The user with this id, if there is one.
    pub async fn user_by_id(&self, id: Uuid) -> Result<Option<User>, StateError> {
        let row: Option<UserRow> = sqlx::query_as(
            "SELECT id, name, role, disabled, created_at, password_changed_at \
             FROM users WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(user_from).transpose()
    }

    /// Changes the role and/or the disabled flag in one transaction: either both apply or,
    /// when the result would leave no enabled admin (`Conflict`), neither. Ends the user's
    /// sessions when a role is given or the user is disabled, like the single changes do.
    pub async fn update_user(
        &self,
        id: Uuid,
        role: Option<Role>,
        disabled: Option<bool>,
    ) -> Result<User, StateError> {
        let mut tx = self.pool.begin().await?;
        let (user, only_admin) = lock_target(&mut tx, id).await?;
        if only_admin && (role.is_some_and(|r| r != Role::Admin) || disabled == Some(true)) {
            return Err(StateError::Conflict(LAST_ADMIN.to_owned()));
        }
        let role_now = role.unwrap_or(user.role);
        let disabled_now = disabled.unwrap_or(user.disabled);
        sqlx::query("UPDATE users SET role = $2, disabled = $3 WHERE id = $1")
            .bind(id)
            .bind(role_now.as_str())
            .bind(disabled_now)
            .execute(&mut *tx)
            .await?;
        if role.is_some() || disabled == Some(true) {
            end_sessions(&mut tx, id).await?;
        }
        tx.commit().await?;
        Ok(User {
            role: role_now,
            disabled: disabled_now,
            ..user
        })
    }

    /// Stores a new password hash, clears the failed-login state and ends the user's
    /// sessions.
    pub async fn set_user_password(
        &self,
        id: Uuid,
        password_hash: &str,
        now: OffsetDateTime,
    ) -> Result<(), StateError> {
        let tx = self.store_password(id, None, password_hash, now).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Like `set_user_password`, and in the same transaction starts a new session for the
    /// user, with the limits of a fresh login: a user changing the own password ends every
    /// session, the one of the request included, and goes on with the new one. Returns its
    /// token, once; only its hash is stored.
    ///
    /// Stores only while the user's hash is still `verified_hash`, the one the current
    /// password was checked against: a change or reset in the meantime wins, and this one is
    /// `Conflict` with nothing changed.
    pub async fn set_user_password_renewing_session(
        &self,
        id: Uuid,
        verified_hash: &str,
        password_hash: &str,
        now: OffsetDateTime,
        cfg: &AuthConfig,
    ) -> Result<String, StateError> {
        let mut tx = self
            .store_password(id, Some(verified_hash), password_hash, now)
            .await?;
        let token = tokens::generate("cbx_se_");
        insert_session(&mut tx, &tokens::hash(&token), id, now, cfg).await?;
        tx.commit().await?;
        Ok(token)
    }

    /// Stores the hash, clears the failed-login state and ends the sessions; the caller
    /// commits. With `expected`, only while the stored hash is that one: `Conflict` when it
    /// is another, `NotFound` when the user does not exist.
    async fn store_password(
        &self,
        id: Uuid,
        expected: Option<&str>,
        password_hash: &str,
        now: OffsetDateTime,
    ) -> Result<Transaction<'static, Postgres>, StateError> {
        let mut tx = self.pool.begin().await?;
        let done = sqlx::query(
            "UPDATE users SET password_hash = $2, password_changed_at = $3, \
             failed_logins = 0, locked_until = NULL \
             WHERE id = $1 AND ($4::text IS NULL OR password_hash = $4)",
        )
        .bind(id)
        .bind(password_hash)
        .bind(now)
        .bind(expected)
        .execute(&mut *tx)
        .await?;
        if done.rows_affected() == 0 {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1)")
                    .bind(id)
                    .fetch_one(&mut *tx)
                    .await?;
            return Err(if exists {
                StateError::Conflict("the password was changed in the meantime".to_owned())
            } else {
                StateError::NotFound
            });
        }
        end_sessions(&mut tx, id).await?;
        Ok(tx)
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

/// Failed attempts in a row that lock a user.
const LOCK_AFTER_FAILURES: i32 = 5;
/// How long a lock lasts.
const LOCK_DURATION: time::Duration = time::Duration::minutes(5);
/// `last_seen_at` is written at most this often.
const LAST_SEEN_GRANULARITY: time::Duration = time::Duration::seconds(60);

/// An Argon2id hash (default parameters) of a password nobody knows. Verifying against it
/// when the user does not exist or is disabled makes those logins cost the same as a wrong
/// password, so the response time does not reveal which names exist. A test checks that its
/// parameters are those of `hash_password`.
const DUMMY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$sCroc39RgVJSt99RBuIWKw$o9dQpQX9dfxpqmsrZLqIPUZiOjWECllDTn7SWiMoDfE";

/// The result of a login attempt. Debug output carries no token.
pub enum LoginOutcome {
    /// The user and the session token, returned once; only its hash is stored.
    Ok(User, String),
    /// Unknown or disabled user, or a wrong password; deliberately not distinguished.
    Wrong,
    /// Too many failed attempts: refused without looking at the password.
    Locked,
    /// No permit for the password check came in time: nothing was checked or counted.
    Busy,
}

impl fmt::Debug for LoginOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoginOutcome::Ok(user, _) => f
                .debug_tuple("Ok")
                .field(&user.name)
                .field(&"<token>")
                .finish(),
            LoginOutcome::Wrong => f.write_str("Wrong"),
            LoginOutcome::Locked => f.write_str("Locked"),
            LoginOutcome::Busy => f.write_str("Busy"),
        }
    }
}

/// The result of checking a user's password outside a login. Debug output carries no hash.
#[derive(Clone, PartialEq, Eq)]
pub enum PasswordCheck {
    /// The password matched this stored hash; a change made from it stores only while the
    /// hash is still this one.
    Right {
        hash: String,
    },
    Wrong,
    Locked,
    /// No permit for the check came in time: nothing was checked or counted.
    Busy,
}

impl fmt::Debug for PasswordCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PasswordCheck::Right { .. } => f.write_str("Right"),
            PasswordCheck::Wrong => f.write_str("Wrong"),
            PasswordCheck::Locked => f.write_str("Locked"),
            PasswordCheck::Busy => f.write_str("Busy"),
        }
    }
}

/// The first half of a login: what the password check found.
enum Verified {
    Wrong,
    Locked,
    Busy,
    /// The password matched this hash of this user.
    Right {
        id: Uuid,
        hash: String,
    },
}

type SessionRow = (
    Uuid,
    String,
    String,
    bool,
    OffsetDateTime,
    OffsetDateTime,
    OffsetDateTime,
    OffsetDateTime,
);

impl StateStore {
    /// Checks a user name and password and, when right, starts a session.
    ///
    /// An unknown or disabled user gets a password check against a dummy hash, so every
    /// `Wrong` takes about the same time, and nothing is changed for such a user. A locked
    /// user also gets the dummy check (so timing does not tell a lock from an unknown name)
    /// and is refused even with the right password.
    ///
    /// Every check takes a permit of `gate`; when none comes in time the outcome is `Busy`,
    /// for every name alike, and nothing is changed or counted.
    pub async fn login(
        &self,
        name: &str,
        password: &str,
        now: OffsetDateTime,
        cfg: &AuthConfig,
        gate: &PasswordGate,
    ) -> Result<LoginOutcome, StateError> {
        let verified = self.verify_login(name, password, now, gate).await?;
        self.finish_login(verified, now, cfg).await
    }

    /// The first half of a login: looks the user up and verifies the password.
    async fn verify_login(
        &self,
        name: &str,
        password: &str,
        now: OffsetDateTime,
        gate: &PasswordGate,
    ) -> Result<Verified, StateError> {
        let found: Option<(Uuid, bool, String, Option<OffsetDateTime>)> = sqlx::query_as(
            "SELECT id, disabled, password_hash, locked_until FROM users WHERE name = $1",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        let Some((id, false, hash, locked_until)) = found else {
            return Ok(
                match gate
                    .verify(password.to_owned(), DUMMY_HASH.to_owned())
                    .await
                {
                    Ok(_) => Verified::Wrong,
                    Err(Busy) => Verified::Busy,
                },
            );
        };
        Ok(
            match self
                .attempt(id, hash, locked_until, password, now, gate)
                .await?
            {
                PasswordCheck::Wrong => Verified::Wrong,
                PasswordCheck::Locked => Verified::Locked,
                PasswordCheck::Busy => Verified::Busy,
                PasswordCheck::Right { hash } => Verified::Right { id, hash },
            },
        )
    }

    /// The second half of a login: starts the session, but only while the user is still
    /// enabled and still has the password hash that was verified. A password change or
    /// reset between the two halves has already ended the sessions that existed, so a
    /// session created now from the old password would survive it; it gets none. The user
    /// row is locked `FOR SHARE`, so a change that is still running commits first and the
    /// condition sees its result.
    async fn finish_login(
        &self,
        verified: Verified,
        now: OffsetDateTime,
        cfg: &AuthConfig,
    ) -> Result<LoginOutcome, StateError> {
        let (id, hash) = match verified {
            Verified::Wrong => return Ok(LoginOutcome::Wrong),
            Verified::Locked => return Ok(LoginOutcome::Locked),
            Verified::Busy => return Ok(LoginOutcome::Busy),
            Verified::Right { id, hash } => (id, hash),
        };
        let token = tokens::generate("cbx_se_");
        let inserted = sqlx::query(
            "INSERT INTO sessions (token_hash, user_id, created_at, last_seen_at, expires_at) \
             SELECT $1, id, $3, $3, $4 FROM users \
             WHERE id = $2 AND NOT disabled AND password_hash = $5 FOR SHARE",
        )
        .bind(&tokens::hash(&token)[..])
        .bind(id)
        .bind(now)
        .bind(session_end(now, cfg))
        .bind(&hash)
        .execute(&self.pool)
        .await?;
        if inserted.rows_affected() != 1 {
            return Ok(LoginOutcome::Wrong);
        }
        let row: Option<UserRow> = sqlx::query_as(
            "SELECT id, name, role, disabled, created_at, password_changed_at \
             FROM users WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) => {
                let user = user_from(row)?;
                // The name only: never the password or the token.
                tracing::info!(user = %user.name, "user logged in");
                Ok(LoginOutcome::Ok(user, token))
            }
            None => Ok(LoginOutcome::Wrong),
        }
    }

    /// Checks `password` against the stored hash of a user with the same failure counting
    /// and lock as a login: a wrong password counts, a locked user is refused. `Busy`, with
    /// nothing counted, when `gate` gives no permit in time.
    pub async fn check_password(
        &self,
        id: Uuid,
        password: &str,
        now: OffsetDateTime,
        gate: &PasswordGate,
    ) -> Result<PasswordCheck, StateError> {
        let found: Option<(String, Option<OffsetDateTime>)> =
            sqlx::query_as("SELECT password_hash, locked_until FROM users WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        let Some((hash, locked_until)) = found else {
            return Ok(
                match gate
                    .verify(password.to_owned(), DUMMY_HASH.to_owned())
                    .await
                {
                    Ok(_) => PasswordCheck::Wrong,
                    Err(Busy) => PasswordCheck::Busy,
                },
            );
        };
        self.attempt(id, hash, locked_until, password, now, gate)
            .await
    }

    /// The shared part of a login and a password check. The password is verified before any
    /// state changes, so a `Busy` check changes nothing. The counter and the lock are each
    /// updated by one statement, so two concurrent wrong attempts count as two.
    async fn attempt(
        &self,
        id: Uuid,
        hash: String,
        locked_until: Option<OffsetDateTime>,
        password: &str,
        now: OffsetDateTime,
        gate: &PasswordGate,
    ) -> Result<PasswordCheck, StateError> {
        if locked_until.is_some_and(|until| until > now) {
            // The same work as for a wrong password.
            return Ok(
                match gate
                    .verify(password.to_owned(), DUMMY_HASH.to_owned())
                    .await
                {
                    Ok(_) => PasswordCheck::Locked,
                    Err(Busy) => PasswordCheck::Busy,
                },
            );
        }
        let Ok(right) = gate.verify(password.to_owned(), hash.clone()).await else {
            return Ok(PasswordCheck::Busy);
        };
        if right {
            // Only an unlocked user whose hash is still the verified one is reset: a lock
            // set meanwhile by concurrent wrong attempts wins, and so does a password
            // change.
            let reset = sqlx::query(
                "UPDATE users SET failed_logins = 0, locked_until = NULL \
                 WHERE id = $1 AND password_hash = $3 \
                   AND (locked_until IS NULL OR locked_until <= $2)",
            )
            .bind(id)
            .bind(now)
            .bind(&hash)
            .execute(&self.pool)
            .await?;
            return Ok(if reset.rows_affected() == 1 {
                PasswordCheck::Right { hash }
            } else {
                PasswordCheck::Locked
            });
        }
        // A counter left from before an expired lock starts again at one. While a lock is
        // running (a concurrent attempt set it) it is neither extended nor counted.
        let after: Option<(String, bool)> = sqlx::query_as(
            "UPDATE users SET \
               failed_logins = CASE WHEN locked_until > $2 THEN failed_logins \
                                    WHEN locked_until <= $2 THEN 1 \
                                    ELSE failed_logins + 1 END, \
               locked_until = CASE WHEN locked_until > $2 THEN locked_until \
                                   WHEN (CASE WHEN locked_until <= $2 THEN 1 \
                                              ELSE failed_logins + 1 END) >= $3 THEN $4 \
                                   ELSE NULL END \
             WHERE id = $1 RETURNING name, COALESCE(locked_until = $4, false)",
        )
        .bind(id)
        .bind(now)
        .bind(LOCK_AFTER_FAILURES)
        .bind(now + LOCK_DURATION)
        .fetch_optional(&self.pool)
        .await?;
        // The lock this very statement set (decided in SQL, so both sides of the comparison
        // are stored microsecond values) is logged,
        // once; attempts while it runs are refused above and log nothing.
        if let Some((name, true)) = after {
            tracing::warn!(user = %name, "user locked for 5 minutes after repeated failed logins");
        }
        Ok(PasswordCheck::Wrong)
    }

    /// The user a session token belongs to, or `None` for an unknown token, a disabled user,
    /// an expired session or one idle for too long. One query reads session and user;
    /// `last_seen_at` is written only when it is more than a minute old.
    pub async fn session_user(
        &self,
        token: &str,
        now: OffsetDateTime,
        cfg: &AuthConfig,
    ) -> Result<Option<User>, StateError> {
        let hash = tokens::hash(token);
        let row: Option<SessionRow> = sqlx::query_as(
            "SELECT u.id, u.name, u.role, u.disabled, u.created_at, u.password_changed_at, \
                    s.last_seen_at, s.expires_at \
             FROM sessions s JOIN users u ON u.id = s.user_id WHERE s.token_hash = $1",
        )
        .bind(&hash[..])
        .fetch_optional(&self.pool)
        .await?;
        let Some((id, name, role, disabled, created_at, changed_at, last_seen, expires)) = row
        else {
            return Ok(None);
        };
        let idle =
            time::Duration::seconds(i64::try_from(cfg.session_idle_secs).unwrap_or(i64::MAX));
        if disabled || now >= expires || now - last_seen > idle {
            return Ok(None);
        }
        if now - last_seen > LAST_SEEN_GRANULARITY {
            sqlx::query(
                "UPDATE sessions SET last_seen_at = $2 WHERE token_hash = $1 AND last_seen_at < $2",
            )
            .bind(&hash[..])
            .bind(now)
            .execute(&self.pool)
            .await?;
        }
        user_from((id, name, role, disabled, created_at, changed_at)).map(Some)
    }

    /// Ends one session; an unknown token is not an error.
    pub async fn logout(&self, token: &str) -> Result<(), StateError> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(&tokens::hash(token)[..])
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Deletes the sessions that have passed their absolute end; returns how many.
    pub async fn delete_expired_sessions(&self, now: OffsetDateTime) -> Result<u64, StateError> {
        let done = sqlx::query("DELETE FROM sessions WHERE expires_at <= $1")
            .bind(now)
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected())
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

#[cfg(test)]
mod login_tests {
    use super::*;
    use crate::auth::hash_password;
    use crate::config::AuthConfig;
    use crate::tokens;
    use sqlx::PgPool;
    use time::Duration;

    const PASSWORD: &str = "correct horse battery";

    fn t0() -> OffsetDateTime {
        // A fixed instant keeps the time arithmetic of the tests readable.
        OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
    }

    fn cfg() -> AuthConfig {
        AuthConfig {
            session_idle_secs: 600,
            session_max_secs: 3600,
            secure_cookie: None,
        }
    }

    async fn add(store: &StateStore, name: &str, role: Role) -> User {
        let hash = hash_password(PASSWORD.to_owned()).await.unwrap();
        store.create_user(name, role, &hash, t0()).await.unwrap()
    }

    async fn try_login(
        store: &StateStore,
        name: &str,
        pw: &str,
        at: OffsetDateTime,
    ) -> LoginOutcome {
        store
            .login(name, pw, at, &cfg(), &PasswordGate::default())
            .await
            .unwrap()
    }

    async fn failed(pool: &PgPool, id: Uuid) -> (i32, Option<OffsetDateTime>) {
        sqlx::query_as("SELECT failed_logins, locked_until FROM users WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    fn token_of(outcome: LoginOutcome) -> String {
        match outcome {
            LoginOutcome::Ok(_, token) => token,
            other => panic!("expected a login, got {other:?}"),
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn login_ok_creates_a_session(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let amy = add(&store, "amy", Role::Operator).await;
        let LoginOutcome::Ok(user, token) = try_login(&store, "amy", PASSWORD, t0()).await else {
            panic!("login must succeed");
        };
        assert_eq!(user, amy);
        assert!(token.starts_with("cbx_se_"), "{token}");
        let (expires, created): (OffsetDateTime, OffsetDateTime) =
            sqlx::query_as("SELECT expires_at, created_at FROM sessions")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(expires, t0() + Duration::seconds(3600));
        assert_eq!(created, t0());
        assert_eq!(
            store.session_user(&token, t0(), &cfg()).await.unwrap(),
            Some(amy)
        );
        // Two logins are two independent sessions.
        let other = token_of(try_login(&store, "amy", PASSWORD, t0()).await);
        assert_ne!(other, token);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn wrong_password_and_unknown_user_look_the_same(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        add(&store, "amy", Role::Viewer).await;
        let wrong = try_login(&store, "amy", "not the password", t0()).await;
        let unknown = try_login(&store, "nobody", PASSWORD, t0()).await;
        assert!(matches!(wrong, LoginOutcome::Wrong), "{wrong:?}");
        assert!(matches!(unknown, LoginOutcome::Wrong), "{unknown:?}");
        let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(sessions, 0);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn five_failures_lock_for_five_minutes(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let amy = add(&store, "amy", Role::Viewer).await;
        for _ in 0..5 {
            assert!(matches!(
                try_login(&store, "amy", "wrong", t0()).await,
                LoginOutcome::Wrong
            ));
        }
        let (count, locked) = failed(&pool, amy.id).await;
        assert_eq!(count, 5);
        assert_eq!(locked, Some(t0() + Duration::minutes(5)));
        // Even the right password is refused while locked, and the lock does not move.
        let during = t0() + Duration::minutes(2);
        assert!(matches!(
            try_login(&store, "amy", PASSWORD, during).await,
            LoginOutcome::Locked
        ));
        assert!(matches!(
            try_login(&store, "amy", "wrong", during).await,
            LoginOutcome::Locked
        ));
        assert_eq!(failed(&pool, amy.id).await.1, locked);
        // After five minutes the right password works.
        let after = t0() + Duration::minutes(5) + Duration::seconds(1);
        assert!(matches!(
            try_login(&store, "amy", PASSWORD, after).await,
            LoginOutcome::Ok(..)
        ));
        assert_eq!(failed(&pool, amy.id).await, (0, None));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_wrong_password_after_a_lock_counts_afresh(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let amy = add(&store, "amy", Role::Viewer).await;
        for _ in 0..5 {
            try_login(&store, "amy", "wrong", t0()).await;
        }
        let after = t0() + Duration::minutes(6);
        assert!(matches!(
            try_login(&store, "amy", "wrong", after).await,
            LoginOutcome::Wrong
        ));
        // One failure, not a new lock from the stale counter.
        assert_eq!(failed(&pool, amy.id).await, (1, None));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn success_resets_the_failure_count(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let amy = add(&store, "amy", Role::Viewer).await;
        for _ in 0..2 {
            for _ in 0..4 {
                try_login(&store, "amy", "wrong", t0()).await;
            }
            assert_eq!(failed(&pool, amy.id).await.0, 4);
            assert!(matches!(
                try_login(&store, "amy", PASSWORD, t0()).await,
                LoginOutcome::Ok(..)
            ));
            assert_eq!(failed(&pool, amy.id).await, (0, None));
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn concurrent_wrong_attempts_all_count(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let amy = add(&store, "amy", Role::Viewer).await;
        let mut tasks = Vec::new();
        for _ in 0..4 {
            let store = store.clone();
            tasks.push(tokio::spawn(async move {
                store
                    .login("amy", "wrong", t0(), &cfg(), &PasswordGate::default())
                    .await
                    .unwrap()
            }));
        }
        for task in tasks {
            assert!(matches!(task.await.unwrap(), LoginOutcome::Wrong));
        }
        assert_eq!(failed(&pool, amy.id).await.0, 4);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn disabled_user_cannot_log_in(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        add(&store, "boss", Role::Admin).await;
        let amy = add(&store, "amy", Role::Viewer).await;
        store.set_user_disabled(amy.id, true).await.unwrap();
        assert!(matches!(
            try_login(&store, "amy", PASSWORD, t0()).await,
            LoginOutcome::Wrong
        ));
        // Nothing about the user changes.
        assert_eq!(failed(&pool, amy.id).await, (0, None));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn session_expires_idle_and_absolute(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        add(&store, "amy", Role::Viewer).await;
        let alive = |token: &str, secs: i64| {
            let store = store.clone();
            let token = token.to_owned();
            async move {
                store
                    .session_user(&token, t0() + Duration::seconds(secs), &cfg())
                    .await
                    .unwrap()
                    .is_some()
            }
        };

        // Idle: 600 s without a request still counts, 601 s ends the session.
        let idle = token_of(try_login(&store, "amy", PASSWORD, t0()).await);
        assert!(alive(&idle, 600).await);
        assert!(
            alive(&idle, 1200).await,
            "the request at 600 s refreshed it"
        );
        assert!(!alive(&idle, 1801).await, "601 s since the last request");

        // Absolute: an active session still ends at expires_at.
        let busy = token_of(try_login(&store, "amy", PASSWORD, t0()).await);
        for secs in (500..3500).step_by(500) {
            assert!(alive(&busy, secs).await, "{secs}");
        }
        assert!(alive(&busy, 3599).await);
        assert!(!alive(&busy, 3600).await, "now >= expires_at");

        // An unknown token is nobody.
        assert!(!alive("cbx_se_unknown", 0).await);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_disabled_user_has_no_session(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let amy = add(&store, "amy", Role::Viewer).await;
        let token = token_of(try_login(&store, "amy", PASSWORD, t0()).await);
        // Disabling ends sessions; a session row that survived anyway is still refused.
        sqlx::query("UPDATE users SET disabled = true WHERE id = $1")
            .bind(amy.id)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            store.session_user(&token, t0(), &cfg()).await.unwrap(),
            None
        );
    }

    async fn last_seen(pool: &PgPool) -> OffsetDateTime {
        sqlx::query_scalar("SELECT last_seen_at FROM sessions")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn last_seen_is_refreshed_at_most_once_a_minute(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        add(&store, "amy", Role::Viewer).await;
        let token = token_of(try_login(&store, "amy", PASSWORD, t0()).await);
        let at = |secs: i64| t0() + Duration::seconds(secs);
        for secs in [1, 30, 60] {
            store.session_user(&token, at(secs), &cfg()).await.unwrap();
            assert_eq!(last_seen(&pool).await, t0(), "{secs} s: no write");
        }
        store.session_user(&token, at(61), &cfg()).await.unwrap();
        assert_eq!(last_seen(&pool).await, at(61));
        store.session_user(&token, at(100), &cfg()).await.unwrap();
        assert_eq!(last_seen(&pool).await, at(61));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn logout_ends_the_session(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        add(&store, "amy", Role::Viewer).await;
        let token = token_of(try_login(&store, "amy", PASSWORD, t0()).await);
        let other = token_of(try_login(&store, "amy", PASSWORD, t0()).await);
        store.logout(&token).await.unwrap();
        assert_eq!(
            store.session_user(&token, t0(), &cfg()).await.unwrap(),
            None
        );
        assert!(
            store
                .session_user(&other, t0(), &cfg())
                .await
                .unwrap()
                .is_some()
        );
        // Logging out twice, or with an unknown token, is not an error.
        store.logout(&token).await.unwrap();
        store.logout("cbx_se_unknown").await.unwrap();
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn only_the_hash_is_stored(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        add(&store, "amy", Role::Viewer).await;
        let token = token_of(try_login(&store, "amy", PASSWORD, t0()).await);
        let rows: Vec<String> = sqlx::query_scalar("SELECT s::text FROM sessions s")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].contains(&token), "{}", rows[0]);
        let stored: Vec<u8> = sqlx::query_scalar("SELECT token_hash FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, tokens::hash(&token).to_vec());
        // No user column holds the token or the password either.
        let users: Vec<String> = sqlx::query_scalar("SELECT u::text FROM users u")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(
            users
                .iter()
                .all(|u| !u.contains(&token) && !u.contains(PASSWORD))
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn expired_sessions_are_deleted(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        add(&store, "amy", Role::Viewer).await;
        let old = token_of(try_login(&store, "amy", PASSWORD, t0()).await);
        let new = token_of(try_login(&store, "amy", PASSWORD, t0() + Duration::hours(1)).await);
        let removed = store
            .delete_expired_sessions(t0() + Duration::seconds(3600))
            .await
            .unwrap();
        assert_eq!(removed, 1);
        let at = t0() + Duration::hours(1);
        assert_eq!(store.session_user(&old, at, &cfg()).await.unwrap(), None);
        assert!(
            store
                .session_user(&new, at, &cfg())
                .await
                .unwrap()
                .is_some()
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn changing_the_password_renews_the_session(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let amy = add(&store, "amy", Role::Viewer).await;
        let current = token_of(try_login(&store, "amy", PASSWORD, t0()).await);
        let other = token_of(try_login(&store, "amy", PASSWORD, t0()).await);
        let later = t0() + Duration::minutes(10);
        let verified = store.user_password_hash(amy.id).await.unwrap().unwrap();
        let renewed = store
            .set_user_password_renewing_session(amy.id, &verified, "$argon2id$new", later, &cfg())
            .await
            .unwrap();
        assert!(renewed.starts_with("cbx_se_"));
        for ended in [&current, &other] {
            assert_eq!(
                store.session_user(ended, later, &cfg()).await.unwrap(),
                None
            );
        }
        assert_eq!(
            store.session_user(&renewed, later, &cfg()).await.unwrap(),
            Some(User {
                password_changed_at: later,
                ..amy.clone()
            })
        );
        // The limits of a fresh login, counted from the change.
        let (created, expires): (OffsetDateTime, OffsetDateTime) =
            sqlx::query_as("SELECT created_at, expires_at FROM sessions")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!((created, expires), (later, later + Duration::seconds(3600)));
        assert_eq!(
            store.user_password_hash(amy.id).await.unwrap().as_deref(),
            Some("$argon2id$new")
        );
        // An unknown user gets no session.
        assert!(matches!(
            store
                .set_user_password_renewing_session(Uuid::new_v4(), &verified, "h", later, &cfg())
                .await,
            Err(StateError::NotFound)
        ));
        let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(sessions, 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_busy_gate_checks_and_counts_nothing(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let amy = add(&store, "amy", Role::Viewer).await;
        let gate = PasswordGate::new(1, std::time::Duration::from_millis(50));
        let held = gate.occupy();
        for password in [PASSWORD, "wrong"] {
            assert!(matches!(
                store
                    .login("amy", password, t0(), &cfg(), &gate)
                    .await
                    .unwrap(),
                LoginOutcome::Busy
            ));
            assert_eq!(
                store
                    .check_password(amy.id, password, t0(), &gate)
                    .await
                    .unwrap(),
                PasswordCheck::Busy
            );
        }
        assert!(matches!(
            store
                .login("nobody", PASSWORD, t0(), &cfg(), &gate)
                .await
                .unwrap(),
            LoginOutcome::Busy
        ));
        assert_eq!(failed(&pool, amy.id).await, (0, None));
        drop(held);
        assert!(matches!(
            store
                .login("amy", PASSWORD, t0(), &cfg(), &gate)
                .await
                .unwrap(),
            LoginOutcome::Ok(..)
        ));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn checking_a_password_counts_failures_like_a_login(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let amy = add(&store, "amy", Role::Viewer).await;
        let gate = PasswordGate::default();
        for _ in 0..5 {
            assert_eq!(
                store
                    .check_password(amy.id, "wrong", t0(), &gate)
                    .await
                    .unwrap(),
                PasswordCheck::Wrong
            );
        }
        assert_eq!(
            store
                .check_password(amy.id, PASSWORD, t0(), &gate)
                .await
                .unwrap(),
            PasswordCheck::Locked
        );
        assert!(matches!(
            try_login(&store, "amy", PASSWORD, t0()).await,
            LoginOutcome::Locked
        ));
        let later = t0() + Duration::minutes(6);
        let checked = store
            .check_password(amy.id, PASSWORD, later, &gate)
            .await
            .unwrap();
        let stored = store.user_password_hash(amy.id).await.unwrap();
        assert!(
            matches!(&checked, PasswordCheck::Right { hash } if Some(hash) == stored.as_ref()),
            "{checked:?}"
        );
        assert_eq!(failed(&pool, amy.id).await, (0, None));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn login_racing_a_password_change_gets_no_session(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let amy = add(&store, "amy", Role::Viewer).await;
        // The password is verified ...
        let verified = store
            .verify_login("amy", PASSWORD, t0(), &PasswordGate::default())
            .await
            .unwrap();
        assert!(matches!(verified, Verified::Right { .. }));
        // ... then changed (this ends the sessions that exist) ...
        store
            .set_user_password(amy.id, "$argon2id$new", t0())
            .await
            .unwrap();
        // ... and only then the session would be created, from the old password.
        let outcome = store.finish_login(verified, t0(), &cfg()).await.unwrap();
        assert!(matches!(outcome, LoginOutcome::Wrong), "{outcome:?}");
        let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(sessions, 0);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn password_change_racing_an_admin_reset_changes_nothing(pool: PgPool) {
        let store = StateStore::from_pool(pool.clone());
        let amy = add(&store, "amy", Role::Viewer).await;
        // The current password is verified ...
        let PasswordCheck::Right { hash } = store
            .check_password(amy.id, PASSWORD, t0(), &PasswordGate::default())
            .await
            .unwrap()
        else {
            panic!("the password is right");
        };
        // ... then an admin resets it ...
        store
            .set_user_password(amy.id, "$argon2id$reset", t0())
            .await
            .unwrap();
        // ... and only then the change would be stored, from the old password.
        let changed = store
            .set_user_password_renewing_session(amy.id, &hash, "$argon2id$new", t0(), &cfg())
            .await;
        assert!(
            matches!(changed, Err(StateError::Conflict(_))),
            "{changed:?}"
        );
        assert_eq!(
            store.user_password_hash(amy.id).await.unwrap().as_deref(),
            Some("$argon2id$reset")
        );
        let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(sessions, 0);
    }

    /// Everything the process logs while the tests run, from a subscriber installed once
    /// for the whole process (a per-test subscriber misses events when tests run in
    /// parallel). A test reads only the lines that name its own user.
    static LOGS: std::sync::Mutex<Vec<u8>> = std::sync::Mutex::new(Vec::new());

    struct LogWriter;

    impl std::io::Write for LogWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            LOGS.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn logged_lines_about(user: &str) -> Vec<String> {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            let _ = tracing::subscriber::set_global_default(
                tracing_subscriber::fmt()
                    .with_ansi(false)
                    .with_writer(|| LogWriter)
                    .finish(),
            );
        });
        let text = String::from_utf8(LOGS.lock().unwrap().clone()).unwrap();
        text.lines()
            .filter(|l| l.contains(user))
            .map(str::to_owned)
            .collect()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn lock_is_logged_once(pool: PgPool) {
        let user = "lock-log-user";
        logged_lines_about(user); // installs the subscriber before anything is logged
        let store = StateStore::from_pool(pool.clone());
        // Not a whole microsecond, like the clock in production: PostgreSQL stores less.
        let at = t0() + Duration::nanoseconds(123_456_789);
        add(&store, user, Role::Viewer).await;
        for _ in 0..4 {
            try_login(&store, user, "wrong", at).await;
        }
        assert!(logged_lines_about(user).is_empty());
        try_login(&store, user, "wrong", at).await; // the fifth sets the lock
        // Attempts while locked do not log again.
        for _ in 0..3 {
            try_login(&store, user, "wrong", at + Duration::minutes(1)).await;
        }
        let lines = logged_lines_about(user);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("WARN") && lines[0].contains("locked"),
            "{lines:?}"
        );
        assert!(!lines[0].contains("$argon2"), "{lines:?}");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn a_login_is_logged_with_the_name_only(pool: PgPool) {
        let user = "login-log-user";
        logged_lines_about(user); // installs the subscriber before anything is logged
        let store = StateStore::from_pool(pool);
        add(&store, user, Role::Viewer).await;
        try_login(&store, user, "wrong", t0()).await;
        assert!(logged_lines_about(user).is_empty(), "a failed login is not");
        let token = token_of(try_login(&store, user, PASSWORD, t0()).await);
        let lines = logged_lines_about(user);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("INFO") && lines[0].contains("logged in"),
            "{lines:?}"
        );
        assert!(
            !lines[0].contains(&token) && !lines[0].contains(PASSWORD),
            "{lines:?}"
        );
    }

    #[test]
    fn the_dummy_hash_costs_as_much_as_a_real_one() {
        use argon2::password_hash::PasswordHasher as _;
        let real = argon2::Argon2::default()
            .hash_password(b"x")
            .unwrap()
            .to_string();
        let real = argon2::PasswordHash::new(&real).unwrap();
        let dummy = argon2::PasswordHash::new(DUMMY_HASH).unwrap();
        assert_eq!(dummy.algorithm, real.algorithm);
        assert_eq!(dummy.version, real.version);
        assert_eq!(dummy.params, real.params);
    }
}
