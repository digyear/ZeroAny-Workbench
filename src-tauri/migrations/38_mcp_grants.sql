-- MCP exposure grants.
--
-- Which saved connections an external agent may reach through the MCP
-- server, and how far it may go. Deliberately resource-kind agnostic:
-- SSH ships first, SQL connections reuse the same row shape (kind='sql')
-- and the same scope vocabulary, so the UI affordance and the enforcement
-- seam stay in one place.
--
-- Absence of a row means "not exposed" — deny by default, so a connection
-- the user saved for themselves never leaks into the MCP tools.
CREATE TABLE IF NOT EXISTS mcp_grants (
    resource_kind TEXT NOT NULL,          -- 'ssh' | 'sql'
    resource_id   TEXT NOT NULL,          -- profile id / connection id
    enabled       INTEGER NOT NULL DEFAULT 0,
    scope         TEXT NOT NULL DEFAULT 'read_only',  -- 'read_only' | 'service_control' | 'full'
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    PRIMARY KEY (resource_kind, resource_id)
);

CREATE INDEX IF NOT EXISTS idx_mcp_grants_kind_enabled
    ON mcp_grants (resource_kind, enabled);