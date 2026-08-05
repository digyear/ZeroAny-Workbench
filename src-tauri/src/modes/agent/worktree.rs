use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

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

/// ZeroAny Workbench worktrees live below `<project>/.zeroany-worktrees/<session>`.
/// Historical provider sessions remain in their native stores after that
/// checkout is removed, so Git can no longer answer `rev-parse` for the old
/// cwd. The stable container name lets us recover the owning project without
/// rewriting the original cwd used for resume.
fn managed_worktree_project_root(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|ancestor| {
            ancestor.file_name().and_then(|name| name.to_str()) == Some(".zeroany-worktrees")
        })
        .and_then(Path::parent)
        .map(Path::to_path_buf)
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

fn ensure_worktree_ignored(project_path: &str) -> Result<(), String> {
    let gitignore = PathBuf::from(project_path).join(".gitignore");
    match std::fs::read_to_string(&gitignore) {
        Ok(contents) => {
            if contents
                .lines()
                .any(|line| line.trim() == ".zeroany-worktrees/")
            {
                return Ok(());
            }
            let updated = if contents.trim_end().is_empty() {
                ".zeroany-worktrees/\n".to_string()
            } else {
                format!("{}\n.zeroany-worktrees/\n", contents.trim_end())
            };
            std::fs::write(&gitignore, updated)
                .map_err(|e| format!("Failed to update .gitignore: {e}"))?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::write(&gitignore, ".zeroany-worktrees/\n")
                .map_err(|e| format!("Failed to create .gitignore: {e}"))?;
        }
        Err(e) => return Err(format!("Failed to read .gitignore: {e}")),
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
    validate_new_branch(&project_path, &branch_name)?;

    let session_uuid =
        uuid::Uuid::parse_str(session_id.trim()).map_err(|e| format!("Invalid session ID: {e}"))?;

    let base_branch = base_branch.trim();
    if base_branch.is_empty() {
        return Err("Base branch is required".to_string());
    }
    let base_commit = format!("{base_branch}^{{commit}}");
    let base = git_output(&project_path, &["rev-parse", "--verify", &base_commit])?;
    if !base.status.success() {
        return Err(format!("Base branch \"{base_branch}\" does not exist"));
    }

    let session_short = session_uuid
        .simple()
        .to_string()
        .chars()
        .take(8)
        .collect::<String>();
    let project_name = PathBuf::from(&project_path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(|name| portable_path_component(name, "project"))
        .unwrap_or_else(|| "project".to_string());
    let branch_component = portable_path_component(&branch_name, "branch");
    let session_dir = format!("{project_name}-{branch_component}-{session_short}");
    let worktree_dir = PathBuf::from(&project_path)
        .join(".zeroany-worktrees")
        .join(session_dir);
    if worktree_dir.exists() {
        return Err(format!(
            "Worktree directory already exists: {}",
            worktree_dir.to_string_lossy()
        ));
    }
    ensure_worktree_ignored(&project_path)?;
    let parent = worktree_dir
        .parent()
        .ok_or_else(|| "Invalid worktree directory".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("Failed to create worktree directory: {e}"))?;

    let _ = git_output(&project_path, &["worktree", "prune"]);
    let worktree_path = worktree_dir.to_string_lossy().to_string();
    let output = git_output(
        &project_path,
        &[
            "worktree",
            "add",
            "-b",
            branch_name.trim(),
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

    fn temp_repo(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("zeroany-workbench-worktree-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
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

    #[test]
    fn portable_path_components_only_expose_safe_ascii_characters() {
        assert_eq!(
            portable_path_component("lute_station/发布@rel 2026.07#", "project"),
            "lute_station-rel-2026.07"
        );
        assert_eq!(portable_path_component("中文/🚀", "branch"), "branch");
    }

    #[tokio::test]
    async fn project_root_resolution_merges_subdirectories_and_linked_worktrees() {
        let repo = temp_repo("resolve-roots");
        let nested = repo.join("src").join("feature");
        std::fs::create_dir_all(&nested).unwrap();
        let worktree = agent_create_worktree(
            repo.to_string_lossy().to_string(),
            "44444444-0000-0000-0000-000000000000".into(),
            "dev".into(),
            "feature/root-resolution".into(),
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
        // main project instead of becoming a phantom standalone project.
        agent_remove_worktree(repo.to_string_lossy().to_string(), worktree.clone(), true).unwrap();
        let removed_roots = agent_resolve_project_roots(vec![worktree.clone()])
            .await
            .unwrap();
        assert_eq!(removed_roots[&worktree], expected);

        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn creates_readable_branch_from_selected_base_in_named_session_directory() {
        let repo = temp_repo("create");
        let project_name = repo.file_name().unwrap().to_string_lossy();
        let result = agent_create_worktree(
            repo.to_string_lossy().to_string(),
            "123e4567-e89b-12d3-a456-426614174000".into(),
            "dev".into(),
            "feature/add-user-login".into(),
        )
        .unwrap();

        assert_eq!(
            PathBuf::from(&result),
            repo.join(".zeroany-worktrees")
                .join(format!("{project_name}-feature-add-user-login-123e4567"))
        );
        let branch = Command::new("git")
            .args(["-C", &result, "branch", "--show-current"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&branch.stdout).trim(),
            "feature/add-user-login"
        );
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn rejects_invalid_session_id_before_creating_branch_or_worktree() {
        let repo = temp_repo("invalid-session-id");

        let error = agent_create_worktree(
            repo.to_string_lossy().to_string(),
            "not-a-uuid".into(),
            "dev".into(),
            "feature/invalid-session".into(),
        )
        .unwrap_err();

        assert!(error.contains("Invalid session ID"), "{error}");
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
    }

    #[test]
    fn gitignore_failure_does_not_create_branch_or_worktree() {
        let repo = temp_repo("gitignore-failure");
        std::fs::create_dir(repo.join(".gitignore")).unwrap();

        let error = agent_create_worktree(
            repo.to_string_lossy().to_string(),
            "33333333-0000-0000-0000-000000000000".into(),
            "dev".into(),
            "feature/gitignore-failure".into(),
        )
        .unwrap_err();

        assert!(error.contains(".gitignore"), "{error}");
        assert!(!repo.join(".zeroany-worktrees").exists());
        let branch = Command::new("git")
            .args(["-C"])
            .arg(&repo)
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                "refs/heads/feature/gitignore-failure",
            ])
            .status()
            .unwrap();
        assert!(!branch.success());
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn rejects_existing_branch_instead_of_reusing_it() {
        let repo = temp_repo("duplicate");
        assert!(Command::new("git")
            .args(["-C"])
            .arg(&repo)
            .args(["branch", "feature/existing"])
            .status()
            .unwrap()
            .success());

        let error = agent_create_worktree(
            repo.to_string_lossy().to_string(),
            "abcdef12-0000-0000-0000-000000000000".into(),
            "dev".into(),
            "feature/existing".into(),
        )
        .unwrap_err();

        assert!(error.contains("already exists"), "{error}");
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn normal_remove_refuses_a_dirty_worktree() {
        let repo = temp_repo("remove-dirty");
        let worktree = agent_create_worktree(
            repo.to_string_lossy().to_string(),
            "11111111-0000-0000-0000-000000000000".into(),
            "dev".into(),
            "dev-11111111".into(),
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
    }

    #[test]
    fn force_remove_discards_a_dirty_worktree() {
        let repo = temp_repo("force-remove-dirty");
        let worktree = agent_create_worktree(
            repo.to_string_lossy().to_string(),
            "22222222-0000-0000-0000-000000000000".into(),
            "dev".into(),
            "dev-22222222".into(),
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
    }
}
