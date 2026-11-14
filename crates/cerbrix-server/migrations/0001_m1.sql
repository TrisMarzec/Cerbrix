CREATE TABLE hosts (
    id         uuid PRIMARY KEY,
    name       text NOT NULL UNIQUE,
    active     boolean NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE enrollment_keys (
    id          uuid PRIMARY KEY,
    key_hash    bytea NOT NULL UNIQUE,
    description text NOT NULL,
    expires_at  timestamptz NOT NULL,
    revoked_at  timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE agent_credentials (
    host_id         uuid PRIMARY KEY REFERENCES hosts (id) ON DELETE CASCADE,
    credential_hash bytea NOT NULL UNIQUE,
    enrolled_at     timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE unknown_agents (
    host_name  text PRIMARY KEY,
    first_seen timestamptz NOT NULL,
    last_seen  timestamptz NOT NULL,
    attempts   bigint NOT NULL DEFAULT 1
);
