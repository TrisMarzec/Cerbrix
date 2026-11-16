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
UPDATE problems SET kind = 'no_data' WHERE summary LIKE 'no data for %' OR summary LIKE '%: no data for %';
ALTER TABLE problems ADD COLUMN closed_by text, ADD COLUMN close_comment text;
-- M3 recorded a manual close of an unacknowledged problem as its acknowledgement, written by
-- the close itself: resolve reason closed_by_operator and acknowledged_at = resolved_at. Move
-- the closer to the close fields; such a problem was never acknowledged. A problem
-- acknowledged before it was closed kept that acknowledgement in M3, and its closer was not
-- recorded: it stays as it is.
UPDATE problems
   SET closed_by = acknowledged_by, close_comment = ack_comment,
       acknowledged_at = NULL, acknowledged_by = NULL, ack_comment = NULL
 WHERE resolve_reason = 'closed_by_operator' AND acknowledged_at = resolved_at
   AND closed_by IS NULL;
