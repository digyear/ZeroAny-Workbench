-- Manual nesting for managed sessions (drag & drop in the left nav).
-- References agent_sessions.id (internal UUID) — stable, unlike provider
-- resume ids which drift with compression/branch tips.
-- Tri-state: NULL = auto (fall back to fork lineage from discovery),
-- '' = explicitly detached to top-level (suppresses auto nesting),
-- '<uuid>' = manual child of that session.
ALTER TABLE agent_sessions ADD COLUMN parent_session_id TEXT;
