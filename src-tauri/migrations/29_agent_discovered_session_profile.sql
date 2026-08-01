-- Hermes session ids are scoped to a provider-native profile. Rebuild the
-- discovery catalog so rows from default and named profiles can coexist and
-- adoption can retain the owning profile for deterministic resume.
CREATE TABLE agent_discovered_sessions_v29 (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    profile TEXT NOT NULL DEFAULT '',
    external_session_id TEXT NOT NULL,
    project_path TEXT,
    project_root TEXT,
    project_name TEXT,
    title TEXT,
    preview TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    parent_external_session_id TEXT,
    session_kind TEXT,
    source_path TEXT,
    hidden INTEGER NOT NULL DEFAULT 0,
    hidden_at TEXT,
    adopted_agent_session_id TEXT,
    UNIQUE(provider, profile, external_session_id),
    FOREIGN KEY (adopted_agent_session_id) REFERENCES agent_sessions(id) ON DELETE SET NULL
);

INSERT INTO agent_discovered_sessions_v29 (
    id, provider, profile, external_session_id, project_path, project_root,
    project_name, title, preview, created_at, updated_at, last_seen_at,
    parent_external_session_id, session_kind, source_path, hidden, hidden_at,
    adopted_agent_session_id
)
SELECT
    CASE
        WHEN provider = 'hermes' THEN provider || ':default:' || external_session_id
        ELSE id
    END,
    provider,
    CASE WHEN provider = 'hermes' THEN 'default' ELSE '' END,
    external_session_id, project_path, project_root, project_name, title,
    preview, created_at, updated_at, last_seen_at, parent_external_session_id,
    session_kind, source_path, hidden, hidden_at, adopted_agent_session_id
FROM agent_discovered_sessions;

DROP TABLE agent_discovered_sessions;
ALTER TABLE agent_discovered_sessions_v29 RENAME TO agent_discovered_sessions;

CREATE INDEX idx_agent_discovered_sessions_provider_project
    ON agent_discovered_sessions(provider, project_name, updated_at DESC);
CREATE INDEX idx_agent_discovered_sessions_last_seen
    ON agent_discovered_sessions(last_seen_at DESC);
CREATE INDEX idx_agent_discovered_sessions_adopted
    ON agent_discovered_sessions(adopted_agent_session_id)
    WHERE adopted_agent_session_id IS NOT NULL;
