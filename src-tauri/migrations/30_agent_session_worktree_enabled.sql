-- Let manual Agent sessions opt out of the automatically created isolated
-- worktree. Existing sessions keep the historical behavior.
ALTER TABLE agent_sessions ADD COLUMN worktree_enabled INTEGER NOT NULL DEFAULT 1;
