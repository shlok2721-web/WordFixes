//! Google Keep Takeout importer.
//!
//! Reads a Takeout `.zip` directly — the same path other tools take: export
//! from Keep, hand over the archive, done. No intermediate Obsidian step.
//!
//! Each note is one `Takeout/Keep/<name>.json`:
//!
//! ```json
//! {
//!   "color": "DEFAULT", "isTrashed": false, "isPinned": false,
//!   "isArchived": true, "title": "…", "textContent": "…",
//!   "createdTimestampUsec": 1700000000000000,
//!   "userEditedTimestampUsec": 1700000000000000,
//!   "labels": [{"name": "…"}],
//!   "listContent": [{"text": "…", "isChecked": false}],
//!   "attachments": [{"filePath": "…", "mimetype": "image/png"}]
//! }
//! ```

use crate::import_md::ImportReport;
use crate::model::{ChecklistItem, Note, NoteType};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Namespace for ids derived from the Takeout entry name. Distinct from the
/// Markdown importer's, so importing the same notes by both routes doesn't
/// silently collide.
const NAMESPACE: uuid::Uuid = uuid::Uuid::from_bytes([
    0x6b, 0xa7, 0xb8, 0x12, 0x9d, 0xad, 0x11, 0xd1, 0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30, 0xc8,
]);

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct KeepNote {
    title: String,
    #[serde(rename = "textContent")]
    text_content: String,
    #[serde(rename = "createdTimestampUsec")]
    created_usec: i64,
    #[serde(rename = "userEditedTimestampUsec")]
    edited_usec: i64,
    #[serde(rename = "isPinned")]
    is_pinned: bool,
    #[serde(rename = "isArchived")]
    is_archived: bool,
    #[serde(rename = "isTrashed")]
    is_trashed: bool,
    color: String,
    labels: Vec<KeepLabel>,
    #[serde(rename = "listContent")]
    list_content: Vec<KeepListItem>,
    attachments: Vec<KeepAttachment>,
    annotations: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct KeepLabel {
    name: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct KeepListItem {
    text: String,
    #[serde(rename = "isChecked")]
    is_checked: bool,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct KeepAttachment {
    #[serde(rename = "filePath")]
    file_path: String,
    mimetype: String,
}

/// Reads a Takeout archive without writing anything, so an import can be
/// previewed before it touches the store.
pub fn scan_zip(path: &Path) -> Result<ImportReport> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("reading zip")?;

    let mut report = ImportReport::default();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name().to_string();

        if !entry.is_file() || !name.ends_with(".json") {
            continue;
        }
        // Takeout can bundle several Google products; only Keep is ours.
        if !name.contains("/Keep/") {
            continue;
        }

        let mut text = String::new();
        if let Err(e) = entry.read_to_string(&mut text) {
            report.skipped.push((PathBuf::from(&name), e.to_string()));
            continue;
        }
        match serde_json::from_str::<KeepNote>(&text) {
            Ok(keep) => report.imported.push(convert(keep, &name)),
            Err(e) => report.skipped.push((PathBuf::from(&name), e.to_string())),
        }
    }

    report
        .imported
        .sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(report)
}

/// Number of attachments referenced by a report's notes. Assets are not
/// imported, so the caller should warn when this is non-zero.
pub fn attachment_count(report: &ImportReport) -> usize {
    report
        .imported
        .iter()
        .filter_map(|n| n.extra.get("keepAttachments"))
        .filter_map(|v| v.as_array())
        .map(|a| a.len())
        .sum()
}

fn convert(keep: KeepNote, entry_name: &str) -> Note {
    let created = usec_to_millis(keep.created_usec);
    let updated = usec_to_millis(keep.edited_usec).max(created);

    let labels: Vec<String> = keep
        .labels
        .into_iter()
        .map(|l| l.name)
        .filter(|n| !n.is_empty())
        .collect();

    let mut note = Note {
        id: uuid::Uuid::new_v5(&NAMESPACE, entry_name.as_bytes()).to_string(),
        title: keep.title,
        created_at: created,
        updated_at: updated,
        device_id: crate::store::device_id(),
        is_pinned: keep.is_pinned,
        archived_at: keep.is_archived.then_some(updated),
        trashed_at: keep.is_trashed.then_some(updated),
        color: color_to_hex(&keep.color),
        labels: (!labels.is_empty()).then_some(labels),
        // Keep has no folders; labels carry the organisation.
        folder_name: None,
        ..Default::default()
    };

    if !keep.list_content.is_empty() {
        note.note_type = NoteType::CHECKLIST;
        note.checklist_items = Some(
            keep.list_content
                .into_iter()
                .enumerate()
                .map(|(index, item)| ChecklistItem {
                    id: crate::model::new_uuid(),
                    text: item.text,
                    is_checked: item.is_checked,
                    order: index as i32,
                    original_order: index as i32,
                    created_at: created,
                    indentation_level: 0,
                })
                .collect(),
        );
    } else {
        note.content = keep.text_content;
    }

    // Assets aren't synced yet and link previews have no home in the model.
    // Park both in `extra` rather than dropping them, so a later version can
    // pick them up without a re-import.
    if !keep.attachments.is_empty() {
        note.extra.insert(
            "keepAttachments".into(),
            serde_json::json!(keep
                .attachments
                .iter()
                .map(|a| serde_json::json!({"filePath": a.file_path, "mimetype": a.mimetype}))
                .collect::<Vec<_>>()),
        );
    }
    if !keep.annotations.is_empty() {
        note.extra.insert(
            "keepAnnotations".into(),
            serde_json::json!(keep.annotations),
        );
    }

    note
}

/// Keep stores microseconds; the model stores milliseconds.
fn usec_to_millis(usec: i64) -> i64 {
    if usec <= 0 {
        crate::model::now_millis()
    } else {
        usec / 1000
    }
}

/// Keep's palette names → the `#RRGGBB` the model expects. `DEFAULT` means
/// "no colour", not white.
fn color_to_hex(name: &str) -> Option<String> {
    let hex = match name {
        "RED" => "#F28B82",
        "ORANGE" => "#FBBC04",
        "YELLOW" => "#FFF475",
        "GREEN" => "#CCFF90",
        "TEAL" => "#A7FFEB",
        "CERULEAN" => "#CBF0F8",
        "BLUE" => "#AECBFA",
        "DARKBLUE" => "#AECBFA",
        "PURPLE" => "#D7AEFB",
        "PINK" => "#FDCFE8",
        "BROWN" => "#E6C9A8",
        "GRAY" | "GREY" => "#E8EAED",
        _ => return None, // DEFAULT and anything unrecognised
    };
    Some(hex.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(json: &str, name: &str) -> Note {
        convert(serde_json::from_str(json).unwrap(), name)
    }

    #[test]
    fn converts_a_text_note() {
        let n = note(
            r#"{"color":"RED","isTrashed":false,"isPinned":true,"isArchived":false,
                "title":"Hello","textContent":"body text",
                "createdTimestampUsec":1700000000000000,
                "userEditedTimestampUsec":1700000001000000,
                "labels":[{"name":"work"},{"name":"urgent"}]}"#,
            "Takeout/Keep/Hello.json",
        );
        assert_eq!(n.title, "Hello");
        assert_eq!(n.content, "body text");
        assert!(n.is_pinned);
        assert!(!n.is_archived());
        assert_eq!(n.color.as_deref(), Some("#F28B82"));
        assert_eq!(
            n.labels.as_deref(),
            Some(&["work".into(), "urgent".into()][..])
        );
        // microseconds -> milliseconds
        assert_eq!(n.created_at, 1_700_000_000_000);
        assert_eq!(n.updated_at, 1_700_000_001_000);
    }

    #[test]
    fn converts_a_checklist() {
        let n = note(
            r#"{"title":"Shopping","listContent":[
                 {"text":"milk","isChecked":false},
                 {"text":"bread","isChecked":true}],
                "createdTimestampUsec":1700000000000000,
                "userEditedTimestampUsec":1700000000000000}"#,
            "Takeout/Keep/Shopping.json",
        );
        assert_eq!(n.note_type, NoteType::CHECKLIST);
        let items = n.checklist_items.unwrap();
        assert_eq!(items.len(), 2);
        assert!(!items[0].is_checked);
        assert!(items[1].is_checked);
        assert_eq!(items[1].order, 1);
        assert!(n.content.is_empty());
    }

    #[test]
    fn archived_and_trashed_flags_become_timestamps() {
        let n = note(
            r#"{"title":"Old","isArchived":true,"isTrashed":true,
                "createdTimestampUsec":1700000000000000,
                "userEditedTimestampUsec":1700000000000000}"#,
            "Takeout/Keep/Old.json",
        );
        assert!(n.is_archived());
        assert!(n.is_trashed());
    }

    #[test]
    fn default_colour_is_none_not_white() {
        assert_eq!(color_to_hex("DEFAULT"), None);
        assert_eq!(color_to_hex("SOMETHING_NEW"), None);
        assert_eq!(color_to_hex("TEAL").as_deref(), Some("#A7FFEB"));
    }

    #[test]
    fn ids_are_stable_per_entry() {
        let a = note(r#"{"title":"A"}"#, "Takeout/Keep/A.json");
        let b = note(r#"{"title":"A"}"#, "Takeout/Keep/A.json");
        let c = note(r#"{"title":"A"}"#, "Takeout/Keep/B.json");
        assert_eq!(a.id, b.id);
        assert_ne!(a.id, c.id);
    }

    #[test]
    fn attachments_and_annotations_are_parked_not_dropped() {
        let n = note(
            r#"{"title":"Pic","attachments":[{"filePath":"x.png","mimetype":"image/png"}],
                "annotations":[{"url":"https://example.com"}]}"#,
            "Takeout/Keep/Pic.json",
        );
        assert!(n.extra.contains_key("keepAttachments"));
        assert!(n.extra.contains_key("keepAnnotations"));
        // And they survive serialisation, since `extra` is flattened.
        let raw = serde_json::to_string(&n).unwrap();
        assert!(raw.contains("x.png"));
    }

    /// Dry run against a real Takeout archive. Read-only. Not run by default —
    /// it needs an archive this repo must never contain.
    ///
    /// ```bash
    /// KEEP_ZIP=/path/to/takeout.zip cargo test dry_run_real_takeout -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore]
    fn dry_run_real_takeout() {
        let Ok(path) = std::env::var("KEEP_ZIP") else {
            panic!("set KEEP_ZIP to a Takeout archive");
        };
        let report = scan_zip(Path::new(&path)).unwrap();

        let checklists = report
            .imported
            .iter()
            .filter(|n| n.note_type == NoteType::CHECKLIST)
            .count();
        let coloured = report.imported.iter().filter(|n| n.color.is_some()).count();
        let labelled = report
            .imported
            .iter()
            .filter(|n| n.labels.is_some())
            .count();
        let untitled = report
            .imported
            .iter()
            .filter(|n| n.title.trim().is_empty())
            .count();
        let bad_dates = report
            .imported
            .iter()
            .filter(|n| n.created_at <= 0 || n.updated_at < n.created_at)
            .count();

        println!("imported      {}", report.imported.len());
        println!("skipped       {}", report.skipped.len());
        println!("checklists    {checklists}");
        println!("coloured      {coloured}");
        println!("labelled      {labelled}");
        println!(
            "pinned        {}",
            report.imported.iter().filter(|n| n.is_pinned).count()
        );
        println!(
            "archived      {}",
            report.imported.iter().filter(|n| n.is_archived()).count()
        );
        println!(
            "trashed       {}",
            report.imported.iter().filter(|n| n.is_trashed()).count()
        );
        println!("untitled      {untitled}");
        println!("attachments   {}", attachment_count(&report));
        println!("bad dates     {bad_dates}");
        for (path, why) in &report.skipped {
            println!("SKIPPED {} — {why}", path.display());
        }

        assert!(report.skipped.is_empty(), "some notes failed to parse");
        assert_eq!(bad_dates, 0, "some notes have impossible timestamps");
    }

    #[test]
    fn a_missing_timestamp_falls_back_to_now() {
        let n = note(r#"{"title":"No dates"}"#, "Takeout/Keep/No.json");
        assert!(n.created_at > 0);
        assert!(n.updated_at >= n.created_at);
    }
}
