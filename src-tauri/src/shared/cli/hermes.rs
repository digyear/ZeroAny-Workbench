// Hermes Agent CLI implementation of [`CliRunner`].
//
// Hermes starts interactively with the bare `hermes` command. Sessions are
// resumed with the global `--resume <session-id>` option and approvals can be
// bypassed with `--yolo`. Project instructions live in `AGENTS.md`; Agent
// mode writes those before spawn rather than trying to pass a system prompt.
// Hermes stores session metadata in a profile-specific `state.db`, so
// discovery is implemented in `modes/agent/usage.rs` and filtered by both the
// selected profile and session cwd.

use std::path::{Path, PathBuf};

use super::runner::{CliRunner, SpawnOpts};

pub struct HermesRunner;

const BINARY: &str = "hermes";

impl HermesRunner {
    pub(crate) fn hermes_home(&self) -> Option<PathBuf> {
        if let Ok(raw) = std::env::var("HERMES_HOME") {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return Some(PathBuf::from(trimmed));
            }
        }
        dirs::home_dir().map(|h| h.join(".hermes"))
    }

    fn profile_home(&self, profile: Option<&str>) -> Option<PathBuf> {
        let home = self.hermes_home()?;
        match profile.map(str::trim).filter(|p| !p.is_empty()) {
            Some("default") | None => Some(home),
            Some(name) => Some(home.join("profiles").join(name)),
        }
    }
}

fn is_safe_session_id(s: &str) -> bool {
    // Current Hermes ids look like `20260717_183257_d25185`; cron and
    // older sources may use longer alphanumeric/underscore forms. Keep the
    // spawn boundary shell-safe without assuming a UUID-only format.
    (8..=128).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn is_safe_profile_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

impl CliRunner for HermesRunner {
    fn id(&self) -> &'static str {
        "hermes"
    }

    fn binary_name(&self) -> &'static str {
        BINARY
    }

    fn resolve_binary_path(&self) -> String {
        crate::shared::platform::path::find_binary(BINARY)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| BINARY.to_string())
    }

    fn build_spawn_command(&self, opts: &SpawnOpts) -> String {
        let mut cmd = opts
            .binary_path_override
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(crate::shared::cli::runner::shell_quote_path)
            .unwrap_or_else(|| BINARY.to_string());

        if let Some(profile) = opts
            .profile
            .as_deref()
            .map(str::trim)
            .filter(|p| is_safe_profile_name(p))
        {
            cmd.push_str(&format!(" --profile \"{}\"", profile));
        }

        // Never splice an arbitrary persisted value into a shell command.
        // Malformed/stale ids start fresh rather than reaching the shell.
        if let Some(sid) = opts
            .resume_session_id
            .as_deref()
            .filter(|s| is_safe_session_id(s))
        {
            cmd.push_str(&format!(" --resume \"{}\"", sid));
        }
        if opts.skip_permissions {
            cmd.push_str(" --yolo");
        }

        // Hermes reads AGENTS.md. AgentPanel calls agent_inject_purpose before
        // spawn, so consuming the shared option here is intentional.
        let _ = &opts.system_prompt;
        cmd
    }

    fn profiles(&self) -> Result<Vec<String>, String> {
        let home = self
            .hermes_home()
            .ok_or_else(|| "Cannot determine Hermes home directory".to_string())?;
        let mut profiles = vec!["default".to_string()];
        let profiles_dir = home.join("profiles");
        if profiles_dir.is_dir() {
            let entries = std::fs::read_dir(&profiles_dir)
                .map_err(|e| format!("Cannot read Hermes profiles: {e}"))?;
            for entry in entries.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                if let Some(name) = entry
                    .file_name()
                    .to_str()
                    .filter(|n| is_safe_profile_name(n))
                {
                    profiles.push(name.to_string());
                }
            }
        }
        profiles[1..].sort_unstable();
        Ok(profiles)
    }

    fn sessions_root_for_profile(&self, profile: Option<&str>) -> Option<PathBuf> {
        self.profile_home(profile)
    }

    fn home_dir(&self) -> Option<PathBuf> {
        self.hermes_home()
    }

    fn plugins_dir(&self) -> Option<PathBuf> {
        self.hermes_home().map(|p| p.join("plugins"))
    }

    fn settings_file(&self) -> Option<PathBuf> {
        self.hermes_home().map(|p| p.join("config.yaml"))
    }

    fn installed_plugins_file(&self) -> Option<PathBuf> {
        None
    }
    fn plugin_marketplaces_dir(&self) -> Option<PathBuf> {
        None
    }
    fn plugin_install_counts_file(&self) -> Option<PathBuf> {
        None
    }

    fn run_plugin_subcommand(&self, _args: &[&str]) -> Result<(bool, String), String> {
        Err(
            "Hermes plugins are managed by Hermes; ZeroAny Workbench's marketplace UI does not apply."
                .into(),
        )
    }

    fn sessions_root(&self) -> Option<PathBuf> {
        self.hermes_home()
    }
    fn session_dir_for_project(&self, _project_path: &str) -> Option<PathBuf> {
        None
    }
    fn session_file_extension(&self) -> &'static str {
        "db"
    }

    fn extract_resume_id_from_output(&self, buffer: &str) -> Option<String> {
        let marker = "hermes --resume ";
        let tail = buffer.split(marker).nth(1)?;
        let id: String = tail
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(*c, '_' | '-'))
            .collect();
        is_safe_session_id(&id).then_some(id)
    }

    fn usage_api_orgs_url(&self) -> Option<String> {
        None
    }
    fn usage_api_url_for(&self, _org_id: &str) -> Option<String> {
        None
    }

    fn is_session_file(&self, path: &Path) -> bool {
        path.file_name().and_then(|n| n.to_str()) == Some("state.db")
    }
}

pub static HERMES: HermesRunner = HermesRunner;

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(resume: Option<&str>, skip: bool) -> SpawnOpts {
        SpawnOpts {
            resume_session_id: resume.map(str::to_string),
            skip_permissions: skip,
            ..SpawnOpts::default()
        }
    }

    #[test]
    fn builds_fresh_and_resumed_commands() {
        assert_eq!(HERMES.build_spawn_command(&opts(None, false)), "hermes");
        assert_eq!(
            HERMES.build_spawn_command(&opts(Some("20260717_183257_d25185"), true)),
            "hermes --resume \"20260717_183257_d25185\" --yolo"
        );
    }

    #[test]
    fn binds_fresh_and_resumed_commands_to_profile() {
        let mut fresh = opts(None, false);
        fresh.profile = Some("cozy-engineer".into());
        assert_eq!(
            HERMES.build_spawn_command(&fresh),
            "hermes --profile \"cozy-engineer\""
        );

        let mut resumed = opts(Some("20260717_183257_d25185"), true);
        resumed.profile = Some("cozy-engineer".into());
        assert_eq!(
            HERMES.build_spawn_command(&resumed),
            "hermes --profile \"cozy-engineer\" --resume \"20260717_183257_d25185\" --yolo"
        );
    }

    #[test]
    fn rejects_malformed_profile_name() {
        let mut selected = opts(None, false);
        selected.profile = Some("bad; touch /tmp/nope".into());
        assert_eq!(HERMES.build_spawn_command(&selected), "hermes");
    }

    #[test]
    fn rejects_malformed_resume_id() {
        assert_eq!(
            HERMES.build_spawn_command(&opts(Some("bad; touch /tmp/nope"), false)),
            "hermes"
        );
    }

    #[test]
    fn extracts_resume_id_from_banner() {
        assert_eq!(
            HERMES
                .extract_resume_id_from_output(
                    "Resume later with: hermes --resume 20260717_183257_d25185\r\n"
                )
                .as_deref(),
            Some("20260717_183257_d25185")
        );
    }
}
