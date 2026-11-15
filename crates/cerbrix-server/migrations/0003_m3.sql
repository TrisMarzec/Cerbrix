ALTER TABLE hosts ADD COLUMN last_seen_at timestamptz;

CREATE TABLE trigger_states (
    host_id         uuid NOT NULL REFERENCES hosts (id) ON DELETE CASCADE,
    trigger_name    text NOT NULL,
    instance        text NOT NULL,
    firing          boolean NOT NULL,
    condition_since timestamptz,
    last_ts         timestamptz NOT NULL,
    last_value      double precision,
    PRIMARY KEY (host_id, trigger_name, instance)
);

CREATE TABLE problems (
    id              uuid PRIMARY KEY,
    host_id         uuid NOT NULL REFERENCES hosts (id) ON DELETE CASCADE,
    trigger_name    text NOT NULL,
    instance        text NOT NULL,
    severity        text NOT NULL CHECK (severity IN ('info', 'warning', 'high', 'critical')),
    summary         text NOT NULL,
    started_at      timestamptz NOT NULL,
    resolved_at     timestamptz,
    resolve_reason  text CHECK (resolve_reason IN ('condition_ended', 'host_removed', 'trigger_removed')),
    acknowledged_at timestamptz,
    acknowledged_by text,
    ack_comment     text
);
CREATE UNIQUE INDEX problems_one_open ON problems (host_id, trigger_name, instance) WHERE resolved_at IS NULL;
CREATE INDEX problems_started ON problems (started_at DESC);

CREATE TABLE series_seen (
    host_id      uuid NOT NULL REFERENCES hosts (id) ON DELETE CASCADE,
    item_key     text NOT NULL,
    instance     text NOT NULL,
    params       jsonb NOT NULL,
    last_arrival timestamptz NOT NULL,
    PRIMARY KEY (host_id, item_key, instance)
);

CREATE TABLE alerting_state (
    singleton    boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    heartbeat_at timestamptz,
    baseline_at  timestamptz
);
INSERT INTO alerting_state DEFAULT VALUES;

CREATE TABLE notification_outbox (
    id              bigserial PRIMARY KEY,
    problem_id      uuid NOT NULL REFERENCES problems (id) ON DELETE CASCADE,
    event           text NOT NULL CHECK (event IN ('started', 'resolved')),
    recipient       text NOT NULL,
    created_at      timestamptz NOT NULL,
    attempts        integer NOT NULL DEFAULT 0,
    next_attempt_at timestamptz NOT NULL,
    locked_until    timestamptz,
    sent_at         timestamptz,
    failed_at       timestamptz,
    last_error      text,
    UNIQUE (problem_id, event, recipient)
);
CREATE INDEX outbox_due ON notification_outbox (next_attempt_at) WHERE sent_at IS NULL AND failed_at IS NULL;
