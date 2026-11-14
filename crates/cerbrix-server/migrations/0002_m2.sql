ALTER TABLE hosts
    ADD COLUMN address     text,
    ADD COLUMN groups      text[] NOT NULL DEFAULT '{}',
    ADD COLUMN source_file text,
    ADD COLUMN config      jsonb;

CREATE TABLE config_documents (
    kind        text NOT NULL CHECK (kind IN ('template', 'notification', 'dashboard', 'display')),
    id          text NOT NULL,
    source_file text NOT NULL,
    body        jsonb NOT NULL,
    PRIMARY KEY (kind, id)
);

CREATE TABLE sync_state (
    singleton        boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    status           text NOT NULL CHECK (status IN ('never', 'in_sync', 'out_of_sync')),
    active_commit    text,
    last_success_at  timestamptz,
    last_attempt_at  timestamptz,
    attempted_commit text,
    issues           jsonb NOT NULL DEFAULT '[]'
);
INSERT INTO sync_state (status) VALUES ('never');

CREATE TABLE sync_history (
    id         bigserial PRIMARY KEY,
    commit     text NOT NULL,
    applied_at timestamptz NOT NULL,
    changes    jsonb NOT NULL
);
