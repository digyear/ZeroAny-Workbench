// Hermes Agent CLI implementation of [`CliRunner`].
//
// Hermes starts interactively with the bare `hermes` command. Sessions are
// resumed with the global `--resume <session-id>` option. Project instructions live in `AGENTS.md`; Agent
// mode writes those before spawn rather than trying to pass a system prompt.
// Hermes stores session metadata in a profile-specific `state.db`, so
// discovery is implemented in `modes/agent/usage.rs` and filtered by both the
// selected profile and session cwd.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sqlx::sqlite::SqliteConnectOptions;

use super::runner::{CliRunner, SpawnOpts};

pub struct HermesRunner;

const BINARY: &str = "hermes";
static PROJECT_SETUP_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

fn normalized_project_path(path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("Hermes project path is empty".into());
    }
    let path = PathBuf::from(trimmed);
    if path.is_absolute() {
        Ok(path)
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|error| format!("Cannot resolve Hermes project path: {error}"))
    }
}

fn path_is_within(path: &Path, folder: &Path) -> bool {
    #[cfg(windows)]
    {
        let path = path.to_string_lossy().to_lowercase();
        let folder = folder.to_string_lossy().to_lowercase();
        return Path::new(&path).starts_with(Path::new(&folder));
    }
    #[cfg(not(windows))]
    path.starts_with(folder)
}

fn run_hermes(binary: &str, profile: &str, args: &[&str]) -> Result<Output, String> {
    let mut command = Command::new(binary);
    command.args(["--profile", profile]);
    let output = command
        .args(args)
        .output()
        .map_err(|error| format!("Failed to run Hermes: {error}"))?;
    if output.status.success() {
        return Ok(output);
    }
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if detail.is_empty() {
        format!("Hermes command failed with {}", output.status)
    } else {
        format!("Hermes command failed: {detail}")
    })
}

fn smart_approvals_enabled(output: &Output) -> bool {
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .trim_matches('"')
        == "smart"
}

/// Register the Workbench workspace as a first-class, profile-scoped Hermes
/// Project and force Hermes' reviewed smart-approval mode before launch.
pub(crate) async fn prepare_native_project(
    binary_override: Option<&str>,
    profile: Option<&str>,
    project_path: &str,
    project_name: &str,
) -> Result<(), String> {
    let profile = profile
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("default")
        .to_string();
    if !is_safe_profile_name(&profile) {
        return Err(format!("Invalid Hermes profile name: {profile}"));
    }
    let path = normalized_project_path(project_path)?;
    if !path.is_dir() {
        return Err(format!(
            "Hermes project folder does not exist: {}",
            path.display()
        ));
    }
    let name = if project_name.trim().is_empty() {
        path.file_name()
            .and_then(|part| part.to_str())
            .unwrap_or("Project")
            .to_string()
    } else {
        project_name.trim().to_string()
    };
    let binary = binary_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| HERMES.resolve_binary_path());
    let db_path = HERMES
        .profile_home(Some(&profile))
        .ok_or_else(|| "Cannot determine Hermes profile directory".to_string())?
        .join("projects.db");
    let path_arg = path.to_string_lossy().to_string();

    let _guard = PROJECT_SETUP_LOCK.lock().await;
    let setup_binary = binary.clone();
    let setup_profile = profile.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let approvals = run_hermes(
            &setup_binary,
            &setup_profile,
            &["config", "get", "approvals.mode", "--json"],
        )?;
        if !smart_approvals_enabled(&approvals) {
            run_hermes(
                &setup_binary,
                &setup_profile,
                &["config", "set", "approvals.mode", "smart"],
            )?;
        }
        run_hermes(&setup_binary, &setup_profile, &["project", "list"])?;
        Ok(())
    })
    .await
    .map_err(|error| format!("Hermes setup failed: {error}"))??;

    let options = SqliteConnectOptions::new()
        .filename(&db_path)
        .read_only(true);
    let pool = sqlx::SqlitePool::connect_with(options)
        .await
        .map_err(|error| format!("Cannot open Hermes Projects: {error}"))?;
    let folders = sqlx::query_scalar::<_, String>(
        "SELECT pf.path FROM project_folders pf \
         JOIN projects p ON p.id = pf.project_id WHERE p.archived = 0",
    )
    .fetch_all(&pool)
    .await
    .map_err(|error| format!("Cannot read Hermes Projects: {error}"))?;
    pool.close().await;
    if folders
        .iter()
        .any(|folder| path_is_within(Path::new(&path_arg), Path::new(folder)))
    {
        return Ok(());
    }

    tokio::task::spawn_blocking(move || {
        run_hermes(
            &binary,
            &profile,
            &[
                "project",
                "create",
                &name,
                &path_arg,
                "--primary",
                &path_arg,
            ],
        )
        .map(|_| ())
    })
    .await
    .map_err(|error| format!("Hermes Project setup failed: {error}"))?
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
        // Workbench always uses Hermes smart approvals. Never translate the
        // shared skip-permissions option into unrestricted --yolo mode.
        let _ = opts.skip_permissions;

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
            "hermes --resume \"20260717_183257_d25185\""
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
            "hermes --profile \"cozy-engineer\" --resume \"20260717_183257_d25185\""
        );
    }

    #[test]
    fn skip_permissions_never_enables_yolo_for_hermes() {
        let command = HERMES.build_spawn_command(&opts(None, true));
        assert_eq!(command, "hermes");
        assert!(!command.contains("--yolo"));
    }

    #[test]
    fn detects_existing_project_folder_without_prefix_collisions() {
        assert!(path_is_within(
            Path::new("/work/apps/api/src"),
            Path::new("/work/apps/api")
        ));
        assert!(path_is_within(
            Path::new("/work/apps/api"),
            Path::new("/work/apps/api")
        ));
        assert!(!path_is_within(
            Path::new("/work/apps/api-v2"),
            Path::new("/work/apps/api")
        ));
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
