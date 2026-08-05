-- Preserve the provider-native cwd while storing a stable repository identity
-- for grouping main checkouts, subdirectories, and linked worktrees together.
ALTER TABLE agent_sessions ADD COLUMN project_root TEXT;
