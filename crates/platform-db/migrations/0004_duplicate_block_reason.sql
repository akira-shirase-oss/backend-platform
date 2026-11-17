-- A step can be blocked because the provider holds its name more than once (F5).
ALTER TABLE operation_steps DROP CONSTRAINT operation_steps_block_reason_check;
ALTER TABLE operation_steps ADD CONSTRAINT operation_steps_block_reason_check
    CHECK (block_reason IN ('conflict', 'rejected', 'duplicate'));
