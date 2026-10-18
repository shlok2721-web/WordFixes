//! Tags → folders library conversion.
//!
//! The Android app has folders but no first-class tags, so a tag-organised
//! desktop library is invisible structure on the phone. This one-time,
//! user-confirmed operation gives every tag a folder and moves each tagged
//! unfiled note into the folder of its *primary* tag. Labels are left untouched in
//! the JSON, so nothing is lost and the operation can be reasoned about —
//! or undone by hand — later.
//!
//! Rules (documented in PLAN.md):
//! - The primary tag is the alphabetically first label (Unicode lowercase),
//!   so the outcome is deterministic for multi-tag notes.
//! - Notes already in any folder are left untouched. A folder chosen by the
//!   user is authoritative; conversion only organises still-unfiled notes.
//! - Affected notes are unarchived, so they actually show up inside their
//!   folder in Simple Notes Sync.
//! - Trashed notes are ignored.
//! - Planning is pure and repeat-safe: running the conversion twice plans
//!   zero work the second time.

use crate::model::Note;

/// What the conversion would do — computed without writing anything, shown
/// to the user, and only then applied.
#[derive(Debug, Default, PartialEq)]
pub struct ConversionPlan {
    /// Folders that do not exist yet, one per tag in use.
    pub create_folders: Vec<String>,
    /// `(note id, folder)` for every note not already in its primary folder.
    pub moves: Vec<(String, String)>,
    /// Ids of archived tagged notes that will have `archivedAt` cleared.
    pub unarchive: Vec<String>,
    /// How many affected notes carry more than one tag (lossy choice).
    pub multi_tag: usize,
}

impl ConversionPlan {
    pub fn is_empty(&self) -> bool {
        self.create_folders.is_empty() && self.moves.is_empty() && self.unarchive.is_empty()
    }
}

/// Folder names come from tag names, which may contain path separators the
/// store cannot accept. Same allow-list idea as `markdown_filename`.
pub fn folder_name_for_tag(tag: &str) -> String {
    let cleaned: String = tag
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "Tagged".to_string()
    } else {
        trimmed.to_string()
    }
}

/// The alphabetically first label, Unicode-lowercase, ignoring empty labels.
fn primary_tag(note: &Note) -> Option<&str> {
    note.labels
        .as_ref()?
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .min_by_key(|l| l.to_lowercase())
}

pub fn plan(notes: &[Note], existing_folders: &[String]) -> ConversionPlan {
    let mut plan = ConversionPlan::default();
    let mut wanted_folders: Vec<String> = Vec::new();

    for note in notes
        .iter()
        .filter(|n| !n.is_trashed() && n.folder_name.as_deref().is_none_or(str::is_empty))
    {
        // Every tag on an unfiled note gets a folder, not only primary ones.
        // Tags on already-filed notes are deliberately ignored: their existing
        // folder is a user decision, not unfinished conversion work.
        for label in note.labels.iter().flatten() {
            let name = folder_name_for_tag(label);
            if label.trim().is_empty() {
                continue;
            }
            if !wanted_folders.contains(&name) {
                wanted_folders.push(name);
            }
        }

        let Some(primary) = primary_tag(note) else {
            continue;
        };
        let folder = folder_name_for_tag(primary);

        let tag_count = note
            .labels
            .iter()
            .flatten()
            .filter(|l| !l.trim().is_empty())
            .count();
        let needs_unarchive = note.is_archived();
        plan.moves.push((note.id.clone(), folder.clone()));
        if needs_unarchive {
            plan.unarchive.push(note.id.clone());
        }
        if tag_count > 1 {
            plan.multi_tag += 1;
        }
    }

    plan.create_folders = wanted_folders
        .into_iter()
        .filter(|f| !existing_folders.contains(f))
        .collect();
    plan.create_folders.sort();
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::now_millis;

    fn tagged(title: &str, labels: &[&str]) -> Note {
        Note {
            title: title.into(),
            labels: Some(labels.iter().map(|s| s.to_string()).collect()),
            ..Default::default()
        }
    }

    #[test]
    fn primary_tag_is_alphabetical_and_unicode_aware() {
        let n = tagged("x", &["Ωμέγα", "αλφα"]);
        let p = plan(&[n], &[]);
        assert_eq!(p.moves[0].1, "αλφα");
        assert_eq!(p.multi_tag, 1);
    }

    #[test]
    fn every_tag_gets_a_folder_but_only_missing_ones_are_created() {
        let a = tagged("a", &["Work", "Home"]);
        let b = tagged("b", &["Ideas"]);
        let p = plan(&[a, b], &["Ideas".to_string()]);
        assert_eq!(p.create_folders, ["Home", "Work"]);
    }

    #[test]
    fn archived_tagged_notes_are_unarchived_and_trashed_ignored() {
        let mut archived = tagged("a", &["Work"]);
        archived.archived_at = Some(now_millis());
        let mut trashed = tagged("t", &["Work"]);
        trashed.trashed_at = Some(now_millis());
        let p = plan(&[archived.clone(), trashed], &[]);
        assert_eq!(p.unarchive, [archived.id.clone()]);
        assert_eq!(p.moves.len(), 1, "trashed note must not be moved");
    }

    #[test]
    fn a_second_run_plans_nothing() {
        let mut done = tagged("a", &["Work"]);
        done.folder_name = Some("Work".into());
        let p = plan(&[done], &["Work".to_string()]);
        assert!(p.is_empty(), "conversion must be idempotent: {p:?}");
    }

    #[test]
    fn an_already_foldered_note_is_never_moved_again() {
        let mut manually_filed = tagged("a", &["Work", "Home"]);
        manually_filed.folder_name = Some("My choice".into());
        manually_filed.archived_at = Some(now_millis());

        let p = plan(&[manually_filed], &["My choice".to_string()]);

        assert!(
            p.is_empty(),
            "an existing folder is authoritative, even when it differs from the first tag: {p:?}"
        );
    }

    #[test]
    fn hostile_tag_names_become_safe_folder_names() {
        assert_eq!(folder_name_for_tag("a/b:c"), "a-b-c");
        assert_eq!(folder_name_for_tag("  "), "Tagged");
    }

    #[test]
    fn untagged_notes_are_left_alone() {
        let plain = Note {
            title: "plain".into(),
            ..Default::default()
        };
        assert!(plan(&[plain], &[]).is_empty());
    }
}
