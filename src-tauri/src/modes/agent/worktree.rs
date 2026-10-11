use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use sha2::{Digest, Sha256};

fn git_output(project_path: &str, args: &[&str]) -> Result<std::process::Output, String> {
    use crate::shared::platform::path::{apply_user_path, find_binary};

    let git =
        find_binary("git").ok_or_else(|| "git is not installed or not on PATH".to_string())?;
    let mut command = std::process::Command::new(git);
    apply_user_path(&mut command);
    command
        .arg("-C")
        .arg(project_path)
        .args(args)
        .output()
        .map_err(|e| format!("git failed to start: {e}"))
}

fn output_path(output: std::process::Output) -> Option<PathBuf> {
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).ok())
        .flatten()
        .map(|value| PathBuf::from(value.trim()))
        .filter(|value| !value.as_os_str().is_empty())
}

fn main_project_from_common_dir(common: &Path) -> Option<PathBuf> {
    (common.file_name().and_then(|name| name.to_str()) == Some(".git"))
        .then(|| common.parent().map(Path::to_path_buf))
        .flatten()
}

/// Worktrees live outside the checkout, under the app data directory:
///
/// ```text
/// <app_data>/worktrees/<project-slug>-<hash8>/<project>-<branch>-<session8>
/// ```
///
/// Keeping them out of the project folder means the repository stays clean and
/// `.gitignore` is never written to. Sessions created by earlier builds used
/// `<project>/.zeroany-worktrees/<session>` (and, before the rebrand,
/// `<project>/.clauge-worktrees/<session>`); both names are still recognised so
/// old sessions keep resolving to their project.
///
/// Every container carries a `.zeroany-project` marker recording the owning
/// project path. Discovery rows outlive individual checkouts, and Git cannot
/// answer `rev-parse` for a cwd that no longer exists, so that marker is what
/// maps a historical worktree path back to its project without rewriting the
/// path used for resume.
const WORKTREES_DIR_NAME: &str = "worktrees";
const PROJECT_MARKER: &str = ".zeroany-project";
const LEGACY_WORKTREE_CONTAINERS: &[&str] = &[".zeroany-worktrees", ".clauge-worktrees"];

static WORKTREES_ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Called once from `lib.rs` setup with Tauri's `app_data_dir`.
pub fn init_worktrees_root(app_data_dir: &Path) {
    let _ = WORKTREES_ROOT.set(app_data_dir.join(WORKTREES_DIR_NAME));
}

/// Root that new worktrees are created under. Falls back to the platform data
/// directory when setup has not run yet (unit tests, early calls).
pub(crate) fn worktrees_root() -> Result<PathBuf, String> {
    if let Some(root) = WORKTREES_ROOT.get() {
        return Ok(root.clone());
    }
    dirs::data_dir()
        .map(|dir| dir.join("zeroany-workbench").join(WORKTREES_DIR_NAME))
        .ok_or_else(|| "Could not resolve a data directory for worktrees".to_string())
}

/// Project root recorded by the nearest `.zeroany-project` marker, if any.
pub(crate) fn project_root_from_marker(path: &Path) -> Option<PathBuf> {
    let container = path
        .ancestors()
        .find(|ancestor| ancestor.join(PROJECT_MARKER).is_file())?;
    let raw = std::fs::read_to_string(container.join(PROJECT_MARKER)).ok()?;
    let recorded = serde_json::from_str::<serde_json::Value>(&raw)
        .ok()
        .and_then(|value| {
            value
                .get("project_root")
                .and_then(|entry| entry.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| raw.trim().to_string());
    let recorded = recorded.trim();
    (!recorded.is_empty()).then(|| PathBuf::from(recorded))
}

/// Project that owns a managed worktree path: the marker for current layouts,
/// the container folder name for legacy in-project layouts.
fn managed_worktree_project_root(path: &Path) -> Option<PathBuf> {
    project_root_from_marker(path).or_else(|| {
        path.ancestors()
            .find(|ancestor| {
                ancestor
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| LEGACY_WORKTREE_CONTAINERS.contains(&name))
                    .unwrap_or(false)
            })
            .and_then(Path::parent)
            .map(Path::to_path_buf)
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectIdentity {
    pub project_root: String,
    pub project_name: String,
    pub is_linked_worktree: bool,
}

pub(crate) fn project_name_from_path(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Unknown")
        .to_string()
}

/// Keep the selected cwd separate from the stable repository identity.
/// A linked worktree runs in its own checkout, but is grouped under the
/// primary checkout and must not receive another nested worktree.
pub(crate) fn resolve_project_identity(path: &str) -> ProjectIdentity {
    let path = path.trim();
    if path.is_empty() {
        return ProjectIdentity {
            project_root: String::new(),
            project_name: "Unknown".to_string(),
            is_linked_worktree: false,
        };
    }

    let common_root = git_output(
        path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()
    .and_then(output_path)
    .and_then(|common| main_project_from_common_dir(&common));
    let checkout_root = git_output(path, &["rev-parse", "--show-toplevel"])
        .ok()
        .and_then(output_path);
    let managed_root = managed_worktree_project_root(Path::new(path));
    let is_linked_worktree = match (&common_root, &checkout_root) {
        (Some(common), Some(checkout)) => common != checkout,
        (None, None) => managed_root.is_some(),
        _ => false,
    };
    let project_root = common_root
        .or(checkout_root)
        .or(managed_root)
        .unwrap_or_else(|| PathBuf::from(path))
        .to_string_lossy()
        .to_string();
    let project_name = project_name_from_path(&project_root);

    ProjectIdentity {
        project_root,
        project_name,
        is_linked_worktree,
    }
}

/// Resolve a provider session cwd to the stable main-project identity used
/// for grouping. The cwd itself remains untouched in the discovered-session
/// row so native resume still opens in the original linked worktree.
pub(crate) fn resolve_project_root(path: &str) -> String {
    resolve_project_identity(path).project_root
}

fn validate_new_branch(project_path: &str, branch_name: &str) -> Result<(), String> {
    let branch_name = branch_name.trim();
    if branch_name.is_empty() {
        return Err("Branch name is required".to_string());
    }

    let format = git_output(project_path, &["check-ref-format", "--branch", branch_name])?;
    if !format.status.success() {
        return Err(format!("Invalid branch name: {branch_name}"));
    }

    let ref_name = format!("refs/heads/{branch_name}");
    let existing = git_output(
        project_path,
        &["show-ref", "--verify", "--quiet", &ref_name],
    )?;
    if existing.status.success() {
        return Err(format!("Branch \"{branch_name}\" already exists"));
    }
    Ok(())
}

fn portable_path_component(value: &str, fallback: &str) -> String {
    let mut component = String::new();
    let mut separator_pending = false;

    for character in value.trim().chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
            if separator_pending && !component.is_empty() && !component.ends_with('-') {
                component.push('-');
            }
            component.push(character);
            separator_pending = false;
        } else {
            separator_pending = true;
        }
    }

    let component = component.trim_matches(['-', '.']);
    if component.is_empty() {
        fallback.to_string()
    } else {
        component.to_string()
    }
}

/// `<slug>-<first 8 hex of sha256(project root)>` — readable and stable, so a
/// project keeps the same container across restarts while two checkouts that
/// share a folder name never collide.
fn project_container_name(project_root: &str) -> String {
    let slug = portable_path_component(&project_name_from_path(project_root), "project");
    let digest = Sha256::digest(project_root.as_bytes());
    format!("{slug}-{}", hex::encode(&digest[..4]))
}

fn project_marker_body(project_root: &str) -> String {
    serde_json::json!({
        "project_root": project_root,
        "managed_by": "zeroany-workbench",
    })
    .to_string()
}

fn write_project_marker(container: &Path, project_root: &str) -> Result<(), String> {
    std::fs::write(
        container.join(PROJECT_MARKER),
        project_marker_body(project_root),
    )
    .map_err(|e| format!("Failed to write worktree marker: {e}"))
}

#[tauri::command]
pub fn agent_is_git_repo(path: String) -> Result<bool, String> {
    let output = git_output(&path, &["rev-parse", "--is-inside-work-tree"])?;
    Ok(output.status.success())
}

/// Resolve arbitrary session working directories to their containing Git
/// repository roots. Non-repository paths map to themselves so the frontend
/// can still surface them under an Unscoped/standalone project group.
#[tauri::command]
pub async fn agent_resolve_project_roots(
    paths: Vec<String>,
) -> Result<HashMap<String, String>, String> {
    tokio::task::spawn_blocking(move || {
        let mut roots = HashMap::new();
        let mut seen = HashSet::new();

        for path in paths {
            let path = path.trim().to_string();
            if path.is_empty() || !seen.insert(path.clone()) {
                continue;
            }

            let root = resolve_project_root(&path);
            roots.insert(path, root);
        }

        roots
    })
    .await
    .map_err(|error| format!("Project root resolution failed: {error}"))
}

#[tauri::command]
pub fn agent_validate_worktree_branch(
    project_path: String,
    branch_name: String,
) -> Result<(), String> {
    validate_new_branch(&project_path, &branch_name)
}

#[tauri::command]
pub fn agent_create_worktree(
    project_path: String,
    session_id: String,
    base_branch: String,
    branch_name: String,
) -> Result<String, String> {
    let root = worktrees_root()?;
    create_worktree_in(
        &root,
        &project_path,
        &session_id,
        &base_branch,
        &branch_name,
    )
}

/// Create a worktree for the repository that owns `project_path`.
///
/// The repository root is resolved first and every Git call runs against that
/// root, so a session cwd'd in a subdirectory — or inside another linked
/// worktree — can no longer create a checkout in an unintended parent
/// repository. Nothing is ever written inside the project folder.
fn create_worktree_in(
    root: &Path,
    project_path: &str,
    session_id: &str,
    base_branch: &str,
    branch_name: &str,
) -> Result<String, String> {
    let session_uuid =
        uuid::Uuid::parse_str(session_id.trim()).map_err(|e| format!("Invalid session ID: {e}"))?;

    let branch_name = branch_name.trim();
    let base_branch = base_branch.trim();
    if base_branch.is_empty() {
        return Err("Base branch is required".to_string());
    }

    let identity = resolve_project_identity(project_path);
    let repo_root = if identity.project_root.trim().is_empty() {
        project_path.trim().to_string()
    } else {
        identity.project_root
    };
    validate_new_branch(&repo_root, branch_name)?;

    let base_commit = format!("{base_branch}^{{commit}}");
    let base = git_output(&repo_root, &["rev-parse", "--verify", &base_commit])?;
    if !base.status.success() {
        return Err(format!("Base branch \"{base_branch}\" does not exist"));
    }

    let session_short = session_uuid
        .simple()
        .to_string()
        .chars()
        .take(8)
        .collect::<String>();
    let project_component = portable_path_component(&project_name_from_path(&repo_root), "project");
    let branch_component = portable_path_component(branch_name, "branch");
    let container = root.join(project_container_name(&repo_root));
    let worktree_dir = container.join(format!(
        "{project_component}-{branch_component}-{session_short}"
    ));
    if worktree_dir.exists() {
        return Err(format!(
            "Worktree directory already exists: {}",
            worktree_dir.to_string_lossy()
        ));
    }

    std::fs::create_dir_all(&container).map_err(|e| {
        format!(
            "Failed to create worktree directory {}: {e}",
            container.to_string_lossy()
        )
    })?;
    write_project_marker(&container, &repo_root)?;

    let _ = git_output(&repo_root, &["worktree", "prune"]);
    let worktree_path = worktree_dir.to_string_lossy().to_string();
    let output = git_output(
        &repo_root,
        &[
            "worktree",
            "add",
            "-b",
            branch_name,
            &worktree_path,
            base_branch,
        ],
    )?;
    if !output.status.success() {
        return Err(format!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(worktree_path)
}

#[tauri::command]
pub fn agent_remove_worktree(
    project_path: String,
    worktree_path: String,
    force: bool,
) -> Result<(), String> {
    use crate::shared::platform::path::{apply_user_path, find_binary};
    let git_bin =
        find_binary("git").ok_or_else(|| "git is not installed or not on PATH".to_string())?;

    let mut remove = std::process::Command::new(&git_bin);
    apply_user_path(&mut remove);
    remove.args(["-C", &project_path, "worktree", "remove"]);
    if force {
        remove.arg("--force");
    }
    let out = remove
        .arg(&worktree_path)
        .output()
        .map_err(|e| format!("git worktree remove failed to spawn: {e}"))?;

    let mut prune = std::process::Command::new(&git_bin);
    apply_user_path(&mut prune);
    let _ = prune
        .args(["-C", &project_path, "worktree", "prune"])
        .output();

    // The container and its `.zeroany-project` marker deliberately stay behind:
    // they are what map this (now historical) worktree path back to its project
    // during session discovery, exactly like the legacy in-project container did.
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        // Treat "not a working tree" / "no such directory" as success — the
        // worktree is already gone (deleted outside ZeroAny Workbench); prune above
        // cleared the stale git metadata. Caller's intent is satisfied.
        let lower = stderr.to_lowercase();
        if lower.contains("is not a working tree")
            || lower.contains("no such file or directory")
            || lower.contains("not a valid working tree")
        {
            return Ok(());
        }
        return Err(if stderr.is_empty() {
            "git worktree remove failed with no output".to_string()
        } else {
            stderr
        });
    }
    Ok(())
}

/// True when the worktree at `worktree_path` has uncommitted changes
/// (modified, staged, or untracked). Used as a preflight before the
/// destructive `git worktree remove --force` in session-delete so we
/// can warn the user that committing-or-stashing now would save work
/// that's about to be discarded.
#[tauri::command]
pub fn agent_worktree_is_dirty(worktree_path: String) -> Result<bool, String> {
    use crate::shared::platform::path::{apply_user_path, find_binary};
    let git_bin =
        find_binary("git").ok_or_else(|| "git is not installed or not on PATH".to_string())?;
    let mut cmd = std::process::Command::new(&git_bin);
    apply_user_path(&mut cmd);
    let out = cmd
        .args(["-C", &worktree_path, "status", "--porcelain"])
        .output()
        .map_err(|e| format!("git status failed to spawn: {e}"))?;
    if !out.status.success() {
        // Worktree path doesn't exist / isn't a git checkout. Treat as
        // "not dirty" so the delete flow doesn't block on a missing
        // worktree — the user wants it gone either way.
        return Ok(false);
    }
    Ok(!String::from_utf8_lossy(&out.stdout).trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zeroany-workbench-worktree-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn temp_repo(name: &str) -> PathBuf {
        let dir = temp_dir(name);
        assert!(Command::new("git")
            .args(["init", "-b", "dev"])
            .arg(&dir)
            .status()
            .unwrap()
            .success());
        std::fs::write(dir.join("README.md"), "test\n").unwrap();
        assert!(Command::new("git")
            .args(["-C"])
            .arg(&dir)
            .args(["add", "."])
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .args(["-C"])
            .arg(&dir)
            .args([
                "-c",
                "user.name=ZeroAny Workbench Test",
                "-c",
                "user.email=test@zeroany-workbench.local",
                "commit",
                "-m",
                "initial",
            ])
            .status()
            .unwrap()
            .success());
        dir
    }

    fn git_lines(path: &str, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(["-C", path])
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn portable_path_components_only_expose_safe_ascii_characters() {
        assert_eq!(
            portable_path_component("lute_station/发布@rel 2026.07#", "project"),
            "lute_station-rel-2026.07"
        );
        assert_eq!(portable_path_component("中文/🚀", "branch"), "branch");
    }

    #[test]
    fn containers_are_stable_and_do_not_collide_across_projects() {
        let a = project_container_name("/home/dev/api");
        let b = project_container_name("/srv/other/api");
        assert!(a.starts_with("api-"));
        assert!(b.starts_with("api-"));
        assert_ne!(a, b, "same folder name must not share a container");
        assert_eq!(a, project_container_name("/home/dev/api"));
    }

    #[tokio::test]
    async fn project_root_resolution_merges_subdirectories_and_linked_worktrees() {
        let repo = temp_repo("resolve-roots");
        let root = temp_dir("resolve-roots-store");
        let nested = repo.join("src").join("feature");
        std::fs::create_dir_all(&nested).unwrap();
        let worktree = create_worktree_in(
            &root,
            &repo.to_string_lossy(),
            "44444444-0000-0000-0000-000000000000",
            "dev",
            "feature/root-resolution",
        )
        .unwrap();

        let nested_string = nested.to_string_lossy().to_string();
        let roots = agent_resolve_project_roots(vec![nested_string.clone(), worktree.clone()])
            .await
            .unwrap();
        let expected = repo.to_string_lossy().to_string();
        assert_eq!(roots[&nested_string], expected);
        assert_eq!(roots[&worktree], expected);

        let main_identity = resolve_project_identity(&nested_string);
        assert_eq!(main_identity.project_root, expected);
        assert_eq!(
            main_identity.project_name,
            repo.file_name().unwrap().to_string_lossy()
        );
        assert!(!main_identity.is_linked_worktree);

        let worktree_identity = resolve_project_identity(&worktree);
        assert_eq!(worktree_identity.project_root, expected);
        assert_eq!(
            worktree_identity.project_name,
            repo.file_name().unwrap().to_string_lossy()
        );
        assert!(worktree_identity.is_linked_worktree);

        // Discovery catalogs outlive individual worktrees. Once the linked
        // checkout is removed, its historical cwd must still group under the
        // main project instead of becoming a phantom standalone project —
        // this is what the `.zeroany-project` marker is for.
        agent_remove_worktree(repo.to_string_lossy().to_string(), worktree.clone(), true).unwrap();
        assert!(!Path::new(&worktree).exists());
        let removed_roots = agent_resolve_project_roots(vec![worktree.clone()])
            .await
            .unwrap();
        assert_eq!(removed_roots[&worktree], expected);

        let _ = std::fs::remove_dir_all(repo);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn worktrees_are_created_outside_the_project_and_leave_gitignore_alone() {
        let repo = temp_repo("outside-project");
        let root = temp_dir("outside-project-store");
        std::fs::write(repo.join(".gitignore"), "target/\n").unwrap();

        let result = create_worktree_in(
            &root,
            &repo.to_string_lossy(),
            "123e4567-e89b-12d3-a456-426614174000",
            "dev",
            "feature/add-user-login",
        )
        .unwrap();

        let project_name = repo.file_name().unwrap().to_string_lossy().to_string();
        let container = root.join(project_container_name(&repo.to_string_lossy()));
        assert_eq!(
            PathBuf::from(&result),
            container.join(format!("{project_name}-feature-add-user-login-123e4567"))
        );
        // Outside the checkout, and the repository is untouched.
        assert!(!Path::new(&result).starts_with(&repo));
        assert_eq!(
            std::fs::read_to_string(repo.join(".gitignore")).unwrap(),
            "target/\n"
        );
        assert!(!repo.join(".zeroany-worktrees").exists());
        assert!(!repo.join(".gitignore").read_link().is_ok());
        // Branch checked out in the new worktree.
        assert_eq!(
            git_lines(&result, &["branch", "--show-current"]),
            "feature/add-user-login"
        );
        // Marker records the owning project so a removed checkout still resolves.
        let marker = std::fs::read_to_string(container.join(PROJECT_MARKER)).unwrap();
        assert!(
            marker.contains(&repo.to_string_lossy().to_string()),
            "{marker}"
        );

        let _ = std::fs::remove_dir_all(repo);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_project_without_gitignore_never_gains_one() {
        let repo = temp_repo("no-gitignore");
        let root = temp_dir("no-gitignore-store");

        create_worktree_in(
            &root,
            &repo.to_string_lossy(),
            "33333333-0000-0000-0000-000000000000",
            "dev",
            "feature/no-gitignore",
        )
        .unwrap();

        assert!(!repo.join(".gitignore").exists());
        let _ = std::fs::remove_dir_all(repo);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn subdirectory_selection_still_targets_the_owning_repository() {
        let repo = temp_repo("nested-selection");
        let root = temp_dir("nested-selection-store");
        let nested = repo.join("packages").join("web");
        std::fs::create_dir_all(&nested).unwrap();

        let result = create_worktree_in(
            &root,
            &nested.to_string_lossy(),
            "55555555-0000-0000-0000-000000000000",
            "dev",
            "feature/from-nested",
        )
        .unwrap();

        // The checkout belongs to the repository that owns the selected folder,
        // and nothing was created inside the project.
        assert_eq!(
            resolve_project_root(&result),
            repo.to_string_lossy().to_string()
        );
        assert!(PathBuf::from(&result).starts_with(&root));
        assert!(!nested.join(".zeroany-worktrees").exists());
        assert!(!nested.join(".gitignore").exists());

        let _ = std::fs::remove_dir_all(repo);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_linked_worktree_does_not_grow_nested_worktrees() {
        let repo = temp_repo("nested-worktree");
        let root = temp_dir("nested-worktree-store");
        let first = create_worktree_in(
            &root,
            &repo.to_string_lossy(),
            "66666666-0000-0000-0000-000000000000",
            "dev",
            "feature/first",
        )
        .unwrap();

        // Selecting the linked checkout resolves to the main repository, so the
        // second worktree is a sibling rather than nesting inside the first.
        let second = create_worktree_in(
            &root,
            &first,
            "77777777-0000-0000-0000-000000000000",
            "dev",
            "feature/second",
        )
        .unwrap();

        assert_eq!(
            resolve_project_root(&second),
            repo.to_string_lossy().to_string()
        );
        assert_eq!(
            PathBuf::from(&first).parent(),
            PathBuf::from(&second).parent()
        );
        assert!(!Path::new(&first).join(".zeroany-worktrees").exists());

        let _ = agent_remove_worktree(repo.to_string_lossy().to_string(), first, true);
        let _ = agent_remove_worktree(repo.to_string_lossy().to_string(), second, true);
        let _ = std::fs::remove_dir_all(repo);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_invalid_session_id_before_creating_branch_or_worktree() {
        let repo = temp_repo("invalid-session-id");
        let root = temp_dir("invalid-session-id-store");

        let error = create_worktree_in(
            &root,
            &repo.to_string_lossy(),
            "not-a-uuid",
            "dev",
            "feature/invalid-session",
        )
        .unwrap_err();

        assert!(error.contains("Invalid session ID"), "{error}");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        assert!(!repo.join(".zeroany-worktrees").exists());
        let branch = Command::new("git")
            .args(["-C"])
            .arg(&repo)
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                "refs/heads/feature/invalid-session",
            ])
            .status()
            .unwrap();
        assert!(!branch.success());
        let _ = std::fs::remove_dir_all(repo);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_existing_branch_instead_of_reusing_it() {
        let repo = temp_repo("duplicate");
        let root = temp_dir("duplicate-store");
        assert!(Command::new("git")
            .args(["-C"])
            .arg(&repo)
            .args(["branch", "feature/existing"])
            .status()
            .unwrap()
            .success());

        let error = create_worktree_in(
            &root,
            &repo.to_string_lossy(),
            "abcdef12-0000-0000-0000-000000000000",
            "dev",
            "feature/existing",
        )
        .unwrap_err();

        assert!(error.contains("already exists"), "{error}");
        let _ = std::fs::remove_dir_all(repo);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn normal_remove_refuses_a_dirty_worktree() {
        let repo = temp_repo("remove-dirty");
        let root = temp_dir("remove-dirty-store");
        let worktree = create_worktree_in(
            &root,
            &repo.to_string_lossy(),
            "11111111-0000-0000-0000-000000000000",
            "dev",
            "dev-11111111",
        )
        .unwrap();
        std::fs::write(
            PathBuf::from(&worktree).join("uncommitted.txt"),
            "keep me\n",
        )
        .unwrap();

        let error =
            agent_remove_worktree(repo.to_string_lossy().to_string(), worktree.clone(), false)
                .unwrap_err();

        assert!(
            error.to_lowercase().contains("modified") || error.to_lowercase().contains("untracked"),
            "{error}"
        );
        assert!(PathBuf::from(&worktree).exists());
        let _ = agent_remove_worktree(repo.to_string_lossy().to_string(), worktree, true);
        let _ = std::fs::remove_dir_all(repo);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn force_remove_discards_a_dirty_worktree() {
        let repo = temp_repo("force-remove-dirty");
        let root = temp_dir("force-remove-dirty-store");
        let worktree = create_worktree_in(
            &root,
            &repo.to_string_lossy(),
            "22222222-0000-0000-0000-000000000000",
            "dev",
            "dev-22222222",
        )
        .unwrap();
        std::fs::write(
            PathBuf::from(&worktree).join("uncommitted.txt"),
            "discard me\n",
        )
        .unwrap();

        agent_remove_worktree(repo.to_string_lossy().to_string(), worktree.clone(), true).unwrap();

        assert!(!PathBuf::from(&worktree).exists());
        let _ = std::fs::remove_dir_all(repo);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn command_entry_point_uses_the_initialised_worktree_root() {
        // Mirrors app startup: the root comes from `app_data_dir`, and the
        // `#[tauri::command]` wrapper is what sessions actually call.
        let repo = temp_repo("command-root");
        let root = temp_dir("command-root-store");
        if WORKTREES_ROOT.get().is_none() {
            init_worktrees_root(&root);
        }
        let root = worktrees_root().unwrap();

        let result = agent_create_worktree(
            repo.to_string_lossy().to_string(),
            "88888888-0000-0000-0000-000000000000".into(),
            "dev".into(),
            "feature/command-root".into(),
        )
        .unwrap();

        assert!(PathBuf::from(&result).starts_with(&root), "{result}");
        assert!(!PathBuf::from(&result).starts_with(&repo));
        assert!(PathBuf::from(&result).exists());

        let _ = agent_remove_worktree(repo.to_string_lossy().to_string(), result, true);
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn legacy_in_project_worktree_paths_still_resolve_to_their_project() {
        let repo = temp_repo("legacy-path");
        let legacy = repo
            .join(".zeroany-worktrees")
            .join("legacy-task")
            .to_string_lossy()
            .to_string();
        std::fs::create_dir_all(&legacy).unwrap();

        assert_eq!(
            resolve_project_root(&legacy),
            repo.to_string_lossy().to_string()
        );

        let _ = std::fs::remove_dir_all(repo);
    }
}
