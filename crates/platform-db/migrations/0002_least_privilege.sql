-- Least privilege for the runtime role (F1). Migrations run as the owner role
-- (`platform_owner`); the server and the worker run as `platform_app`, which is no
-- superuser, owns nothing, and holds only the table rights below. On operation_events it
-- may only read and append: UPDATE, DELETE and TRUNCATE are not granted, and the
-- triggers that also refuse them fire even in replica mode (ENABLE ALWAYS), so
-- `session_replication_role = replica` (superuser-only anyway) cannot silence them.
-- `platform-dbctl bootstrap` creates both roles before the first migration.

REVOKE CREATE ON SCHEMA public FROM PUBLIC;
GRANT USAGE ON SCHEMA public TO platform_app;

GRANT SELECT, INSERT, UPDATE ON desired_states, operations, operation_steps, observed_resources
    TO platform_app;
GRANT SELECT, INSERT ON operation_events TO platform_app;

ALTER TABLE operation_events ENABLE ALWAYS TRIGGER operation_events_append_only;
ALTER TABLE operation_events ENABLE ALWAYS TRIGGER operation_events_no_truncate;
