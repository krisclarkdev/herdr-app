//! App-level PowerShell startup script: once per window, after the UI is up.
//! Failures never block or abort window open; they toast and persist a last-run log.

use crate::{persist, settings::Person, spawn};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::PathBuf,
    process::{Child, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const LAST_RUN_FILE: &str = "startup-script-last.json";
const OUTPUT_CAP: usize = 64 * 1024;
const TRUNCATION_MARK: &str = "\n…[truncated]…\n";
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

static RAN_THIS_PROCESS: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LastRun {
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub timed_out: bool,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl LastRun {
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

pub fn last_run_path() -> PathBuf {
    crate::mods::app_dir().join(LAST_RUN_FILE)
}

pub fn load_last_run() -> Option<LastRun> {
    let text = std::fs::read_to_string(last_run_path()).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_last_run(run: &LastRun) -> std::io::Result<()> {
    let body = serde_json::to_string_pretty(run).unwrap_or_else(|_| "{}".into()) + "\n";
    persist::write_atomic(&last_run_path(), body)
}

/// Cap stored stdout/stderr so Settings stays responsive (head + tail).
pub fn truncate_output(s: &str) -> String {
    if s.len() <= OUTPUT_CAP {
        return s.to_owned();
    }
    let keep = (OUTPUT_CAP.saturating_sub(TRUNCATION_MARK.len())) / 2;
    let head_end = floor_char_boundary(s, keep);
    let tail_start = ceil_char_boundary(s, s.len().saturating_sub(keep));
    format!("{}{}{}", &s[..head_end], TRUNCATION_MARK, &s[tail_start..])
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// True when a non-empty enabled script should be scheduled (platform gated separately).
pub fn should_schedule(person: &Person) -> bool {
    person.startup_script_enabled && !person.startup_script.trim().is_empty()
}

/// Claim the once-per-process gate. Returns true only the first time.
pub fn claim_once() -> bool {
    RAN_THIS_PROCESS.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok()
}

#[cfg(test)]
pub fn reset_once_for_tests() {
    RAN_THIS_PROCESS.store(false, Ordering::SeqCst);
}

/// Windows-first auto-run. Non-Windows builds never spawn.
pub fn runs_on_this_os() -> bool {
    cfg!(windows)
}

pub fn run(script: &str) -> LastRun {
    run_with_timeout(script, DEFAULT_TIMEOUT)
}

pub fn run_with_timeout(script: &str, timeout: Duration) -> LastRun {
    let started_at_ms = now_ms();
    if !runs_on_this_os() {
        return LastRun {
            started_at_ms,
            finished_at_ms: now_ms(),
            exit_code: Some(0),
            error: Some("Startup scripts run on Windows only".into()),
            ..Default::default()
        };
    }
    match run_powershell(script, timeout) {
        Ok(mut run) => {
            run.started_at_ms = started_at_ms;
            if run.finished_at_ms == 0 {
                run.finished_at_ms = now_ms();
            }
            run.stdout = truncate_output(&run.stdout);
            run.stderr = truncate_output(&run.stderr);
            run
        }
        Err(error) => LastRun {
            started_at_ms,
            finished_at_ms: now_ms(),
            error: Some(error),
            ..Default::default()
        },
    }
}

fn run_powershell(script: &str, timeout: Duration) -> Result<LastRun, String> {
    let dir = std::env::temp_dir().join(format!("herdr-startup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create temp dir: {e}"))?;
    let path = dir.join("startup.ps1");
    // BOM-less UTF-8; PowerShell -File accepts it on modern hosts.
    std::fs::write(&path, script.as_bytes()).map_err(|e| format!("Cannot write temp script: {e}"))?;
    let shell = crate::window::shell();
    let cwd = crate::mods::home();
    let mut child = spawn::detached_command(&shell)
        .args(["-NoLogo", "-NonInteractive", "-File"])
        .arg(&path)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Cannot start {shell}: {e}"))?;
    let result = wait_child(&mut child, timeout);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
    result
}

fn wait_child(child: &mut Child, timeout: Duration) -> Result<LastRun, String> {
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let stdout_thread = std::thread::spawn(move || read_pipe(&mut stdout_pipe));
    let stderr_thread = std::thread::spawn(move || read_pipe(&mut stderr_pipe));
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                timed_out = true;
                let _ = child.kill();
                break child.wait().map_err(|e| format!("Wait after kill failed: {e}"))?;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("Cannot wait for startup script: {e}")),
        }
    };
    let stdout = stdout_thread.join().unwrap_or_default();
    let stderr = stderr_thread.join().unwrap_or_default();
    Ok(LastRun {
        started_at_ms: 0,
        finished_at_ms: now_ms(),
        exit_code: status.code(),
        timed_out,
        stdout,
        stderr,
        error: None,
    })
}

fn read_pipe(pipe: &mut Option<impl Read>) -> String {
    let Some(pipe) = pipe.as_mut() else {
        return String::new();
    };
    let mut buf = Vec::new();
    let _ = pipe.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

/// Build the toast notification for a failed run (Settings → Scripts action).
pub fn failure_notification(run: &LastRun) -> serde_json::Value {
    serde_json::json!({
        "title": "Startup script failed",
        "body": run.short_reason(),
        "level": "error",
        "action": { "name": "app.settings", "args": { "section": "scripts" } },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_or_disabled_script_is_not_scheduled() {
        let mut person = Person::default();
        assert!(!should_schedule(&person));
        person.startup_script = "  \n\t  ".into();
        assert!(!should_schedule(&person));
        person.startup_script = "Write-Output ok".into();
        assert!(should_schedule(&person));
        person.startup_script_enabled = false;
        assert!(!should_schedule(&person));
    }

    #[test]
    fn once_gate_fires_only_first_claim() {
        reset_once_for_tests();
        assert!(claim_once());
        assert!(!claim_once());
        assert!(!claim_once());
        reset_once_for_tests();
        assert!(claim_once());
        reset_once_for_tests();
    }

    #[test]
    fn truncate_keeps_head_and_tail() {
        let s = "a".repeat(OUTPUT_CAP + 100);
        let out = truncate_output(&s);
        assert!(out.len() <= OUTPUT_CAP + TRUNCATION_MARK.len());
        assert!(out.starts_with('a'));
        assert!(out.ends_with('a'));
        assert!(out.contains(TRUNCATION_MARK));
        assert_eq!(truncate_output("short"), "short");
    }

    #[test]
    fn last_run_round_trips_and_reports_failure() {
        let dir = std::env::temp_dir().join(format!(
            "herdr-startup-last-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var_os("HERDR_APP_HOME");
        // SAFETY: test-only env for app_dir().
        unsafe { std::env::set_var("HERDR_APP_HOME", &dir) };
        let run = LastRun {
            started_at_ms: 1,
            finished_at_ms: 2,
            exit_code: Some(1),
            timed_out: false,
            stdout: "out".into(),
            stderr: "err line\nmore".into(),
            error: None,
        };
        assert!(run.failed());
        assert!(run.short_reason().contains("Exit code 1"));
        save_last_run(&run).unwrap();
        assert_eq!(load_last_run().as_ref(), Some(&run));
        match prev {
            Some(v) => unsafe { std::env::set_var("HERDR_APP_HOME", v) },
            None => unsafe { std::env::remove_var("HERDR_APP_HOME") },
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn non_windows_auto_run_is_gated() {
        assert_eq!(runs_on_this_os(), cfg!(windows));
    }

    #[test]
    fn failure_notification_opens_scripts() {
        let n = failure_notification(&LastRun {
            exit_code: Some(2),
            stderr: "boom".into(),
            ..Default::default()
        });
        assert_eq!(n["title"], "Startup script failed");
        assert_eq!(n["level"], "error");
        assert_eq!(n["action"]["name"], "app.settings");
        assert_eq!(n["action"]["args"]["section"], "scripts");
    }
}
