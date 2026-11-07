//! The user's Shortcuts, listed in search and run through the `shortcuts` command line tool
//! that ships with macOS. It needs no permission of its own; a shortcut's actions may still
//! ask for theirs when it runs.
//!
//! Listing takes ~40 ms, so the launcher caches it and refreshes in the background when the
//! palette opens (see `Launcher::refresh_shortcuts_if_stale`).

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;

use crate::error::CoreError;

const SHORTCUTS: &str = "/usr/bin/shortcuts";

/// Where the results' icon comes from.
pub(crate) const APP_PATH: &str = "/System/Applications/Shortcuts.app";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Shortcut {
    /// The shortcut's UUID: stable across renames, and what `shortcuts run` is given.
    pub id: String,
    pub name: String,
}

/// Every shortcut, in the Shortcuts app's order. Blocks while the tool runs.
pub(crate) fn list() -> Result<Vec<Shortcut>, CoreError> {
    let output = Command::new(SHORTCUTS)
        .args(["list", "--show-identifiers"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| CoreError::Other {
            message: format!("couldn't list shortcuts: {error}"),
        })?;
    if !output.status.success() {
        return Err(CoreError::Other {
            message: format!("couldn't list shortcuts ({})", output.status),
        });
    }
    Ok(parse_list(&String::from_utf8_lossy(&output.stdout)))
}

/// Parses `shortcuts list --show-identifiers`: one `Name (UUID)` per line. The name itself
/// may contain parentheses, so the identifier is taken from the end.
fn parse_list(output: &str) -> Vec<Shortcut> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim_end();
            let rest = line.strip_suffix(')')?;
            let (name, id) = rest.rsplit_once(" (")?;
            let is_uuid = id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
            (is_uuid && !name.trim().is_empty()).then(|| Shortcut {
                id: id.to_owned(),
                name: name.trim().to_owned(),
            })
        })
        .collect()
}

/// Starts the shortcut and returns at once; `on_failure` gets the tool's error output if it
/// fails (on a background thread).
pub(crate) fn run(
    id: &str,
    on_failure: impl FnOnce(String) + Send + 'static,
) -> Result<(), CoreError> {
    let mut child = Command::new(SHORTCUTS)
        .args(["run", id])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| CoreError::Other {
            message: format!("couldn't run the shortcut: {error}"),
        })?;
    let id = id.to_owned();
    thread::spawn(move || {
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = pipe.read_to_string(&mut stderr);
        }
        match child.wait() {
            Ok(status) if status.success() => {}
            Ok(status) => {
                log::warn!("shortcut {id} failed ({status})");
                let stderr = stderr.trim();
                on_failure(if stderr.is_empty() {
                    format!("The shortcut failed ({status}).")
                } else {
                    stderr.to_owned()
                });
            }
            Err(error) => log::warn!("shortcut {id} failed: {error}"),
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_and_identifiers() {
        let output = "WiFiman (550E9DDA-CC71-4C83-94BA-71A9508CB563)\n\
                      Resize (Large) (11111111-2222-3333-4444-555555555555)\n\
                      \n\
                      Not a shortcut line\n\
                      Bad id (1234)\n";
        assert_eq!(
            parse_list(output),
            vec![
                Shortcut {
                    id: "550E9DDA-CC71-4C83-94BA-71A9508CB563".into(),
                    name: "WiFiman".into(),
                },
                Shortcut {
                    id: "11111111-2222-3333-4444-555555555555".into(),
                    name: "Resize (Large)".into(),
                },
            ]
        );
    }

    #[test]
    #[ignore = "runs the real shortcuts tool"]
    fn lists_this_macs_shortcuts() {
        println!("{:#?}", list().unwrap());
    }
}
