CREATE TABLE users (
    id                  uuid PRIMARY KEY,
    name                text NOT NULL UNIQUE,
    role                text NOT NULL CHECK (role IN ('viewer', 'operator', 'admin')),
    password_hash       text NOT NULL,
    disabled            boolean NOT NULL DEFAULT false,
    failed_logins       integer NOT NULL DEFAULT 0,
    locked_until        timestamptz,
    created_at          timestamptz NOT NULL,
    password_changed_at timestamptz NOT NULL
);

CREATE TABLE sessions (
    token_hash   bytea PRIMARY KEY,
    user_id      uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at   timestamptz NOT NULL,
    last_seen_at timestamptz NOT NULL,
    expires_at   timestamptz NOT NULL
);
CREATE INDEX sessions_user ON sessions (user_id);

CREATE TABLE display_tokens (
    display_id text PRIMARY KEY,
    token_hash bytea NOT NULL UNIQUE,
    created_at timestamptz NOT NULL
);

ALTER TABLE problems ADD COLUMN kind text NOT NULL DEFAULT 'compare' CHECK (kind IN ('compare', 'no_data'));
CREATE INDEX problems_open_host ON problems (host_id) WHERE resolved_at IS NULL;
