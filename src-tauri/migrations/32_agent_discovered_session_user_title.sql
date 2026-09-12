-- A locally chosen name for a discovered session. Discovery rescans refresh
-- `title` from the provider's own store (Hermes auto-titles from the first
-- user message), so the user's rename must live in a column the upsert never
-- touches. Display prefers `user_title` when present.
ALTER TABLE agent_discovered_sessions ADD COLUMN user_title TEXT;
