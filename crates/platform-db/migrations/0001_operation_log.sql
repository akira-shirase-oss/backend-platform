-- The control database's B0 schema: desired state, the operation log (operations, their
-- recorded steps, an append-only event log) and observed resources. Desired and observed
-- are separate tables (invariant 1); status is derived from both.

CREATE TABLE desired_states (
    project     text PRIMARY KEY CHECK (project ~ '^[a-z][a-z0-9]{2,19}$'),
    revision    bigint NOT NULL CHECK (revision > 0),
    spec        jsonb NOT NULL,
    updated_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE operations (
    id               uuid PRIMARY KEY,
    seq              bigint GENERATED ALWAYS AS IDENTITY UNIQUE,
    project          text NOT NULL REFERENCES desired_states (project),
    revision         bigint NOT NULL CHECK (revision > 0),
    desired          jsonb NOT NULL,
    idempotency_key  text CHECK (length(idempotency_key) BETWEEN 1 AND 128),
    state            text NOT NULL DEFAULT 'pending'
                     CHECK (state IN ('pending', 'running', 'succeeded', 'partial')),
    round            integer NOT NULL DEFAULT 0 CHECK (round >= 0),
    -- The lease: who may write this operation's steps. lease_token is a fencing token,
    -- incremented by every claim; every write names the token it holds.
    lease_owner      uuid,
    lease_token      bigint NOT NULL DEFAULT 0,
    lease_expires_at timestamptz,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),
    finished_at      timestamptz,
    UNIQUE (project, idempotency_key)
);
CREATE INDEX operations_unfinished ON operations (seq) WHERE state IN ('pending', 'running');
CREATE INDEX operations_unfinished_by_project ON operations (project, seq)
    WHERE state IN ('pending', 'running');
CREATE INDEX operations_by_project ON operations (project, seq DESC);

CREATE TABLE operation_steps (
    operation_id   uuid NOT NULL REFERENCES operations (id),
    position       smallint NOT NULL CHECK (position >= 0),
    kind           text NOT NULL
                   CHECK (kind IN ('database_role', 'database', 'bucket', 'oidc_realm', 'oidc_client')),
    logical_name   text NOT NULL,
    provider_name  text NOT NULL,
    spec           jsonb NOT NULL,
    state          text NOT NULL DEFAULT 'pending'
                   CHECK (state IN ('pending', 'uncertain', 'done', 'blocked', 'gave_up')),
    attempts       smallint NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    resource_id    text,
    adopted        boolean,
    block_reason   text CHECK (block_reason IN ('conflict', 'rejected')),
    last_error     text,
    updated_at     timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (operation_id, position),
    -- A step is done exactly when a real provider id is recorded.
    CHECK ((state = 'done') = (resource_id IS NOT NULL)),
    CHECK ((state = 'blocked') = (block_reason IS NOT NULL))
);

-- Append-only: what happened to each operation, in order. Never updated or deleted.
CREATE TABLE operation_events (
    id            bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    operation_id  uuid NOT NULL REFERENCES operations (id),
    position      smallint,
    event         text NOT NULL,
    attempts      smallint,
    detail        text,
    lease_token   bigint,
    at            timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX operation_events_by_operation ON operation_events (operation_id, id);

CREATE FUNCTION refuse_operation_event_change() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'operation_events is append-only (% refused)', TG_OP
        USING ERRCODE = 'insufficient_privilege';
END;
$$;
CREATE TRIGGER operation_events_append_only
    BEFORE UPDATE OR DELETE ON operation_events
    FOR EACH ROW EXECUTE FUNCTION refuse_operation_event_change();
CREATE TRIGGER operation_events_no_truncate
    BEFORE TRUNCATE ON operation_events
    FOR EACH STATEMENT EXECUTE FUNCTION refuse_operation_event_change();

-- What providers reported, keyed by the deterministic name: at most one row per name.
CREATE TABLE observed_resources (
    kind           text NOT NULL,
    provider_name  text NOT NULL,
    project        text NOT NULL,
    resource_id    text NOT NULL,
    operation_id   uuid NOT NULL REFERENCES operations (id),
    observed_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (kind, provider_name)
);
CREATE INDEX observed_resources_by_project ON observed_resources (project);
