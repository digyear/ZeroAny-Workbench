-- Bastions like JumpServer drop you into an asset-selection menu instead of a
-- host shell: until that menu is answered, anything typed (including an
-- agent's command) goes to the menu. These columns let a profile replay a
-- short script after the shell opens, e.g. the asset's number or its IP.
ALTER TABLE ssh_profiles ADD COLUMN post_connect_inputs TEXT;
ALTER TABLE ssh_profiles ADD COLUMN post_connect_delay_ms INTEGER NOT NULL DEFAULT 800;
