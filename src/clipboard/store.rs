//! The history database: SQLite at `<data_dir>/clipboard/history.sqlite`, with images as PNG
//! files beside it. Everything is owner-only (0700 / 0600): copies can hold secrets.

use std::fs::{self, DirBuilder};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};

use super::capture::{Content, truncate};
use super::{ClipboardItem, ClipboardKind};
use crate::error::CoreError;

/// `PRAGMA user_version` of the schema below; bump it and add a migration step to change it.
const SCHEMA_VERSION: i32 = 1;

/// Text handed to the shell per item is capped, to keep the FFI cheap; the full text is
/// still pasted.
const MAX_RETURNED_TEXT_BYTES: usize = 100 * 1024;

const SECONDS_PER_DAY: f64 = 86_400.0;

/// Where a copy came from.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Source {
    pub app: Option<String>,
    pub bundle_id: Option<String>,
}

/// The stored payload of one item, for writing it back to the pasteboard.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Stored {
    Text(String),
    Image(PathBuf),
    Files(Vec<String>),
}

/// Two connections to one WAL database, so the palette's lookups never wait on a write.
/// Writes take turns on `writer`, and an image is written to disk while it's held, so
/// pruning can't remove a file an insert is about to refer to.
pub(super) struct Store {
    writer: Mutex<Connection>,
    /// Read-only (`query_only`); WAL lets it read while a write is in progress.
    reader: Mutex<Connection>,
    images: PathBuf,
}

impl Store {
    /// Opens or creates the store in `dir` (e.g. `<data_dir>/clipboard`).
    pub fn open(dir: &Path) -> Result<Self, CoreError> {
        let images = dir.join("images");
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&images)?;
        // `recursive` leaves existing directories alone; tighten them too.
        for path in [dir, images.as_path()] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }

        let path = dir.join("history.sqlite");
        let connection = Connection::open(&path).map_err(sql_error)?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(sql_error)?;
        migrate(&connection)?;
        // The WAL and shared-memory files appear with the first write; create them now so
        // they get the same permissions.
        connection
            .execute_batch("BEGIN IMMEDIATE; COMMIT;")
            .map_err(sql_error)?;
        for suffix in ["", "-wal", "-shm"] {
            let file = PathBuf::from(format!("{}{suffix}", path.display()));
            if file.exists() {
                fs::set_permissions(&file, fs::Permissions::from_mode(0o600))?;
            }
        }

        let reader = Connection::open(&path).map_err(sql_error)?;
        reader
            .pragma_update(None, "query_only", true)
            .map_err(sql_error)?;

        let store = Self {
            writer: Mutex::new(connection),
            reader: Mutex::new(reader),
            images,
        };
        store.remove_orphaned_images();
        Ok(store)
    }

    /// Records a copy, or bumps the existing item with the same content (newest copy time
    /// and source). Images are written to disk first.
    pub fn insert(&self, content: &Content, source: &Source, now: f64) -> Result<(), CoreError> {
        let hash = content.hash();
        let file_paths = match content {
            Content::Files(paths) => serde_json::to_string(paths)?,
            _ => "[]".into(),
        };
        let writer = self.writer.lock();
        let image_path = match content {
            Content::Image { png, .. } => Some(self.write_image(&hash, png)?),
            _ => None,
        };
        writer
            .execute(
                "INSERT INTO items
                    (kind, text, image_path, file_paths, source_app, source_bundle_id, hash, copied_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(hash) DO UPDATE SET
                    copied_at = excluded.copied_at,
                    source_app = excluded.source_app,
                    source_bundle_id = excluded.source_bundle_id",
                params![
                    kind_code(content.kind()),
                    content.text(),
                    image_path.map(|p| p.to_string_lossy().into_owned()),
                    file_paths,
                    source.app,
                    source.bundle_id,
                    hash,
                    now,
                ],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    /// Pinned first, then newest. Every whitespace-separated word of `query` must appear in
    /// the text or the source app's name (case-insensitive for ASCII, as SQLite's LIKE is).
    pub fn items(&self, query: &str, limit: usize) -> Result<Vec<ClipboardItem>, CoreError> {
        let words: Vec<String> = query
            .split_whitespace()
            .map(|word| format!("%{}%", escape_like(word)))
            .collect();
        let filter = if words.is_empty() {
            String::new()
        } else {
            let clauses = (1..=words.len())
                .map(|i| {
                    format!(
                        "(text LIKE ?{i} ESCAPE '\\' OR IFNULL(source_app, '') LIKE ?{i} ESCAPE '\\')"
                    )
                })
                .collect::<Vec<_>>()
                .join(" AND ");
            format!("WHERE {clauses}")
        };
        let sql = format!(
            "SELECT id, kind, text, image_path, file_paths, source_app, source_bundle_id,
                    copied_at, pinned
             FROM items {filter}
             ORDER BY pinned DESC, copied_at DESC
             LIMIT {limit}"
        );
        let reader = self.reader.lock();
        let mut statement = reader.prepare(&sql).map_err(sql_error)?;
        let rows = statement
            .query_map(params_from_iter(words.iter()), item_from_row)
            .map_err(sql_error)?;
        rows.collect::<Result<_, _>>().map_err(sql_error)
    }

    /// The item's payload, or `None` if it no longer exists.
    pub fn stored(&self, id: i64) -> Result<Option<Stored>, CoreError> {
        let row = self
            .reader
            .lock()
            .query_row(
                "SELECT kind, text, image_path, file_paths FROM items WHERE id = ?1",
                [id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(sql_error)?;
        let Some((kind, text, image_path, file_paths)) = row else {
            return Ok(None);
        };
        Ok(Some(match kind_from_code(kind) {
            ClipboardKind::Text | ClipboardKind::Link => Stored::Text(text),
            ClipboardKind::Image => Stored::Image(PathBuf::from(image_path.unwrap_or_default())),
            ClipboardKind::Files => Stored::Files(serde_json::from_str(&file_paths)?),
        }))
    }

    /// Whether anything was deleted.
    pub fn delete(&self, id: i64) -> Result<bool, CoreError> {
        let writer = self.writer.lock();
        let images = image_paths(&writer, "WHERE id = ?1", [id])?;
        let deleted = writer
            .execute("DELETE FROM items WHERE id = ?1", [id])
            .map_err(sql_error)?;
        remove_files(&images);
        Ok(deleted > 0)
    }

    /// Whether anything changed.
    pub fn set_pinned(&self, id: i64, pinned: bool) -> Result<bool, CoreError> {
        let changed = self
            .writer
            .lock()
            .execute(
                "UPDATE items SET pinned = ?2 WHERE id = ?1 AND pinned != ?2",
                params![id, pinned],
            )
            .map_err(sql_error)?;
        Ok(changed > 0)
    }

    /// Deletes every unpinned item; returns how many.
    pub fn clear(&self) -> Result<usize, CoreError> {
        let writer = self.writer.lock();
        let images = image_paths(&writer, "WHERE pinned = 0", [])?;
        let deleted = writer
            .execute("DELETE FROM items WHERE pinned = 0", [])
            .map_err(sql_error)?;
        remove_files(&images);
        Ok(deleted)
    }

    /// Drops unpinned items older than `retention_days`, then the oldest unpinned ones
    /// beyond `max_items`; 0 lifts either limit. Returns how many were removed.
    pub fn prune(&self, max_items: u32, retention_days: u32, now: f64) -> Result<usize, CoreError> {
        let cutoff = match retention_days {
            0 => f64::MIN,
            days => now - f64::from(days) * SECONDS_PER_DAY,
        };
        // SQLite treats a negative LIMIT as no limit.
        let max_items = match max_items {
            0 => -1,
            count => i64::from(count),
        };
        let condition = "WHERE pinned = 0 AND (copied_at < ?1 OR id NOT IN (
                SELECT id FROM items WHERE pinned = 0 ORDER BY copied_at DESC LIMIT ?2))";
        let writer = self.writer.lock();
        let images = image_paths(&writer, condition, params![cutoff, max_items])?;
        let deleted = writer
            .execute(
                &format!("DELETE FROM items {condition}"),
                params![cutoff, max_items],
            )
            .map_err(sql_error)?;
        remove_files(&images);
        Ok(deleted)
    }

    fn write_image(&self, hash: &str, png: &[u8]) -> Result<PathBuf, CoreError> {
        let path = self.images.join(format!("{hash}.png"));
        if !path.exists() {
            // Write to a temp name and rename, so a half-written image is never referenced.
            let tmp = path.with_extension("png.tmp");
            fs::write(&tmp, png)?;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
            fs::rename(&tmp, &path)?;
        }
        Ok(path)
    }

    /// Deletes image files no item refers to (e.g. after a crash between steps).
    fn remove_orphaned_images(&self) {
        let Ok(referenced) = image_paths(&self.writer.lock(), "WHERE 1", []) else {
            return;
        };
        let Ok(entries) = fs::read_dir(&self.images) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !referenced.contains(&path) {
                let _ = fs::remove_file(path);
            }
        }
    }
}

fn image_paths(
    connection: &Connection,
    condition: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<PathBuf>, CoreError> {
    // `condition` always starts with WHERE.
    let sql = format!("SELECT image_path FROM items {condition} AND image_path IS NOT NULL");
    let mut statement = connection.prepare(&sql).map_err(sql_error)?;
    let rows = statement
        .query_map(params, |row| row.get::<_, String>(0))
        .map_err(sql_error)?;
    Ok(rows.filter_map(Result::ok).map(PathBuf::from).collect())
}

fn migrate(connection: &Connection) -> Result<(), CoreError> {
    let version: i32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(sql_error)?;
    if version < 1 {
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS items (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    kind INTEGER NOT NULL,
                    text TEXT NOT NULL,
                    image_path TEXT,
                    file_paths TEXT NOT NULL DEFAULT '[]',
                    source_app TEXT,
                    source_bundle_id TEXT,
                    hash TEXT NOT NULL UNIQUE,
                    copied_at REAL NOT NULL,
                    pinned INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE INDEX IF NOT EXISTS items_order ON items (pinned DESC, copied_at DESC);",
            )
            .map_err(sql_error)?;
    }
    if version > SCHEMA_VERSION {
        return Err(CoreError::Other {
            message: format!("clipboard history was written by a newer Jaycast (schema {version})"),
        });
    }
    connection
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(sql_error)?;
    Ok(())
}

fn item_from_row(row: &Row) -> rusqlite::Result<ClipboardItem> {
    let kind = kind_from_code(row.get(1)?);
    let text: String = row.get(2)?;
    let file_paths: String = row.get(4)?;
    Ok(ClipboardItem {
        id: row.get(0)?,
        kind,
        text: truncate(&text, MAX_RETURNED_TEXT_BYTES).to_owned(),
        image_path: row.get(3)?,
        file_paths: serde_json::from_str(&file_paths).unwrap_or_default(),
        source_app: row.get(5)?,
        source_bundle_id: row.get(6)?,
        copied_at: row.get(7)?,
        pinned: row.get(8)?,
    })
}

fn kind_code(kind: ClipboardKind) -> i64 {
    match kind {
        ClipboardKind::Text => 0,
        ClipboardKind::Link => 1,
        ClipboardKind::Image => 2,
        ClipboardKind::Files => 3,
    }
}

fn kind_from_code(code: i64) -> ClipboardKind {
    match code {
        1 => ClipboardKind::Link,
        2 => ClipboardKind::Image,
        3 => ClipboardKind::Files,
        _ => ClipboardKind::Text,
    }
}

/// Escapes LIKE's wildcards (`%`, `_`) and the escape character itself.
pub(super) fn escape_like(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

fn remove_files(paths: &[PathBuf]) {
    for path in paths {
        let _ = fs::remove_file(path);
    }
}

fn sql_error(error: rusqlite::Error) -> CoreError {
    CoreError::Other {
        message: format!("clipboard history: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("jaycast-clipboard-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn source(app: &str) -> Source {
        Source {
            app: Some(app.into()),
            bundle_id: Some(format!("test.{app}")),
        }
    }

    fn text(s: &str) -> Content {
        Content::Text(s.into())
    }

    fn texts(items: &[ClipboardItem]) -> Vec<&str> {
        items.iter().map(|i| i.text.as_str()).collect()
    }

    #[test]
    fn escape_like_escapes_wildcards() {
        assert_eq!(escape_like("50%_off\\"), "50\\%\\_off\\\\");
        assert_eq!(escape_like("plain"), "plain");
    }

    #[test]
    fn newest_first_and_recopying_bumps() {
        let dir = temp_dir("order");
        let store = Store::open(&dir).unwrap();
        store.insert(&text("one"), &source("A"), 1.0).unwrap();
        store.insert(&text("two"), &source("A"), 2.0).unwrap();
        store.insert(&text("one"), &source("B"), 3.0).unwrap();
        let items = store.items("", 10).unwrap();
        assert_eq!(texts(&items), vec!["one", "two"]);
        assert_eq!(items[0].source_app.as_deref(), Some("B"));
        assert_eq!(items[0].copied_at, 3.0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pinned_items_sort_first_and_survive_clear_and_prune() {
        let dir = temp_dir("pinned");
        let store = Store::open(&dir).unwrap();
        store.insert(&text("old"), &source("A"), 1.0).unwrap();
        store.insert(&text("new"), &source("A"), 2.0).unwrap();
        let old = store.items("old", 1).unwrap()[0].id;
        assert!(store.set_pinned(old, true).unwrap());
        assert!(!store.set_pinned(old, true).unwrap());
        assert_eq!(texts(&store.items("", 10).unwrap()), vec!["old", "new"]);

        // Pruning to one item far in the future drops everything unpinned.
        store.prune(0, 1, 1e12).unwrap();
        assert_eq!(texts(&store.items("", 10).unwrap()), vec!["old"]);
        store.insert(&text("again"), &source("A"), 3.0).unwrap();
        assert_eq!(store.clear().unwrap(), 1);
        assert_eq!(texts(&store.items("", 10).unwrap()), vec!["old"]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn prune_keeps_the_newest_within_limits() {
        let dir = temp_dir("prune");
        let store = Store::open(&dir).unwrap();
        for (i, t) in ["a", "b", "c", "d"].iter().enumerate() {
            store
                .insert(&text(t), &source("A"), i as f64 * SECONDS_PER_DAY)
                .unwrap();
        }
        // At day 3: retention 2 days drops "a" (day 0); max 2 then drops "b".
        assert_eq!(store.prune(2, 2, 3.0 * SECONDS_PER_DAY).unwrap(), 2);
        assert_eq!(texts(&store.items("", 10).unwrap()), vec!["d", "c"]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn prune_with_zero_limits_keeps_everything() {
        let dir = temp_dir("prune-unlimited");
        let store = Store::open(&dir).unwrap();
        for (i, t) in ["a", "b", "c"].iter().enumerate() {
            store
                .insert(&text(t), &source("A"), i as f64 * SECONDS_PER_DAY)
                .unwrap();
        }
        assert_eq!(store.prune(0, 0, 10_000.0 * SECONDS_PER_DAY).unwrap(), 0);
        // Each limit still applies on its own.
        assert_eq!(store.prune(0, 2, 3.0 * SECONDS_PER_DAY).unwrap(), 1);
        assert_eq!(store.prune(1, 0, 3.0 * SECONDS_PER_DAY).unwrap(), 1);
        assert_eq!(texts(&store.items("", 10).unwrap()), vec!["c"]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn query_matches_every_word_in_text_or_source() {
        let dir = temp_dir("query");
        let store = Store::open(&dir).unwrap();
        store
            .insert(&text("Hello World"), &source("Notes"), 1.0)
            .unwrap();
        store
            .insert(&text("100% sure"), &source("Mail"), 2.0)
            .unwrap();
        store
            .insert(&text("100 percent"), &source("Mail"), 3.0)
            .unwrap();
        assert_eq!(
            texts(&store.items("world hello", 10).unwrap()),
            vec!["Hello World"]
        );
        assert_eq!(
            texts(&store.items("notes", 10).unwrap()),
            vec!["Hello World"]
        );
        // `%` is literal, not a wildcard.
        assert_eq!(texts(&store.items("100%", 10).unwrap()), vec!["100% sure"]);
        assert_eq!(store.items("mail", 1).unwrap().len(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn images_and_files_round_trip_and_images_are_cleaned_up() {
        let dir = temp_dir("payloads");
        let store = Store::open(&dir).unwrap();
        let image = Content::Image {
            png: vec![1, 2, 3],
            width: 2,
            height: 1,
        };
        store.insert(&image, &Source::default(), 1.0).unwrap();
        let files = Content::Files(vec!["/tmp/a b.txt".into()]);
        store.insert(&files, &Source::default(), 2.0).unwrap();

        let items = store.items("", 10).unwrap();
        assert_eq!(items[0].kind, ClipboardKind::Files);
        assert_eq!(items[0].file_paths, vec!["/tmp/a b.txt".to_string()]);
        assert_eq!(items[0].text, "a b.txt");
        assert_eq!(items[1].kind, ClipboardKind::Image);
        let image_path = PathBuf::from(items[1].image_path.clone().unwrap());
        assert_eq!(fs::read(&image_path).unwrap(), vec![1, 2, 3]);
        let mode = fs::metadata(&image_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(
            store.stored(items[1].id).unwrap(),
            Some(Stored::Image(image_path.clone()))
        );
        assert_eq!(
            store.stored(items[0].id).unwrap(),
            Some(Stored::Files(vec!["/tmp/a b.txt".into()]))
        );

        assert!(store.delete(items[1].id).unwrap());
        assert!(!image_path.exists());
        assert_eq!(store.stored(items[1].id).unwrap(), None);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn store_is_owner_only_and_reopens() {
        let dir = temp_dir("perms");
        {
            let store = Store::open(&dir).unwrap();
            store.insert(&text("secret"), &source("A"), 1.0).unwrap();
        }
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join("history.sqlite")), 0o600);
        let store = Store::open(&dir).unwrap();
        assert_eq!(texts(&store.items("", 10).unwrap()), vec!["secret"]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn returned_text_is_capped() {
        let dir = temp_dir("cap");
        let store = Store::open(&dir).unwrap();
        let long = "x".repeat(MAX_RETURNED_TEXT_BYTES * 2);
        store.insert(&text(&long), &source("A"), 1.0).unwrap();
        let items = store.items("", 1).unwrap();
        assert_eq!(items[0].text.len(), MAX_RETURNED_TEXT_BYTES);
        assert_eq!(store.stored(items[0].id).unwrap(), Some(Stored::Text(long)));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_database_from_a_newer_version_is_refused() {
        let dir = temp_dir("newer-schema");
        {
            let store = Store::open(&dir).unwrap();
            store
                .writer
                .lock()
                .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
                .unwrap();
        }
        let Err(CoreError::Other { message }) = Store::open(&dir) else {
            panic!("a newer schema was opened");
        };
        assert!(message.contains("newer Jaycast"), "{message}");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn orphaned_images_are_removed_on_open() {
        let dir = temp_dir("orphans");
        let image = Content::Image {
            png: vec![9],
            width: 1,
            height: 1,
        };
        let kept = {
            let store = Store::open(&dir).unwrap();
            store.insert(&image, &Source::default(), 1.0).unwrap();
            PathBuf::from(store.items("", 1).unwrap()[0].image_path.clone().unwrap())
        };
        // Left behind by a crash between writing an image and recording it.
        let orphan = dir.join("images").join("orphan.png");
        let partial = dir.join("images").join("half.png.tmp");
        fs::write(&orphan, [1]).unwrap();
        fs::write(&partial, [1]).unwrap();

        let _store = Store::open(&dir).unwrap();
        assert!(kept.exists());
        assert!(!orphan.exists());
        assert!(!partial.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recopying_an_image_reuses_its_file_and_clear_removes_it() {
        let dir = temp_dir("image-dedupe");
        let store = Store::open(&dir).unwrap();
        let image = Content::Image {
            png: vec![4, 5, 6],
            width: 3,
            height: 1,
        };
        store.insert(&image, &source("A"), 1.0).unwrap();
        store.insert(&image, &source("B"), 2.0).unwrap();
        let items = store.items("", 10).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].source_app.as_deref(), Some("B"));
        assert_eq!(fs::read_dir(dir.join("images")).unwrap().count(), 1);

        assert_eq!(store.clear().unwrap(), 1);
        assert_eq!(fs::read_dir(dir.join("images")).unwrap().count(), 0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn links_keep_their_kind_and_paste_back_as_text() {
        let dir = temp_dir("links");
        let store = Store::open(&dir).unwrap();
        let link = Content::Link("https://example.com".into());
        store.insert(&link, &source("Safari"), 1.0).unwrap();
        let item = &store.items("", 1).unwrap()[0];
        assert_eq!(item.kind, ClipboardKind::Link);
        assert_eq!(item.source_bundle_id.as_deref(), Some("test.Safari"));
        assert!(!item.pinned);
        assert_eq!(
            store.stored(item.id).unwrap(),
            Some(Stored::Text("https://example.com".into()))
        );
        for kind in [
            ClipboardKind::Text,
            ClipboardKind::Link,
            ClipboardKind::Image,
            ClipboardKind::Files,
        ] {
            assert_eq!(kind_from_code(kind_code(kind)), kind);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn lookups_run_while_a_write_is_in_progress() {
        let dir = temp_dir("concurrent");
        let store = Store::open(&dir).unwrap();
        store.insert(&text("committed"), &source("A"), 1.0).unwrap();
        let writer = store.writer.lock();
        writer
            .execute_batch(
                "BEGIN IMMEDIATE;
                 INSERT INTO items (kind, text, hash, copied_at) VALUES (0, 'pending', 'p', 2.0);",
            )
            .unwrap();

        // On another thread, so a lookup that waits on the writer times out instead of
        // hanging. The writer is released before asserting, so the scope can always join.
        let (tx, rx) = std::sync::mpsc::channel();
        let result = std::thread::scope(|scope| {
            scope.spawn(|| {
                let items = store.items("", 10).unwrap();
                let stored = store.stored(items[0].id).unwrap();
                let _ = tx.send((items, stored));
            });
            let result = rx.recv_timeout(std::time::Duration::from_secs(5));
            writer.execute_batch("ROLLBACK;").unwrap();
            drop(writer);
            result
        });
        let (items, stored) = result.expect("lookups waited on the writer");
        assert_eq!(texts(&items), vec!["committed"]);
        assert_eq!(stored, Some(Stored::Text("committed".into())));
        fs::remove_dir_all(dir).unwrap();
    }
}
