-- Quick-start sessions (product plan §5) + per-project defaults (decision D2).
--
-- `initial_prompt` is the one-shot first message captured by the Quick Bar.
-- It is consumed exactly once, on the first spawn of a fresh session, then
-- cleared so reopening the tab never re-sends it.
ALTER TABLE agent_sessions ADD COLUMN initial_prompt TEXT;

-- Spawn lifecycle for the session row: idle | starting | running | exited | failed.
-- Written by the spawn path, read by the list/detail UI and the companion.
ALTER TABLE agent_sessions ADD COLUMN spawn_state TEXT NOT NULL DEFAULT 'idle';

-- Remembered defaults per project, so a new session only needs a prompt.
-- Two granularities coexist (setting `agent_preferences_scope`):
--   scope_kind = 'project_root' | 'worktree', scope_key = the respective path.
CREATE TABLE IF NOT EXISTS agent_project_preferences (
    scope_kind       TEXT NOT NULL,
    scope_key        TEXT NOT NULL,
    provider         TEXT,
    profile          TEXT,
    purpose          TEXT,
    base_branch      TEXT,
    worktree_enabled INTEGER,
    context_names    TEXT,
    git_name         TEXT,
    git_email        TEXT,
    skip_permissions INTEGER,
    updated_at       TEXT NOT NULL,
    PRIMARY KEY (scope_kind, scope_key)
);