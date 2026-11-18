//! Moves settings and state left behind by builds from when the app was called Jaycast.

use std::fs;
use std::path::Path;

/// Renames `~/.config/jaycast.toml` to `config_path` (a symlink is moved as a link) and the
/// `Jaycast` folder beside `data_dir` to `data_dir`, each only when the new one is missing.
/// The shell creates `data_dir` before the core starts, so an empty one counts as missing.
/// Failures are logged and the app starts with whatever is at the new paths.
pub fn migrate_from_jaycast(config_path: &Path, data_dir: &Path) {
    if let Some(dir) = config_path.parent() {
        let legacy = dir.join("jaycast.toml");
        if config_path.file_name() == Some("backpack.toml".as_ref()) {
            move_if_missing(&legacy, config_path);
        }
    }
    if data_dir.file_name() == Some("Backpack".as_ref())
        && let Some(parent) = data_dir.parent()
    {
        let legacy = parent.join("Jaycast");
        if legacy.is_dir() && is_empty_dir(data_dir) {
            let _ = fs::remove_dir(data_dir);
        }
        move_if_missing(&legacy, data_dir);
    }
}

fn move_if_missing(from: &Path, to: &Path) {
    if fs::symlink_metadata(from).is_err() || fs::symlink_metadata(to).is_ok() {
        return;
    }
    match fs::rename(from, to) {
        Ok(()) => log::info!("moved {} to {}", from.display(), to.display()),
        Err(error) => log::error!(
            "couldn't move {} to {}: {error}",
            from.display(),
            to.display()
        ),
    }
}

fn is_empty_dir(path: &Path) -> bool {
    fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("backpack-rename-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn moves_the_old_config_and_data_into_an_empty_new_dir() {
        let dir = scratch("moves");
        fs::write(dir.join("jaycast.toml"), "old").unwrap();
        fs::create_dir(dir.join("Jaycast")).unwrap();
        fs::write(dir.join("Jaycast/emoji.json"), "[]").unwrap();
        fs::create_dir(dir.join("Backpack")).unwrap();

        migrate_from_jaycast(&dir.join("backpack.toml"), &dir.join("Backpack"));

        assert_eq!(
            fs::read_to_string(dir.join("backpack.toml")).unwrap(),
            "old"
        );
        assert!(dir.join("Backpack/emoji.json").exists());
        assert!(!dir.join("jaycast.toml").exists());
        assert!(!dir.join("Jaycast").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn leaves_existing_new_files_alone() {
        let dir = scratch("keeps");
        fs::write(dir.join("jaycast.toml"), "old").unwrap();
        fs::write(dir.join("backpack.toml"), "new").unwrap();
        fs::create_dir_all(dir.join("Jaycast")).unwrap();
        fs::create_dir_all(dir.join("Backpack")).unwrap();
        fs::write(dir.join("Backpack/emoji.json"), "[]").unwrap();

        migrate_from_jaycast(&dir.join("backpack.toml"), &dir.join("Backpack"));

        assert_eq!(
            fs::read_to_string(dir.join("backpack.toml")).unwrap(),
            "new"
        );
        assert!(dir.join("jaycast.toml").exists());
        assert!(dir.join("Jaycast").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ignores_custom_paths() {
        let dir = scratch("custom");
        fs::write(dir.join("jaycast.toml"), "old").unwrap();
        fs::create_dir_all(dir.join("Jaycast")).unwrap();

        migrate_from_jaycast(&dir.join("config.toml"), &dir.join("data"));

        assert!(dir.join("jaycast.toml").exists());
        assert!(!dir.join("data").exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
