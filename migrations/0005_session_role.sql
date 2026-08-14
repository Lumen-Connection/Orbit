-- Backfill the session role from NULL/empty to the default Coder.
-- The `role` column already exists (nullable) since migration 0001; this
-- migration only normalizes existing rows so AgentRole::from_id degrades cleanly.
UPDATE session SET role = 'coder' WHERE role IS NULL OR role = '';