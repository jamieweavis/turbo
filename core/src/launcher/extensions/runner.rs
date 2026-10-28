//! Runs a shell command with a timeout and captures bounded output.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::config::CommandOutput;

/// Per stream; anything beyond is drained and dropped so the command never blocks on a
/// full pipe.
pub(super) const OUTPUT_LIMIT: usize = 64 * 1024;
const POLL: Duration = Duration::from_millis(50);
/// After the command exits, how long to wait for its output pipes to close. A background
/// job it started (`server &`) can hold them open indefinitely.
const DRAIN_GRACE: Duration = Duration::from_secs(1);

const SIGKILL: i32 = 9;
unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct RunResult {
    /// Exited with status 0.
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    /// Why it failed when there's no exit status to explain it.
    pub problem: Option<String>,
}

/// Runs `program args` from `dir` with no stdin. On timeout the whole process group is
/// killed, so children the shell started go too.
pub(super) fn run(program: &str, args: &[&str], dir: &str, timeout: Duration) -> RunResult {
    let spawned = Command::new(program)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            log::warn!("couldn't start {program}: {error}");
            return RunResult {
                success: false,
                stdout: String::new(),
                stderr: String::new(),
                problem: Some(format!("Couldn't start {program}: {error}")),
            };
        }
    };

    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());

    let started = Instant::now();
    let (success, problem) = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let problem = match status.code() {
                    Some(0) => None,
                    Some(code) => Some(format!("Exited with status {code}")),
                    None => Some("Terminated by a signal".to_owned()),
                };
                break (status.success(), problem);
            }
            Ok(None) if started.elapsed() >= timeout => {
                // SAFETY: plain syscall; a negative pid targets the process group we made.
                unsafe { kill(-(child.id() as i32), SIGKILL) };
                let _ = child.wait();
                break (
                    false,
                    Some(format!("Timed out after {} s", timeout.as_secs())),
                );
            }
            Ok(None) => thread::sleep(POLL),
            Err(error) => {
                break (
                    false,
                    Some(format!("Couldn't wait for the command: {error}")),
                );
            }
        }
    };

    // The program only: arguments can carry the user's query.
    match &problem {
        None => log::info!("{program} finished in {:?}", started.elapsed()),
        Some(problem) => log::warn!("{program} failed after {:?}: {problem}", started.elapsed()),
    }
    RunResult {
        success,
        stdout: stdout.recv_timeout(DRAIN_GRACE).unwrap_or_default(),
        stderr: stderr.recv_timeout(DRAIN_GRACE).unwrap_or_default(),
        problem,
    }
}

/// Reads a pipe to the end on its own thread, keeping the first `OUTPUT_LIMIT` bytes.
fn drain(pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut kept = Vec::new();
        let mut truncated = false;
        if let Some(mut pipe) = pipe {
            let mut buffer = [0u8; 8192];
            while let Ok(n) = pipe.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                let room = OUTPUT_LIMIT - kept.len();
                kept.extend_from_slice(&buffer[..n.min(room)]);
                truncated |= n > room;
            }
        }
        let mut text = String::from_utf8_lossy(&kept).into_owned();
        if truncated {
            text.push_str("\n… (output truncated)");
        }
        let _ = tx.send(text);
    });
    rx
}

/// What to tell the user once a command finishes.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Report {
    Nothing,
    /// Shown through `LauncherObserver::command_finished`.
    Notify {
        output: String,
        success: bool,
    },
    /// Copy this to the clipboard (a failure to copy is then reported).
    Copy(String),
}

pub(super) fn report(mode: CommandOutput, result: &RunResult) -> Report {
    if !result.success {
        return Report::Notify {
            output: failure_text(result),
            success: false,
        };
    }
    match mode {
        CommandOutput::Discard => Report::Nothing,
        CommandOutput::Show => Report::Notify {
            output: combined(result),
            success: true,
        },
        CommandOutput::Copy => {
            let text = result.stdout.strip_suffix('\n').unwrap_or(&result.stdout);
            Report::Copy(text.strip_suffix('\r').unwrap_or(text).to_owned())
        }
    }
}

/// stdout, then stderr, each only if non-empty.
fn combined(result: &RunResult) -> String {
    [result.stdout.trim_end(), result.stderr.trim_end()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The output (stderr is usually the explanation), led by the reason it failed.
fn failure_text(result: &RunResult) -> String {
    let output = combined(result);
    match (&result.problem, output.is_empty()) {
        (Some(problem), true) => problem.clone(),
        (Some(problem), false) => format!("{problem}\n{output}"),
        (None, _) => output,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str, timeout: Duration) -> RunResult {
        run("/bin/sh", &["-c", script], "/", timeout)
    }

    fn ok(stdout: &str, stderr: &str) -> RunResult {
        RunResult {
            success: true,
            stdout: stdout.into(),
            stderr: stderr.into(),
            problem: None,
        }
    }

    #[test]
    fn captures_both_streams() {
        let result = sh("echo out; echo err >&2", Duration::from_secs(5));
        assert_eq!(result, ok("out\n", "err\n"));
    }

    #[test]
    fn reports_exit_status() {
        let result = sh("echo nope >&2; exit 3", Duration::from_secs(5));
        assert!(!result.success);
        assert_eq!(result.problem.as_deref(), Some("Exited with status 3"));
        assert_eq!(result.stderr, "nope\n");
    }

    #[test]
    fn kills_the_process_group_on_timeout() {
        let started = Instant::now();
        let result = sh("sleep 30 & sleep 30", Duration::from_millis(300));
        assert!(!result.success);
        assert!(result.problem.unwrap().starts_with("Timed out"));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn caps_output() {
        let result = sh("yes | head -c 200000", Duration::from_secs(5));
        assert!(result.success);
        assert!(result.stdout.len() < OUTPUT_LIMIT + 64);
        assert!(result.stdout.ends_with("(output truncated)"));
    }

    #[test]
    fn missing_program_is_a_problem_not_a_panic() {
        let result = run("/no/such/program", &[], "/", Duration::from_secs(1));
        assert!(!result.success);
        assert!(result.problem.unwrap().starts_with("Couldn't start"));
    }

    #[test]
    fn runs_in_the_given_directory() {
        let result = run("/bin/pwd", &[], "/tmp", Duration::from_secs(5));
        assert!(result.stdout.trim_end().ends_with("tmp"));
    }

    #[test]
    fn discard_reports_only_failures() {
        assert_eq!(
            report(CommandOutput::Discard, &ok("x", "")),
            Report::Nothing
        );
        let failed = RunResult {
            success: false,
            stdout: String::new(),
            stderr: "boom\n".into(),
            problem: Some("Exited with status 1".into()),
        };
        assert_eq!(
            report(CommandOutput::Discard, &failed),
            Report::Notify {
                output: "Exited with status 1\nboom".into(),
                success: false
            }
        );
        assert_eq!(
            report(CommandOutput::Copy, &failed),
            report(CommandOutput::Discard, &failed)
        );
    }

    #[test]
    fn show_combines_streams() {
        assert_eq!(
            report(CommandOutput::Show, &ok("out\n", "warn\n")),
            Report::Notify {
                output: "out\nwarn".into(),
                success: true
            }
        );
        assert_eq!(
            report(CommandOutput::Show, &ok("", "")),
            Report::Notify {
                output: String::new(),
                success: true
            }
        );
    }

    #[test]
    fn copy_trims_one_trailing_newline() {
        assert_eq!(
            report(CommandOutput::Copy, &ok("value\n", "")),
            Report::Copy("value".into())
        );
        assert_eq!(
            report(CommandOutput::Copy, &ok("a\n\n", "")),
            Report::Copy("a\n".into())
        );
        assert_eq!(
            report(CommandOutput::Copy, &ok("crlf\r\n", "")),
            Report::Copy("crlf".into())
        );
    }
}
