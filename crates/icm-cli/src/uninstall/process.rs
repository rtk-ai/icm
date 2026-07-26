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

/// Result of a process-detection attempt.
pub(crate) struct ProcessDetection {
    pub processes: Vec<RunningProcess>,
    /// Detection wasn't possible — either an unsupported platform (the
    /// `detect_inner` stub below) or a runtime failure on a supported one
    /// (e.g. `/proc` unreadable, `ps` failed to spawn). Audit finding: an
    /// empty `processes` list is otherwise indistinguishable from
    /// "confirmed nothing running," silently defeating the one safeguard
    /// `--purge-data` has against WAL corruption from a live `icm serve`.
    /// Callers must treat this the same as "something might be running."
    pub unsupported: bool,
}

/// Detect running `icm serve` processes. The caller's own PID is filtered
/// out so an `icm uninstall` invocation doesn't flag itself.
pub(crate) fn detect_icm_serve() -> ProcessDetection {
    match detect_inner() {
        Some(procs) => {
            let processes = procs
                .into_iter()
                .filter(|p| p.pid != std::process::id())
                .filter(|p| is_icm_serve_cmdline(&p.cmdline))
                .collect();
            ProcessDetection {
                processes,
                unsupported: false,
            }
        }
        None => ProcessDetection {
            processes: Vec::new(),
            unsupported: true,
        },
    }
}

/// Whether `cmdline` looks like an `icm serve` invocation. Audit finding:
/// a raw `cmdline.contains("icm serve")` has a false-negative (any flag
/// between the binary and the subcommand, e.g. `icm --db /x serve`, breaks
/// the contiguous substring) and a false-positive (any unrelated process
/// whose argv happens to contain that literal text, e.g. `grep 'icm
/// serve' log.txt`). Instead: the first token's basename must be exactly
/// `icm` (allowing a `.exe` suffix), and some later token must be exactly
/// `serve`.
fn is_icm_serve_cmdline(cmdline: &str) -> bool {
    let mut tokens = cmdline.split_whitespace();
    let Some(argv0) = tokens.next() else {
        return false;
    };
    let basename = argv0.rsplit(['/', '\\']).next().unwrap_or(argv0);
    let basename = basename.strip_suffix(".exe").unwrap_or(basename);
    if basename != "icm" {
        return false;
    }
    tokens.any(|t| t == "serve")
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
    // Windows/BSD/other: no detection in this PR. `None` (not `Some(vec![])`)
    // — the caller treats this as "detection unavailable," which
    // --purge-data must handle the same as "something might be running"
    // rather than silently assuming the coast is clear (audit finding). A
    // follow-up can add `sysinfo` or `tasklist` integration.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_runs_without_panic_and_filters_self() {
        // We can't assert on the *content* (the test runner's own pid
        // would show up in /proc), but the function must succeed and the
        // self-PID filter must hold.
        let detection = detect_icm_serve();
        for p in &detection.processes {
            assert_ne!(p.pid, std::process::id());
            assert!(is_icm_serve_cmdline(&p.cmdline));
        }
    }

    /// Audit regression: a raw substring check (`cmdline.contains("icm
    /// serve")`) missed `icm --db /x serve` (a flag between binary and
    /// subcommand breaks the contiguous substring) and falsely matched an
    /// unrelated process like `grep 'icm serve' log.txt`.
    #[test]
    fn is_icm_serve_cmdline_matches_only_the_real_thing() {
        assert!(is_icm_serve_cmdline("icm serve"));
        assert!(is_icm_serve_cmdline("icm serve --http 127.0.0.1:11435"));
        assert!(is_icm_serve_cmdline(
            "icm --db /x/memories.db serve --compact"
        ));
        assert!(is_icm_serve_cmdline("/usr/local/bin/icm serve"));
        assert!(is_icm_serve_cmdline("C:\\Users\\pat\\bin\\icm.exe serve"));

        assert!(!is_icm_serve_cmdline("grep 'icm serve' log.txt"));
        assert!(!is_icm_serve_cmdline("icm decay"));
        assert!(!is_icm_serve_cmdline(""));
    }

    /// Audit regression: on an unsupported platform, detection used to
    /// return `Ok(vec![])`, indistinguishable from "confirmed nothing
    /// running" — silently defeating --purge-data's one safeguard against
    /// WAL corruption even without `-y`. The `unsupported` flag lets the
    /// caller refuse by default instead.
    #[test]
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn detect_icm_serve_reports_unsupported_on_unsupported_platforms() {
        let detection = detect_icm_serve();
        assert!(detection.unsupported);
        assert!(detection.processes.is_empty());
    }
}
