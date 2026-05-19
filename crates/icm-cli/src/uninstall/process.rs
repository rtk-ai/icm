//! Detect (but do **not** kill) running `icm serve` processes.
//!
//! Cross-platform safety: killing other processes from inside a CLI
//! command is fraught. We list them and let the user decide. The
//! orchestrator surfaces the list as a warning before `--purge-data`
//! mutation since serve holds the SQLite DB open via WAL.
//!
//! Implementation:
//! - Linux: walk `/proc/<pid>/cmdline` (NUL-separated argv).
//! - macOS: spawn `ps -eo pid=,command=` and parse line-by-line.
//! - Windows / other: stubbed `Ok(vec![])`. A future PR can use
//!   `tasklist /v` or `sysinfo`; uninstall still works without it,
//!   we just don't print a warning.

use super::discover::RunningProcess;

/// Return every process whose command line contains `"icm serve"`. The
/// caller's own PID is filtered out so an `icm uninstall` invocation
/// doesn't flag itself.
pub(crate) fn detect_icm_serve() -> Vec<RunningProcess> {
    detect_inner()
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.pid != std::process::id())
        .filter(|p| p.cmdline.contains("icm serve"))
        .collect()
}

#[cfg(target_os = "linux")]
fn detect_inner() -> Option<Vec<RunningProcess>> {
    use std::fs;
    let entries = fs::read_dir("/proc").ok()?;
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        let cmd_path = entry.path().join("cmdline");
        let Ok(raw) = fs::read(&cmd_path) else {
            continue;
        };
        // cmdline arguments are NUL-separated; replace with spaces.
        let cmdline: String = raw
            .into_iter()
            .map(|b| if b == 0 { b' ' } else { b })
            .map(char::from)
            .collect();
        let trimmed = cmdline.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }
        out.push(RunningProcess {
            pid,
            cmdline: trimmed,
        });
    }
    Some(out)
}

#[cfg(target_os = "macos")]
fn detect_inner() -> Option<Vec<RunningProcess>> {
    use std::process::Command;
    let output = Command::new("ps")
        .args(["-eo", "pid=,command="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut out = Vec::new();
    for line in stdout.lines() {
        let line = line.trim_start();
        let (pid_str, rest) = match line.split_once(char::is_whitespace) {
            Some(parts) => parts,
            None => continue,
        };
        let Ok(pid) = pid_str.parse::<u32>() else {
            continue;
        };
        out.push(RunningProcess {
            pid,
            cmdline: rest.trim().to_string(),
        });
    }
    Some(out)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn detect_inner() -> Option<Vec<RunningProcess>> {
    // Windows/BSD/other: no detection in this PR. Uninstall still works;
    // the report just won't surface a warning. A follow-up can add
    // `sysinfo` or `tasklist` integration.
    Some(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_runs_without_panic_and_filters_self() {
        // We can't assert on the *content* (the test runner's own pid
        // would show up in /proc), but the function must succeed and the
        // self-PID filter must hold.
        let procs = detect_icm_serve();
        for p in &procs {
            assert_ne!(p.pid, std::process::id());
            assert!(p.cmdline.contains("icm serve"));
        }
    }
}
