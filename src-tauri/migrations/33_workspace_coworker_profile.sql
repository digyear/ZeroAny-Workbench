-- Hermes coworker profile selection. NULL/empty = default profile.
-- Only meaningful for provider='hermes'; other providers ignore it.
ALTER TABLE workspace_coworkers ADD COLUMN profile TEXT;
