-- Fork lineage for managed sessions: the provider-native external id of the
-- session this one was forked from (Hermes `_branched_from`, remapped to the
-- parent's visible compression tip at discovery time). Copied from the
-- discovered row on adopt; NULL for manually created sessions. The left nav
-- uses it to nest forked sessions under their parent within a project group.
ALTER TABLE agent_sessions ADD COLUMN parent_external_session_id TEXT;

-- Backfill for sessions adopted before this column existed: the discovered
-- catalog row (linked via adopted_agent_session_id) carries the fork parent.
-- Pre-upgrade scans stored the tip's parent (a compression predecessor for
-- compressed chains) — that id never matches a visible row, so such rows
-- simply stay flat; genuine fork children get their real parent id.
UPDATE agent_sessions
SET parent_external_session_id = (
    SELECT d.parent_external_session_id
    FROM agent_discovered_sessions d
    WHERE d.adopted_agent_session_id = agent_sessions.id
    LIMIT 1
)
WHERE parent_external_session_id IS NULL
  AND EXISTS (
    SELECT 1 FROM agent_discovered_sessions d
    WHERE d.adopted_agent_session_id = agent_sessions.id
      AND d.parent_external_session_id IS NOT NULL
  );
