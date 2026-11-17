-- Functions resolve names through a fixed search_path (F16): a trigger never runs with
-- whatever search_path the calling session happens to have.
ALTER FUNCTION refuse_operation_event_change() SET search_path = pg_catalog;
