//! Per-space startup command: once per open generation, background, toast on failure.

use std::{
    collections::HashSet,
    time::Duration,
};

const TIMEOUT: Duration = Duration::from_secs(120);
const OUTPUT_CAP: usize = 4 * 1024;

/// Session-only: space ids whose non-empty startup command already ran this open generation.
#[derive(Clone, Debug, Default)]
pub struct Gate {
    ran: HashSet<String>,
}

impl Gate {
    /// Claim a run for `space_id`. Returns true only the first time this open generation.
    pub fn claim(&mut self, space_id: &str) -> bool {
        self.ran.insert(space_id.to_owned())
    }

    pub fn contains(&self, space_id: &str) -> bool {
        self.ran.contains(space_id)
    }

    /// Forget a closed space so the next open can run again.
    pub fn on_close(&mut self, space_id: &str) {
        self.ran.remove(space_id);
    }

    /// Drop ids no longer in the model (machine unload, restore removals).
    pub fn retain_present(&mut self, present: impl Fn(&str) -> bool) {
        self.ran.retain(|id| present(id));
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunResult {
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
}

impl RunResult {
    pub fn failed(&self) -> bool {
        self.timed_out || self.error.is_some() || self.exit_code != Some(0)
    }

    pub fn short_reason(&self) -> String {
        if self.timed_out {
            return "Timed out".into();
        }
        if let Some(error) = &self.error {
            return error.clone();
        }
        match self.exit_code {
            Some(0) | None => String::new(),
            Some(code) => {
                let stderr = self.stderr.trim();
                if stderr.is_empty() {
                    format!("Exit code {code}")
                } else {
                    let line = stderr.lines().next().unwrap_or(stderr);
                    format!("Exit code {code}: {line}")
                }
            }
        }
    }
}


/// Folder for exec cwd. Empty/`~` → local home on this machine; for a remote, omit cwd
/// so `hc exec` uses that machine's home (never the viewer's home path).
pub fn resolve_cwd(folder: &str, machine: Option<&str>, local_home: &str) -> Option<String> {
    let folder = folder.trim();
    if folder.is_empty() || folder == "~" {
        if machine.is_some() {
            None
        } else {
            Some(local_home.to_owned())
        }
    } else {
        Some(folder.to_owned())
    }
}

pub fn should_run(command: &str) -> bool {
    !command.trim().is_empty()
}

/// Toast for a failed space startup (optional action focuses the space).
pub fn failure_notification(space_name: &str, space_id: &str, result: &RunResult) -> serde_json::Value {
    serde_json::json!({
        "title": format!("Startup command failed in {space_name}"),
        "body": result.short_reason(),
        "level": "error",
        "action": { "name": "space.select", "args": { "id": space_id } },
    })
}

fn truncate(s: &str) -> String {
    if s.len() <= OUTPUT_CAP {
        return s.to_owned();
    }
    let mut end = OUTPUT_CAP;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// Run `command` in `cwd` on `machine` (None: this computer). Never panics.
pub fn run(command: &str, cwd: Option<String>, machine: Option<&str>) -> RunResult {
    let command = command.trim();
    if command.is_empty() {
        return RunResult { exit_code: Some(0), ..Default::default() };
    }
    let outcome = match machine {
        None => run_local(command, cwd),
        Some(m) => run_remote(m, command, cwd),
    };
    match outcome {
        Ok((code, stdout, stderr)) => RunResult {
            exit_code: Some(code),
            stdout: truncate(&stdout),
            stderr: truncate(&stderr),
            ..Default::default()
        },
        Err(error) => {
            let timed_out = error.contains("took longer than") || error.contains("was stopped");
            RunResult {
                timed_out,
                error: Some(error),
                ..Default::default()
            }
        }
    }
}

fn run_local(command: &str, cwd: Option<String>) -> Result<(i32, String, String), String> {
    if cfg!(windows) {
        let shell = crate::window::shell();
        crate::mods::run_blocking(
            &shell,
            &[
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                command.into(),
            ],
            cwd,
            TIMEOUT,
        )
    } else {
        let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".into());
        crate::mods::run_blocking(&shell, &["-lc".into(), command.into()], cwd, TIMEOUT)
    }
}

fn run_remote(machine: &str, command: &str, cwd: Option<String>) -> Result<(i32, String, String), String> {
    // Prefer POSIX; Windows remotes without sh fall back to PowerShell (same as agent checks).
    match crate::mods::run_remote(machine, "sh", &["-lc".into(), command.into()], cwd.clone()) {
        Err(e) if e.contains("cannot start sh") || e.contains("program not found") => {
            crate::mods::run_remote(
                machine,
                "powershell.exe",
                &[
                    "-NoLogo".into(),
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-Command".into(),
                    command.into(),
                ],
                cwd,
            )
            .map(|(c, o, e)| (c, String::from_utf8_lossy(&o).into_owned(), String::from_utf8_lossy(&e).into_owned()))
        }
        Ok((c, o, e)) => Ok((c, String::from_utf8_lossy(&o).into_owned(), String::from_utf8_lossy(&e).into_owned())),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_empty_or_tilde_cwd_is_machine_home_not_local() {
        assert_eq!(resolve_cwd("", Some("box"), "/local/home"), None);
        assert_eq!(resolve_cwd("~", Some("box"), "/local/home"), None);
        assert_eq!(resolve_cwd("  ~  ", Some("box"), "/local/home"), None);
        assert_eq!(resolve_cwd("/work", Some("box"), "/local/home").as_deref(), Some("/work"));
        assert_eq!(resolve_cwd("", None, "/local/home").as_deref(), Some("/local/home"));
        assert_eq!(resolve_cwd("~", None, "/local/home").as_deref(), Some("/local/home"));
    }

    #[test]
    fn empty_command_is_not_run() {
        assert!(!should_run(""));
        assert!(!should_run("  \n\t "));
        assert!(should_run("echo hi"));
    }

    #[test]
    fn once_per_open_gate_resets_on_close() {
        let mut g = Gate::default();
        assert!(g.claim("a"));
        assert!(!g.claim("a"));
        assert!(g.contains("a"));
        g.on_close("a");
        assert!(!g.contains("a"));
        assert!(g.claim("a"));
    }

    #[test]
    fn gate_drops_ids_no_longer_present() {
        let mut g = Gate::default();
        assert!(g.claim("keep"));
        assert!(g.claim("gone"));
        g.retain_present(|id| id == "keep");
        assert!(g.contains("keep"));
        assert!(!g.contains("gone"));
    }

    #[test]
    fn failure_notification_focuses_space() {
        let n = failure_notification(
            "Work",
            "s1",
            &RunResult {
                exit_code: Some(1),
                stderr: "boom\nmore".into(),
                ..Default::default()
            },
        );
        assert_eq!(n["title"], "Startup command failed in Work");
        assert_eq!(n["level"], "error");
        assert!(n["body"].as_str().unwrap().contains("Exit code 1"));
        assert_eq!(n["action"]["name"], "space.select");
        assert_eq!(n["action"]["args"]["id"], "s1");
    }

    #[cfg(unix)]
    #[test]
    fn local_echo_succeeds() {
        let r = run("echo ok", None, None);
        assert!(!r.failed(), "{r:?}");
        assert!(r.stdout.contains("ok"), "{r:?}");
    }

    #[cfg(unix)]
    #[test]
    fn local_false_fails_without_blocking_caller() {
        let r = run("false", None, None);
        assert!(r.failed());
        assert_eq!(r.exit_code, Some(1));
    }
}
