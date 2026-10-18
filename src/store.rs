//! On-disk note store.
//!
//! Layout is the one the Android app expects to find over WebDAV, so the
//! desktop app and the sync server share a single directory:
//!
//! ```text
//! <data-dir>/
//!   notes/            <uuid>.json          (master)
//!   notes/<Folder>/   <uuid>.json          (notes inside a folder)
//!   notes-md/         <Title>.md           (human-readable mirror)
//! ```

use crate::model::Note;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Shared deletion ledger the Android app syncs (`<notes>/deletions.json`).
/// Listing a note id here is a tombstone that stops the phone resurrecting a
/// note the desktop hard-deleted.
const DELETIONS_FILE: &str = "deletions.json";
/// Upstream folder metadata. It is a JSON array, not a note payload, and must
/// remain available over WebDAV without being parsed into the note list.
const FOLDERS_FILE: &str = "folders.json";

/// Folder metadata shared with the Android app. Directories carry note files,
/// while this ledger carries empty folders and deletion tombstones.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct FolderMeta {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    color: Option<String>,
    #[serde(default, rename = "updatedAt")]
    updated_at: i64,
    #[serde(default)]
    deleted: bool,
    #[serde(flatten)]
    extra: std::collections::BTreeMap<String, serde_json::Value>,
}

pub struct Store {
    root: PathBuf,
    /// Write the `/notes-md/` mirror alongside the JSON master.
    pub markdown_mirror: bool,
    /// Stamped on every write so the filesystem watcher can tell our own
    /// changes from ones arriving over WebDAV.
    pub write_stamp: crate::watcher::WriteStamp,
}

impl Store {
    pub fn new(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(root.join("notes"))?;
        fs::create_dir_all(root.join("notes-md"))?;
        Ok(Self {
            root,
            markdown_mirror: true,
            write_stamp: crate::watcher::WriteStamp::default(),
        })
    }

    #[allow(dead_code)] // used by tests and by the "open notes folder" action
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Loads every note under `notes/`, including one level of folder
    /// subdirectories. A note that fails to parse is skipped rather than
    /// aborting the whole load — a single bad file shouldn't hide the rest.
    pub fn load_all(&self) -> Result<Vec<Note>> {
        let mut notes = Vec::new();
        let mut failures = Vec::new();
        collect(&self.root.join("notes"), None, &mut notes, &mut failures)?;
        for (path, err) in &failures {
            eprintln!("skipping unreadable note {}: {err}", path.display());
        }
        // A folder move over WebDAV is a PUT in the new directory plus a DELETE
        // in the old one. If that sequence is interrupted, both copies can be
        // left behind. Show one logical note, choosing the newest payload by
        // the same last-write-wins rule used by sync/import.
        let mut by_id: std::collections::BTreeMap<String, Note> = std::collections::BTreeMap::new();
        for note in notes {
            let replace = by_id
                .get(&note.id)
                .map_or(true, |current| note.updated_at > current.updated_at);
            if replace {
                by_id.insert(note.id.clone(), note);
            }
        }
        let mut notes: Vec<Note> = by_id.into_values().collect();
        notes.sort_by(|a, b| {
            b.is_pinned
                .cmp(&a.is_pinned)
                .then(b.updated_at.cmp(&a.updated_at))
        });
        Ok(notes)
    }

    fn json_path(&self, note: &Note) -> PathBuf {
        let mut p = self.root.join("notes");
        if let Some(folder) = note.folder_name.as_deref().filter(|f| !f.is_empty()) {
            p.push(sanitize_component(folder));
        }
        p.join(format!("{}.json", note.id))
    }

    fn markdown_path(&self, note: &Note) -> PathBuf {
        let mut p = self.root.join("notes-md");
        if let Some(folder) = note.folder_name.as_deref().filter(|f| !f.is_empty()) {
            p.push(sanitize_component(folder));
        }
        p.join(note.markdown_filename())
    }

    pub fn save(&self, note: &Note) -> Result<()> {
        self.write_stamp.touch();
        if let Some(folder) = note.folder_name.as_deref().filter(|name| !name.is_empty()) {
            self.ensure_folder_active(folder)?;
        }
        let json_path = self.json_path(note);
        let json_bytes = serde_json::to_vec_pretty(note)?;
        // Write the new copy before removing the old location. The previous
        // delete-first order could erase a perfectly good note if the target
        // folder was invalid, the disk filled up, or a later write failed.
        write_atomic(&json_path, &json_bytes)
            .with_context(|| format!("writing {}", json_path.display()))?;

        let markdown_path = if self.markdown_mirror {
            let md_path = self.markdown_path(note);
            write_atomic(&md_path, note.to_markdown().as_bytes())
                .with_context(|| format!("writing {}", md_path.display()))?;
            Some(md_path)
        } else {
            None
        };

        // Both desired files now exist. Remove stale copies left by a folder
        // move, title rename, or disabling the Markdown mirror, but never the
        // final paths just written.
        self.remove_files_except(&note.id, Some(&json_path), markdown_path.as_deref())?;
        Ok(())
    }

    /// Hard-deletes a note's files. Moving a note to the trash is a metadata
    /// change (`trashedAt`) and goes through [`Store::save`] instead — that
    /// keeps the Android app's retention sweep authoritative.
    pub fn delete(&self, id: &str) -> Result<()> {
        self.write_stamp.touch();
        // Record a tombstone BEFORE removing the files, so a phone that still has
        // this note deletes it on next sync instead of re-uploading it (a
        // "zombie"). The Android app reads notes/deletions.json and skips /
        // removes any id listed there. Without this, a hard delete on the desktop
        // is silently undone by the phone.
        // Failing closed is essential: deleting the local file without a
        // tombstone lets the phone resurrect it on the next sync.
        self.record_deletion(id)?;
        self.remove_files_except(id, None, None)
    }

    /// Appends `id` to the shared `notes/deletions.json` ledger that the Android
    /// app's sync reads, in its exact wire format
    /// (`{version, deletedNotes:[{id, deletedAt, deviceId}]}`). Idempotent: an id
    /// already present is left as-is.
    pub fn record_deletion(&self, id: &str) -> Result<()> {
        let path = self.root.join("notes").join(DELETIONS_FILE);
        let mut ledger: serde_json::Value = fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| serde_json::json!({ "version": 1, "deletedNotes": [] }));

        let records = ledger
            .get_mut("deletedNotes")
            .and_then(|v| v.as_array_mut())
            .ok_or_else(|| anyhow::anyhow!("deletions.json has no deletedNotes array"))?;
        if records
            .iter()
            .any(|r| r.get("id").and_then(|v| v.as_str()) == Some(id))
        {
            return Ok(()); // already tombstoned
        }
        records.push(serde_json::json!({
            "id": id,
            "deletedAt": crate::model::now_millis(),
            "deviceId": device_id(),
        }));

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        write_atomic(&path, serde_json::to_string_pretty(&ledger)?.as_bytes())?;
        Ok(())
    }

    /// Removes a note's files without writing a tombstone — for a delete that
    /// came *from* sync, where the ledger already holds the record. Using
    /// [`Store::delete`] here would re-date the tombstone on every device that
    /// applies it.
    pub fn delete_synced(&self, id: &str) -> Result<()> {
        self.write_stamp.touch();
        self.remove_files_except(id, None, None)
    }

    /// The shared deletion ledger as raw JSON, or an empty one if absent.
    pub fn deletions(&self) -> serde_json::Value {
        fs::read_to_string(self.root.join("notes").join(DELETIONS_FILE))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| serde_json::json!({ "version": 1, "deletedNotes": [] }))
    }

    /// Overwrites the deletion ledger with a merged one.
    pub fn write_deletions(&self, ledger: &serde_json::Value) -> Result<()> {
        let path = self.root.join("notes").join(DELETIONS_FILE);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        write_atomic(&path, serde_json::to_string_pretty(ledger)?.as_bytes())
    }

    fn remove_files_except(
        &self,
        id: &str,
        keep_json: Option<&Path>,
        keep_markdown: Option<&Path>,
    ) -> Result<()> {
        for dir in walk_dirs(&self.root.join("notes"))? {
            let p = dir.join(format!("{id}.json"));
            if p.exists() && keep_json != Some(p.as_path()) {
                fs::remove_file(&p)?;
            }
        }
        // The Markdown mirror is named after the title, so find it by frontmatter.
        for dir in walk_dirs(&self.root.join("notes-md"))? {
            for entry in fs::read_dir(&dir)? {
                let p = entry?.path();
                if keep_markdown != Some(p.as_path())
                    && p.extension().is_some_and(|e| e == "md")
                    && fs::read_to_string(&p)
                        .map(|s| s.lines().any(|line| line == format!("id: {id}")))
                        .unwrap_or(false)
                {
                    fs::remove_file(&p)?;
                }
            }
        }
        Ok(())
    }

    /// Creates an empty folder.
    ///
    /// Folders are directories, so an empty one is a real thing on disk and
    /// syncs to the phone as soon as it holds a note. Returns the sanitised
    /// name actually used.
    pub fn create_folder(&self, name: &str) -> Result<String> {
        let clean = sanitize_component(name);
        if clean.is_empty() {
            anyhow::bail!("folder name is empty after removing illegal characters");
        }
        let path = self.root.join("notes").join(&clean);
        if self
            .folders()?
            .iter()
            .any(|folder| folder.to_lowercase() == clean.to_lowercase())
        {
            anyhow::bail!("a folder called \"{clean}\" already exists");
        }
        self.write_stamp.touch();
        fs::create_dir_all(&path)?;
        if self.markdown_mirror {
            fs::create_dir_all(self.root.join("notes-md").join(&clean))?;
        }
        if let Err(e) = self.set_folder_deleted(&clean, false) {
            // Creation has not become visible to the caller yet, so roll back
            // the empty directory shells if the sync metadata could not save.
            let _ = fs::remove_dir(&path);
            let _ = fs::remove_dir(self.root.join("notes-md").join(&clean));
            return Err(e);
        }
        Ok(clean)
    }

    /// Moves a note to a folder (or to no folder when `folder` is None).
    /// `save` already removes the old-location copy, so this is just a save
    /// with a changed `folder_name` — named for the intent.
    pub fn move_note(&self, note: &Note, folder: Option<String>) -> Result<()> {
        let mut moved = note.clone();
        moved.folder_name = folder.filter(|f| !f.is_empty());
        moved.touch_now();
        self.save(&moved)
    }

    /// Deletes a folder, moving any notes it holds back to the top level first
    /// so nothing is lost. Returns how many notes were moved out.
    pub fn delete_folder(&self, name: &str) -> Result<usize> {
        let clean = sanitize_component(name);
        let mut moved = 0;
        for note in self.load_all()? {
            if note.folder_name.as_deref() == Some(name) {
                self.move_note(&note, None)?;
                moved += 1;
            }
        }
        self.set_folder_deleted(&clean, true)?;
        // Remove the now-empty directories in both trees. `remove_dir` only
        // succeeds if empty — a safety net against deleting a note we missed.
        let _ = fs::remove_dir(self.root.join("notes").join(&clean));
        let _ = fs::remove_dir(self.root.join("notes-md").join(&clean));
        self.write_stamp.touch();
        Ok(moved)
    }

    /// Deletes a folder's directories only if it holds no notes. Used to
    /// auto-clean a folder that just lost its last note (the "emptying =
    /// deleting" behaviour), without touching a folder that still has notes.
    pub fn prune_folder_if_empty(&self, name: &str) -> Result<bool> {
        let has_notes = self
            .load_all()?
            .iter()
            .any(|n| n.folder_name.as_deref() == Some(name));
        if has_notes {
            return Ok(false);
        }
        let clean = sanitize_component(name);
        self.set_folder_deleted(&clean, true)?;
        let _ = fs::remove_dir(self.root.join("notes").join(&clean));
        let _ = fs::remove_dir(self.root.join("notes-md").join(&clean));
        self.write_stamp.touch();
        Ok(true)
    }

    /// Permanently deletes every note currently in the trash. Returns the count.
    pub fn empty_trash(&self) -> Result<usize> {
        let mut n = 0;
        for note in self.load_all()? {
            if note.is_trashed() {
                self.delete(&note.id)?;
                n += 1;
            }
        }
        Ok(n)
    }

    /// Permanently deletes trashed notes that have been in the trash longer than
    /// `retention_days`. A note is eligible when `now - trashedAt` exceeds the
    /// retention window. `retention_days == 0` means "keep forever" — the sweep
    /// is a no-op, so a user who never opts in never loses a trashed note.
    /// Returns how many notes were swept.
    pub fn sweep_trash(&self, retention_days: u32) -> Result<usize> {
        if retention_days == 0 {
            return Ok(0);
        }
        let cutoff = crate::model::now_millis() - (retention_days as i64) * 86_400_000;
        let mut n = 0;
        for note in self.load_all()? {
            if let Some(trashed_at) = note.trashed_at {
                if trashed_at < cutoff {
                    self.delete(&note.id)?;
                    n += 1;
                }
            }
        }
        Ok(n)
    }

    /// Removes a label from every note that carries it. Returns how many notes
    /// changed. (Tags aren't a separate object — they live on notes — so
    /// "deleting a tag" means stripping it everywhere.)
    pub fn delete_tag(&self, tag: &str) -> Result<usize> {
        let mut changed = 0;
        for mut note in self.load_all()? {
            if let Some(labels) = note.labels.as_mut() {
                let before = labels.len();
                labels.retain(|l| l != tag);
                if labels.len() != before {
                    if labels.is_empty() {
                        note.labels = None;
                    }
                    note.touch();
                    self.save(&note)?;
                    changed += 1;
                }
            }
        }
        Ok(changed)
    }

    /// Folder names, derived from the subdirectories that actually exist.
    pub fn folders(&self) -> Result<Vec<String>> {
        use std::collections::{BTreeMap, BTreeSet};

        let metadata = self.load_folder_meta().unwrap_or_else(|e| {
            eprintln!("could not read folder metadata: {e}");
            Vec::new()
        });
        let mut by_key: BTreeMap<String, FolderMeta> = BTreeMap::new();
        for folder in metadata {
            if folder.name.trim().is_empty() {
                continue;
            }
            let key = folder.name.to_lowercase();
            let replace = by_key
                .get(&key)
                .map_or(true, |current| folder.updated_at > current.updated_at);
            if replace {
                by_key.insert(key, folder);
            }
        }

        let mut out: BTreeSet<String> = by_key
            .values()
            .filter(|folder| !folder.deleted)
            .map(|folder| folder.name.clone())
            .collect();
        for entry in fs::read_dir(self.root.join("notes"))? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let name = entry.file_name().to_string_lossy().into_owned();
                // A newer tombstone beats a leftover empty directory. A
                // directory unknown to metadata is a legacy/discovered folder.
                if !by_key.get(&name.to_lowercase()).is_some_and(|m| m.deleted) {
                    out.insert(name);
                }
            }
        }
        Ok(out.into_iter().collect())
    }

    fn folder_meta_path(&self) -> PathBuf {
        self.root.join("notes").join(FOLDERS_FILE)
    }

    fn load_folder_meta(&self) -> Result<Vec<FolderMeta>> {
        let path = self.folder_meta_path();
        let Ok(raw) = fs::read_to_string(&path) else {
            return Ok(Vec::new());
        };
        let values: Vec<serde_json::Value> =
            serde_json::from_str(&raw).with_context(|| format!("reading {}", path.display()))?;
        let mut folders = Vec::new();
        for value in values {
            match value {
                // Backward-compatible upstream format: ["Work", "Home"].
                serde_json::Value::String(name) if !name.trim().is_empty() => {
                    folders.push(FolderMeta {
                        name,
                        color: None,
                        updated_at: 0,
                        deleted: false,
                        extra: Default::default(),
                    })
                }
                serde_json::Value::Object(_) => {
                    if let Ok(folder) = serde_json::from_value::<FolderMeta>(value) {
                        if !folder.name.trim().is_empty() {
                            folders.push(folder);
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(folders)
    }

    fn set_folder_deleted(&self, name: &str, deleted: bool) -> Result<()> {
        let mut folders = self.load_folder_meta()?;
        let key = name.to_lowercase();
        if let Some(folder) = folders
            .iter_mut()
            .find(|folder| folder.name.to_lowercase() == key)
        {
            folder.name = name.to_string();
            folder.deleted = deleted;
            folder.updated_at = crate::model::now_millis();
        } else {
            folders.push(FolderMeta {
                name: name.to_string(),
                color: None,
                updated_at: crate::model::now_millis(),
                deleted,
                extra: Default::default(),
            });
        }
        folders.sort_by_key(|folder| folder.name.to_lowercase());
        let path = self.folder_meta_path();
        write_atomic(&path, serde_json::to_string_pretty(&folders)?.as_bytes())?;
        self.write_stamp.touch();
        Ok(())
    }

    fn ensure_folder_active(&self, name: &str) -> Result<()> {
        let key = name.to_lowercase();
        let folders = match self.load_folder_meta() {
            Ok(folders) => folders,
            Err(e) => {
                // Folder metadata is auxiliary. A damaged ledger must never
                // prevent edits to the JSON master from being saved.
                eprintln!("could not update folder metadata: {e}");
                return Ok(());
            }
        };
        if folders
            .iter()
            .any(|folder| folder.name.to_lowercase() == key && !folder.deleted)
        {
            return Ok(());
        }
        self.set_folder_deleted(name, false)
    }
}

fn collect(
    dir: &Path,
    folder: Option<&str>,
    out: &mut Vec<Note>,
    failures: &mut Vec<(PathBuf, anyhow::Error)>,
) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            // Only one level deep: upstream maps folders to a single subdirectory.
            if folder.is_none() {
                let name = entry.file_name().to_string_lossy().into_owned();
                collect(&path, Some(&name), out, failures)?;
            }
            continue;
        }
        // Sync metadata lives in notes/ but neither file is a note.
        if path
            .file_name()
            .is_some_and(|n| n == DELETIONS_FILE || n == FOLDERS_FILE)
        {
            continue;
        }
        if path.extension().is_some_and(|e| e == "json") {
            match fs::read_to_string(&path)
                .map_err(anyhow::Error::from)
                .and_then(|s| serde_json::from_str::<Note>(&s).map_err(anyhow::Error::from))
            {
                Ok(mut note) => {
                    if note.folder_name.is_none() {
                        note.folder_name = folder.map(str::to_owned);
                    }
                    out.push(note);
                }
                Err(e) => failures.push((path, e)),
            }
        }
    }
    Ok(())
}

fn walk_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs = vec![root.to_path_buf()];
    if root.exists() {
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                dirs.push(entry.path());
            }
        }
    }
    Ok(dirs)
}

/// Write via a temp file + rename so a crash or a concurrent WebDAV read never
/// observes a half-written note.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn sanitize_component(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .collect::<String>()
        .trim()
        .to_string()
}

/// Stable per-install id, written next to the config. The Android app uses this
/// to attribute writes, so it must not change between launches.
pub fn device_id() -> String {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        let path = config_dir().join("device-id");
        if let Ok(existing) = fs::read_to_string(&path) {
            let trimmed = existing.trim().to_string();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
        let id = format!("desktop-{}", crate::model::new_uuid());
        let _ = fs::create_dir_all(config_dir());
        let _ = fs::write(&path, &id);
        id
    })
    .clone()
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Notes")
}

pub fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Notes")
}

/// Directory names used before the app was renamed to "Notes". Kept only so
/// existing installs migrate their data instead of starting empty.
const LEGACY_DIR: &str = "simple-notes-desktop";

/// One-time move of the pre-rename config and data folders to their new "Notes"
/// locations. Runs at startup, before anything reads config. A folder is only
/// moved when the new path doesn't exist yet, so it never clobbers newer data.
/// Returns whether the data folder was migrated (the caller then repoints the
/// stored `data_dir`).
pub fn migrate_legacy_dirs() -> bool {
    if let Some(base) = dirs::config_dir() {
        let (old, new) = (base.join(LEGACY_DIR), base.join("Notes"));
        if old.is_dir() && !new.exists() {
            if let Err(e) = fs::rename(&old, &new) {
                eprintln!("could not migrate config folder: {e}");
            }
        }
    }
    let mut data_moved = false;
    if let Some(base) = dirs::data_dir() {
        let (old, new) = (base.join(LEGACY_DIR), base.join("Notes"));
        if old.is_dir() && !new.exists() {
            match fs::rename(&old, &new) {
                Ok(()) => data_moved = true,
                Err(e) => eprintln!("could not migrate notes folder: {e}"),
            }
        }
    }
    data_moved
}

/// The pre-rename default notes directory, used to detect a stored `data_dir`
/// that still points at the old location after migration.
pub fn legacy_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(LEGACY_DIR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Note;

    fn temp_store() -> Store {
        let dir = std::env::temp_dir().join(format!("sn-test-{}", crate::model::new_uuid()));
        Store::new(dir).unwrap()
    }

    #[test]
    fn save_and_reload_round_trips() {
        let store = temp_store();
        let note = Note {
            title: "Hello".into(),
            content: "world".into(),
            ..Default::default()
        };
        store.save(&note).unwrap();

        let loaded = store.load_all().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].title, "Hello");
        assert!(store.root().join("notes-md/Hello.md").exists());
    }

    #[test]
    fn greek_tonos_round_trips_in_title_and_body() {
        let store = temp_store();
        let note = Note {
            title: "ά έ ή ί ό ύ ώ".into(),
            content: "Καλημέρα· μία σημείωση με τόνο.".into(),
            ..Default::default()
        };
        store.save(&note).unwrap();

        let loaded = store.load_all().unwrap();
        assert_eq!(loaded[0].title, note.title);
        assert_eq!(loaded[0].content, note.content);
    }

    #[test]
    fn folder_notes_land_in_a_subdirectory() {
        let store = temp_store();
        let note = Note {
            title: "Standup".into(),
            folder_name: Some("Work".into()),
            ..Default::default()
        };
        store.save(&note).unwrap();

        assert!(store
            .root()
            .join("notes/Work")
            .join(format!("{}.json", note.id))
            .exists());
        assert_eq!(
            store.load_all().unwrap()[0].folder_name.as_deref(),
            Some("Work")
        );
        assert_eq!(store.folders().unwrap(), vec!["Work".to_string()]);
    }

    #[test]
    fn moving_a_note_between_folders_leaves_no_duplicate() {
        let store = temp_store();
        let mut note = Note {
            title: "Move me".into(),
            ..Default::default()
        };
        store.save(&note).unwrap();
        note.folder_name = Some("Archive".into());
        store.save(&note).unwrap();

        assert_eq!(store.load_all().unwrap().len(), 1);
    }

    #[test]
    fn interrupted_remote_move_deduplicates_by_id_and_keeps_newest() {
        let store = temp_store();
        let old = Note {
            id: "same".into(),
            title: "Old".into(),
            updated_at: 100,
            ..Default::default()
        };
        let new = Note {
            id: "same".into(),
            title: "New".into(),
            updated_at: 200,
            folder_name: Some("Work".into()),
            ..Default::default()
        };
        fs::write(
            store.root().join("notes/same.json"),
            serde_json::to_vec(&old).unwrap(),
        )
        .unwrap();
        fs::create_dir_all(store.root().join("notes/Work")).unwrap();
        fs::write(
            store.root().join("notes/Work/same.json"),
            serde_json::to_vec(&new).unwrap(),
        )
        .unwrap();

        let loaded = store.load_all().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].title, "New");
        assert_eq!(loaded[0].folder_name.as_deref(), Some("Work"));
    }

    #[test]
    fn failed_move_keeps_the_last_good_copy() {
        let store = temp_store();
        let mut note = Note {
            title: "Keep me".into(),
            content: "safe".into(),
            ..Default::default()
        };
        store.save(&note).unwrap();

        // Linux rejects a single path component this long. Saving the move must
        // fail without deleting the valid top-level copy first.
        note.folder_name = Some("x".repeat(300));
        assert!(store.save(&note).is_err());

        let loaded = store.load_all().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].content, "safe");
        assert_eq!(loaded[0].folder_name, None);
    }

    #[test]
    fn deleting_one_id_does_not_match_a_longer_frontmatter_id() {
        let store = temp_store();
        let short = Note {
            id: "abc".into(),
            title: "Short".into(),
            ..Default::default()
        };
        let longer = Note {
            id: "abcdef".into(),
            title: "Long".into(),
            ..Default::default()
        };
        store.save(&short).unwrap();
        store.save(&longer).unwrap();

        store.delete(&short.id).unwrap();
        assert!(store.root().join("notes-md/Long.md").exists());
        assert_eq!(store.load_all().unwrap().len(), 1);
        assert_eq!(store.load_all().unwrap()[0].id, longer.id);
    }

    /// Exactly the payload shape the Android app PUTs to `/notes/<uuid>.json`,
    /// including a field this build doesn't model.
    #[test]
    fn reads_a_note_uploaded_by_the_android_app() {
        let store = temp_store();
        let id = "11111111-2222-3333-4444-555555555555";
        fs::write(
            store.root().join("notes").join(format!("{id}.json")),
            r#"{"id":"11111111-2222-3333-4444-555555555555","title":"From the phone",
                "content":"typed on Android","createdAt":1767622222000,"updatedAt":1767622222000,
                "deviceId":"pixel-8","syncStatus":"SYNCED","noteType":"TEXT","isPinned":true,
                "someFutureField":{"nested":true}}"#,
        )
        .unwrap();

        let notes = store.load_all().unwrap();
        assert_eq!(notes.len(), 1);
        let note = &notes[0];
        assert_eq!(note.title, "From the phone");
        assert_eq!(note.device_id, "pixel-8");
        assert!(note.is_pinned);
        assert!(note.extra.contains_key("someFutureField"));

        // Re-saving from the desktop must not drop the unmodelled field.
        store.save(note).unwrap();
        let raw =
            fs::read_to_string(store.root().join("notes").join(format!("{id}.json"))).unwrap();
        assert!(raw.contains("someFutureField"));
    }

    #[test]
    fn upstream_metadata_files_are_not_loaded_as_notes() {
        let store = temp_store();
        fs::write(
            store.root().join("notes/folders.json"),
            r#"[{"name":"Work"}]"#,
        )
        .unwrap();
        fs::write(
            store.root().join("notes/deletions.json"),
            r#"{"version":1,"deletedNotes":[]}"#,
        )
        .unwrap();

        assert!(store.load_all().unwrap().is_empty());
    }

    #[test]
    fn delete_removes_json_and_markdown() {
        let store = temp_store();
        let note = Note {
            title: "Bye".into(),
            ..Default::default()
        };
        store.save(&note).unwrap();
        store.delete(&note.id).unwrap();

        assert!(store.load_all().unwrap().is_empty());
        assert!(!store.root().join("notes-md/Bye.md").exists());
    }
}

#[cfg(test)]
mod folder_tests {
    use super::*;

    fn temp_store() -> Store {
        Store::new(std::env::temp_dir().join(format!("sn-fold-{}", crate::model::new_uuid())))
            .unwrap()
    }

    #[test]
    fn creates_a_folder_in_both_trees() {
        let store = temp_store();
        assert_eq!(store.create_folder("Work").unwrap(), "Work");
        assert!(store.root().join("notes/Work").is_dir());
        assert!(store.root().join("notes-md/Work").is_dir());
        assert_eq!(store.folders().unwrap(), vec!["Work".to_string()]);
        let metadata: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(store.root().join("notes/folders.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(metadata[0]["name"], "Work");
        assert_eq!(metadata[0]["deleted"], false);
        assert!(metadata[0]["updatedAt"].as_i64().unwrap() > 0);
    }

    #[test]
    fn refuses_a_duplicate() {
        let store = temp_store();
        store.create_folder("Work").unwrap();
        assert!(store.create_folder("Work").is_err());
    }

    #[test]
    fn strips_path_hostile_characters() {
        let store = temp_store();
        // A name like this must not escape the notes directory.
        assert_eq!(store.create_folder("../../etc").unwrap(), "....etc");
        assert!(store.root().join("notes/....etc").is_dir());
    }

    #[test]
    fn refuses_a_name_that_sanitises_to_nothing() {
        let store = temp_store();
        assert!(store.create_folder("///").is_err());
        assert!(store.create_folder("   ").is_err());
    }

    #[test]
    fn emoji_folder_names_survive() {
        // The user's real vault is full of these.
        let store = temp_store();
        assert_eq!(store.create_folder("Σχολείο 🚸").unwrap(), "Σχολείο 🚸");
        assert_eq!(store.create_folder("κωδικοί 🔑").unwrap(), "κωδικοί 🔑");
    }

    #[test]
    fn metadata_only_folders_and_tombstones_control_visibility() {
        let store = temp_store();
        fs::write(
            store.root().join("notes/folders.json"),
            r#"[
                {"name":"Remote empty","updatedAt":100,"deleted":false},
                {"name":"Gone","updatedAt":200,"deleted":true}
            ]"#,
        )
        .unwrap();
        fs::create_dir_all(store.root().join("notes/Gone")).unwrap();

        assert_eq!(store.folders().unwrap(), vec!["Remote empty".to_string()]);
    }

    #[test]
    fn saving_an_imported_folder_note_registers_folder_metadata() {
        let store = temp_store();
        let note = Note {
            folder_name: Some("Imported".into()),
            ..Default::default()
        };
        store.save(&note).unwrap();

        let raw = fs::read_to_string(store.root().join("notes/folders.json")).unwrap();
        assert!(raw.contains("\"name\": \"Imported\""));
        assert_eq!(store.folders().unwrap(), vec!["Imported".to_string()]);
    }
}

#[cfg(test)]
mod delete_folder_tests {
    use super::*;
    use crate::model::Note;

    #[test]
    fn delete_folder_moves_notes_to_top_level() {
        let store =
            Store::new(std::env::temp_dir().join(format!("sn-df-{}", crate::model::new_uuid())))
                .unwrap();
        let a = Note {
            title: "a".into(),
            folder_name: Some("Work".into()),
            ..Default::default()
        };
        let b = Note {
            title: "b".into(),
            folder_name: Some("Work".into()),
            ..Default::default()
        };
        store.save(&a).unwrap();
        store.save(&b).unwrap();

        let moved = store.delete_folder("Work").unwrap();
        assert_eq!(moved, 2);
        assert!(
            !store.root().join("notes/Work").exists(),
            "folder dir should be gone"
        );
        // Both notes survive, now at top level.
        let all = store.load_all().unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.iter().all(|n| n.folder_name.is_none()));
    }

    #[test]
    fn delete_empty_folder_removes_it() {
        let store =
            Store::new(std::env::temp_dir().join(format!("sn-df2-{}", crate::model::new_uuid())))
                .unwrap();
        store.create_folder("Empty").unwrap();
        assert_eq!(store.delete_folder("Empty").unwrap(), 0);
        assert!(store.folders().unwrap().is_empty());
        let raw = fs::read_to_string(store.root().join("notes/folders.json")).unwrap();
        let metadata: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap();
        assert_eq!(metadata[0]["name"], "Empty");
        assert_eq!(metadata[0]["deleted"], true);
    }
}

#[cfg(test)]
mod delete_tag_tests {
    use super::*;
    use crate::model::Note;

    #[test]
    fn delete_tag_strips_it_from_all_notes() {
        let store =
            Store::new(std::env::temp_dir().join(format!("sn-dt-{}", crate::model::new_uuid())))
                .unwrap();
        let a = Note {
            title: "a".into(),
            labels: Some(vec!["work".into(), "urgent".into()]),
            ..Default::default()
        };
        let b = Note {
            title: "b".into(),
            labels: Some(vec!["work".into()]),
            ..Default::default()
        };
        let c = Note {
            title: "c".into(),
            labels: Some(vec!["home".into()]),
            ..Default::default()
        };
        for n in [&a, &b, &c] {
            store.save(n).unwrap();
        }

        let n = store.delete_tag("work").unwrap();
        assert_eq!(n, 2, "two notes had 'work'");
        let all = store.load_all().unwrap();
        assert!(all
            .iter()
            .all(|note| !note.labels.iter().flatten().any(|l| l == "work")));
        // A note left with no labels becomes None, not an empty vec.
        let b_after = all.iter().find(|n| n.title == "b").unwrap();
        assert_eq!(b_after.labels, None);
        // Unrelated tag untouched.
        assert!(all
            .iter()
            .find(|n| n.title == "c")
            .unwrap()
            .labels
            .as_ref()
            .unwrap()
            .contains(&"home".to_string()));
    }
}

#[cfg(test)]
mod prune_and_trash_tests {
    use super::*;
    use crate::model::Note;

    #[test]
    fn prune_removes_only_empty_folders() {
        let store =
            Store::new(std::env::temp_dir().join(format!("sn-pr-{}", crate::model::new_uuid())))
                .unwrap();
        store.create_folder("Work").unwrap();
        // Empty → pruned.
        assert!(store.prune_folder_if_empty("Work").unwrap());
        assert!(store.folders().unwrap().is_empty());

        // Non-empty → kept.
        let n = Note {
            title: "x".into(),
            folder_name: Some("Keep".into()),
            ..Default::default()
        };
        store.save(&n).unwrap();
        assert!(!store.prune_folder_if_empty("Keep").unwrap());
        assert_eq!(store.folders().unwrap(), vec!["Keep".to_string()]);
    }

    #[test]
    fn empty_trash_deletes_only_trashed() {
        let store =
            Store::new(std::env::temp_dir().join(format!("sn-et-{}", crate::model::new_uuid())))
                .unwrap();
        let live = Note {
            title: "live".into(),
            ..Default::default()
        };
        let trashed = Note {
            title: "gone".into(),
            trashed_at: Some(crate::model::now_millis()),
            ..Default::default()
        };
        store.save(&live).unwrap();
        store.save(&trashed).unwrap();

        assert_eq!(store.empty_trash().unwrap(), 1);
        let all = store.load_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].title, "live");
    }

    #[test]
    fn sweep_trash_respects_the_retention_window() {
        let store =
            Store::new(std::env::temp_dir().join(format!("sn-sw-{}", crate::model::new_uuid())))
                .unwrap();
        let day = 86_400_000i64;
        let now = crate::model::now_millis();
        let live = Note {
            title: "live".into(),
            ..Default::default()
        };
        let recent = Note {
            title: "recent".into(),
            trashed_at: Some(now - 2 * day),
            ..Default::default()
        };
        let old = Note {
            title: "old".into(),
            trashed_at: Some(now - 40 * day),
            ..Default::default()
        };
        store.save(&live).unwrap();
        store.save(&recent).unwrap();
        store.save(&old).unwrap();

        // 30-day window: only the 40-day-old note is swept. The recent trash and
        // the live note stay.
        assert_eq!(store.sweep_trash(30).unwrap(), 1);
        let mut titles: Vec<_> = store
            .load_all()
            .unwrap()
            .into_iter()
            .map(|n| n.title)
            .collect();
        titles.sort();
        assert_eq!(titles, vec!["live".to_string(), "recent".to_string()]);
    }

    #[test]
    fn delete_writes_a_tombstone_the_phone_can_read() {
        let store =
            Store::new(std::env::temp_dir().join(format!("sn-tomb-{}", crate::model::new_uuid())))
                .unwrap();
        let note = Note {
            title: "bye".into(),
            ..Default::default()
        };
        let id = note.id.clone();
        store.save(&note).unwrap();
        store.delete(&id).unwrap();

        let ledger =
            std::fs::read_to_string(store.root().join("notes").join("deletions.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&ledger).unwrap();
        assert_eq!(v["version"], 1);
        let recs = v["deletedNotes"].as_array().unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0]["id"], id);
        assert!(recs[0]["deletedAt"].as_i64().unwrap() > 0);
        assert!(recs[0]["deviceId"].as_str().unwrap().len() > 0);

        // The ledger must not be loaded as a note, and deleting the same id twice
        // must not duplicate the tombstone.
        assert!(store.load_all().unwrap().is_empty());
        store.record_deletion(&id).unwrap();
        let v2: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(store.root().join("notes").join("deletions.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(v2["deletedNotes"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn sweep_trash_zero_days_keeps_everything() {
        let store =
            Store::new(std::env::temp_dir().join(format!("sn-sw0-{}", crate::model::new_uuid())))
                .unwrap();
        let ancient = Note {
            title: "ancient".into(),
            trashed_at: Some(0),
            ..Default::default()
        };
        store.save(&ancient).unwrap();
        // 0 = keep forever: even a note trashed at the epoch survives.
        assert_eq!(store.sweep_trash(0).unwrap(), 0);
        assert_eq!(store.load_all().unwrap().len(), 1);
    }
}
