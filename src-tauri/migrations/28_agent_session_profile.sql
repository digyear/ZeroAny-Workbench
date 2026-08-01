-- Optional provider-native profile identity. Hermes uses this to keep each
-- managed session bound to the profile whose isolated config/state.db owns it.
-- NULL preserves legacy behavior for providers without profiles and for
-- existing Hermes rows that historically followed the CLI's sticky default.
ALTER TABLE agent_sessions ADD COLUMN profile TEXT;
