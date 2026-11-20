//! Moves settings and state left behind by builds from when the app was called Backpack (and,
//! before that, Jaycast).

use std::fs;
use std::path::Path;

/// Earlier names, newest first: the first one found on disk is the one moved.
const LEGACY_NAMES: [&str; 2] = ["Backpack", "Jaycast"];

/// Renames `~/.config/<old>.toml` to `config_path` (a symlink is moved as a link) and the `<Old>`
/// folder beside `data_dir` to `data_dir`, each only when the new one is missing. The newest old
/// name present wins. The shell creates `data_dir` before the core starts, so an empty one counts
/// as missing. Failures are logged and the app starts with whatever is at the new paths.
pub fn migrate_legacy(config_path: &Path, data_dir: &Path) {
    if config_path.file_name() == Some("turbo.toml".as_ref())
        && let Some(dir) = config_path.parent()
        && let Some(legacy) = LEGACY_NAMES
            .iter()
            .map(|name| dir.join(format!("{}.toml", name.to_lowercase())))
            .find(|path| fs::symlink_metadata(path).is_ok())
    {
        move_if_missing(&legacy, config_path);
    }
    if data_dir.file_name() == Some("Turbo".as_ref())
        && let Some(parent) = data_dir.parent()
        && let Some(legacy) = LEGACY_NAMES
            .iter()
            .map(|name| parent.join(name))
            .find(|path| path.is_dir())
    {
        if is_empty_dir(data_dir) {
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
        let dir = std::env::temp_dir().join(format!("turbo-rename-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn moves_the_old_config_and_data_into_an_empty_new_dir() {
        let dir = scratch("moves");
        fs::write(dir.join("backpack.toml"), "old").unwrap();
        fs::create_dir(dir.join("Backpack")).unwrap();
        fs::write(dir.join("Backpack/emoji.json"), "[]").unwrap();
        fs::create_dir(dir.join("Turbo")).unwrap();

        migrate_legacy(&dir.join("turbo.toml"), &dir.join("Turbo"));

        assert_eq!(fs::read_to_string(dir.join("turbo.toml")).unwrap(), "old");
        assert!(dir.join("Turbo/emoji.json").exists());
        assert!(!dir.join("backpack.toml").exists());
        assert!(!dir.join("Backpack").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn moves_the_oldest_name_when_its_the_only_one() {
        let dir = scratch("oldest");
        fs::write(dir.join("jaycast.toml"), "older").unwrap();
        fs::create_dir(dir.join("Jaycast")).unwrap();
        fs::write(dir.join("Jaycast/emoji.json"), "[]").unwrap();

        migrate_legacy(&dir.join("turbo.toml"), &dir.join("Turbo"));

        assert_eq!(fs::read_to_string(dir.join("turbo.toml")).unwrap(), "older");
        assert!(dir.join("Turbo/emoji.json").exists());
        assert!(!dir.join("Jaycast").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prefers_the_newest_old_name() {
        let dir = scratch("newest");
        fs::write(dir.join("jaycast.toml"), "older").unwrap();
        fs::write(dir.join("backpack.toml"), "old").unwrap();
        fs::create_dir(dir.join("Jaycast")).unwrap();
        fs::create_dir(dir.join("Backpack")).unwrap();

        migrate_legacy(&dir.join("turbo.toml"), &dir.join("Turbo"));

        assert_eq!(fs::read_to_string(dir.join("turbo.toml")).unwrap(), "old");
        assert!(dir.join("jaycast.toml").exists());
        assert!(dir.join("Jaycast").exists());
        assert!(!dir.join("Backpack").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn leaves_existing_new_files_alone() {
        let dir = scratch("keeps");
        fs::write(dir.join("backpack.toml"), "old").unwrap();
        fs::write(dir.join("turbo.toml"), "new").unwrap();
        fs::create_dir_all(dir.join("Backpack")).unwrap();
        fs::create_dir_all(dir.join("Turbo")).unwrap();
        fs::write(dir.join("Turbo/emoji.json"), "[]").unwrap();

        migrate_legacy(&dir.join("turbo.toml"), &dir.join("Turbo"));

        assert_eq!(fs::read_to_string(dir.join("turbo.toml")).unwrap(), "new");
        assert!(dir.join("backpack.toml").exists());
        assert!(dir.join("Backpack").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ignores_custom_paths() {
        let dir = scratch("custom");
        fs::write(dir.join("backpack.toml"), "old").unwrap();
        fs::create_dir_all(dir.join("Backpack")).unwrap();

        migrate_legacy(&dir.join("config.toml"), &dir.join("data"));

        assert!(dir.join("backpack.toml").exists());
        assert!(!dir.join("data").exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
