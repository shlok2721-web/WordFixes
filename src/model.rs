//! Note model, kept field-for-field compatible with the Android app's
//! `/notes/<uuid>.json` payload (see upstream `models/Note.kt`).
//!
//! Unknown fields are preserved on load and written back untouched so that a
//! desktop round-trip never drops data written by a newer Android version.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NoteType {
    #[default]
    TEXT,
    CHECKLIST,
}

/// Variants are SCREAMING_CASE to match the strings the Android app writes;
/// renaming them would break JSON compatibility.
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SyncStatus {
    #[default]
    LOCAL_ONLY,
    SYNCED,
    PENDING,
    CONFLICT,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChecklistItem {
    #[serde(default = "new_uuid")]
    pub id: String,
    #[serde(default)]
    pub text: String,
    #[serde(default, rename = "isChecked")]
    pub is_checked: bool,
    #[serde(default)]
    pub order: i32,
    #[serde(default, rename = "originalOrder")]
    pub original_order: i32,
    #[serde(default = "now_millis", rename = "createdAt")]
    pub created_at: i64,
    #[serde(default, rename = "indentationLevel")]
    pub indentation_level: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    #[serde(default = "new_uuid")]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub content: String,
    #[serde(default = "now_millis", rename = "createdAt")]
    pub created_at: i64,
    #[serde(default = "now_millis", rename = "updatedAt")]
    pub updated_at: i64,
    #[serde(default, rename = "deviceId")]
    pub device_id: String,
    #[serde(default, rename = "syncStatus")]
    pub sync_status: SyncStatus,
    #[serde(default, rename = "noteType")]
    pub note_type: NoteType,
    #[serde(
        default,
        rename = "checklistItems",
        skip_serializing_if = "Option::is_none"
    )]
    pub checklist_items: Option<Vec<ChecklistItem>>,
    #[serde(
        default,
        rename = "checklistSortOption",
        skip_serializing_if = "Option::is_none"
    )]
    pub checklist_sort_option: Option<String>,
    #[serde(
        default,
        rename = "importedAt",
        skip_serializing_if = "Option::is_none"
    )]
    pub imported_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, rename = "isPinned")]
    pub is_pinned: bool,
    #[serde(
        default,
        rename = "folderName",
        skip_serializing_if = "Option::is_none"
    )]
    pub folder_name: Option<String>,
    #[serde(default, rename = "trashedAt", skip_serializing_if = "Option::is_none")]
    pub trashed_at: Option<i64>,
    #[serde(
        default,
        rename = "archivedAt",
        skip_serializing_if = "Option::is_none"
    )]
    pub archived_at: Option<i64>,

    /// Anything the Android app wrote that this build does not model yet.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for Note {
    fn default() -> Self {
        Self {
            id: new_uuid(),
            title: String::new(),
            content: String::new(),
            created_at: now_millis(),
            updated_at: now_millis(),
            device_id: crate::store::device_id(),
            sync_status: SyncStatus::LOCAL_ONLY,
            note_type: NoteType::TEXT,
            checklist_items: None,
            checklist_sort_option: None,
            imported_at: None,
            labels: None,
            color: None,
            is_pinned: false,
            folder_name: None,
            trashed_at: None,
            archived_at: None,
            extra: BTreeMap::new(),
        }
    }
}

/// One-time checklist re-sort, mirroring the Android app's
/// `sortChecklistItems`: the chosen order is written back into `order` AND
/// `originalOrder` (indices 0..n), so the result becomes the new manual
/// baseline that both apps display. `option` takes the upstream enum names
/// ("MANUAL", "ALPHABETICAL_ASC", "ALPHABETICAL_DESC", "UNCHECKED_FIRST",
/// "CHECKED_FIRST", "CREATION_DATE", "CREATION_DATE_DESC"); unknown strings
/// fall back to MANUAL. All sorts are stable over the current display order.
pub fn sort_checklist(items: &mut [ChecklistItem], option: &str) {
    items.sort_by_key(|i| i.order);
    match option {
        "ALPHABETICAL_ASC" => items.sort_by_key(|i| i.text.to_lowercase()),
        "ALPHABETICAL_DESC" => {
            items.sort_by_key(|i| std::cmp::Reverse(i.text.to_lowercase()))
        }
        "CHECKED_FIRST" => items.sort_by_key(|i| !i.is_checked),
        // Upstream sorts each checked-partition by creation time.
        "CREATION_DATE" => items.sort_by_key(|i| (i.is_checked, i.created_at)),
        "CREATION_DATE_DESC" => {
            items.sort_by_key(|i| (i.is_checked, std::cmp::Reverse(i.created_at)))
        }
        // MANUAL and UNCHECKED_FIRST both mean "unchecked first, keep order".
        _ => items.sort_by_key(|i| i.is_checked),
    }
    for (index, item) in items.iter_mut().enumerate() {
        item.order = index as i32;
        item.original_order = index as i32;
    }
}

impl Note {
    pub fn is_trashed(&self) -> bool {
        self.trashed_at.is_some()
    }

    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }

    pub fn touch(&mut self) {
        self.updated_at = now_millis();
        self.sync_status = SyncStatus::PENDING;
    }

    /// Same as [`touch`], usable on a value you already own by move.
    pub fn touch_now(&mut self) {
        self.touch();
    }

    /// Preview shown under the title in the list.
    ///
    /// Checklists keep one item per line with real checkbox glyphs, the way
    /// the mobile cards show them — flattening them into `[ ] a · [ ] b` was
    /// unreadable at list width.
    pub fn preview(&self) -> String {
        match (&self.note_type, &self.checklist_items) {
            (NoteType::CHECKLIST, Some(items)) => {
                let mut sorted = items.clone();
                sorted.sort_by_key(|i| i.order);
                let shown: Vec<String> = sorted
                    .iter()
                    .take(3)
                    .map(|i| format!("{} {}", if i.is_checked { "☑" } else { "☐" }, i.text.trim()))
                    .collect();
                let mut out = shown.join("\n");
                if sorted.len() > 3 {
                    out.push_str(&format!("\n+{} more", sorted.len() - 3));
                }
                out
            }
            _ => {
                // Collapse runs of blank lines but keep real line breaks, so a
                // note's shape survives into the preview.
                let lines: Vec<&str> = self
                    .content
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .take(3)
                    .collect();
                let joined = lines.join("\n");
                if joined.chars().count() > 160 {
                    format!("{}…", joined.chars().take(160).collect::<String>())
                } else {
                    joined
                }
            }
        }
    }

    /// Markdown mirror written to `/notes-md/`.
    ///
    /// Byte-compatible with upstream `Note.toMarkdown()` (`Note.kt:126`).
    /// Optional keys are emitted only when set, matching upstream — and
    /// `archived` is an **epoch-ms number**, not a boolean. `docs/DESKTOP.md`
    /// documents an older, lossier shape (`tags: []`); the code is the truth.
    pub fn to_markdown(&self) -> String {
        let mut header = format!(
            "---\nid: {}\ncreated: {}\nupdated: {}\ndevice: {}\ntype: {}",
            self.id,
            iso8601(self.created_at),
            iso8601(self.updated_at),
            self.device_id,
            match self.note_type {
                NoteType::TEXT => "text",
                NoteType::CHECKLIST => "checklist",
            }
        );

        if self.note_type == NoteType::CHECKLIST {
            if let Some(sort) = &self.checklist_sort_option {
                header.push_str(&format!("\nsort: {sort}"));
            }
        }
        if let Some(imported) = self.imported_at {
            header.push_str(&format!("\nimported: {imported}"));
        }
        if let Some(labels) = self.labels.as_ref().filter(|l| !l.is_empty()) {
            header.push_str(&format!("\nlabels: [{}]", labels.join(", ")));
        }
        if let Some(color) = &self.color {
            header.push_str(&format!("\ncolor: \"{color}\""));
        }
        if self.is_pinned {
            header.push_str("\npinned: true");
        }
        if let Some(folder) = &self.folder_name {
            header.push_str(&format!("\nfolder: \"{folder}\""));
        }
        if let Some(archived) = self.archived_at {
            header.push_str(&format!("\narchived: {archived}"));
        }
        header.push_str(&format!("\n---\n\n# {}", self.title));

        let body = match (&self.note_type, &self.checklist_items) {
            (NoteType::CHECKLIST, Some(items)) => {
                let mut sorted = items.clone();
                sorted.sort_by_key(|i| i.order);
                sorted
                    .iter()
                    .map(|i| {
                        // A multiline item writes its extra lines as indented
                        // markdown list continuations, which the importer folds
                        // back into one item. Single-line items stay
                        // byte-compatible with upstream.
                        let text = i.text.replace('\n', "\n  ");
                        format!("- [{}] {}", if i.is_checked { "x" } else { " " }, text)
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            _ => self.content.clone(),
        };

        if body.trim().is_empty() {
            format!("{header}\n")
        } else {
            format!("{header}\n\n{body}")
        }
    }

    /// Filename used for the Markdown mirror.
    ///
    /// Keeps letters and digits of **any** script — the notes here are largely
    /// Greek, and the earlier `is_ascii()` rule collapsed 110 of 161 titles to
    /// empty, naming their mirrors `<uuid>.md` and defeating the whole point of
    /// a human-readable mirror. Emoji and symbols are dropped (not
    /// alphanumeric), as are the path-hostile characters `/ \ : * ? " < > |`.
    pub fn markdown_filename(&self) -> String {
        let cleaned: String = self
            .title
            .chars()
            .filter(|c| {
                // A small allow-list of punctuation, plus any alphanumeric in
                // any language. Everything else — emoji, symbols, the illegal
                // path set — is dropped.
                c.is_alphanumeric()
                    || c.is_whitespace()
                    || matches!(c, '-' | '_' | '.' | ',' | '(' | ')')
            })
            .collect();
        let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut name = collapsed.trim().replace(' ', "_");

        // Filesystems cap a name at 255 bytes, and Greek is 2 bytes/char, so a
        // note whose "title" is really a paragraph blew past it — the write
        // failed and the mirror silently went missing. Cap at 200 bytes on a
        // char boundary, leaving room for ".md".
        const MAX_BYTES: usize = 200;
        if name.len() > MAX_BYTES {
            let mut end = MAX_BYTES;
            while !name.is_char_boundary(end) {
                end -= 1;
            }
            name.truncate(end);
        }

        if name.is_empty() {
            format!("{}.md", self.id)
        } else {
            format!("{name}.md")
        }
    }
}

/// "37 min ago" / "1 day ago", matching how the mobile app labels notes.
pub fn relative_time(millis: i64) -> String {
    let secs = (now_millis() - millis).max(0) / 1000;
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => plural(secs / 60, "min"),
        3600..=86_399 => plural(secs / 3600, "hour"),
        86_400..=2_591_999 => plural(secs / 86_400, "day"),
        2_592_000..=31_535_999 => plural(secs / 2_592_000, "month"),
        _ => plural(secs / 31_536_000, "year"),
    }
}

fn plural(n: i64, unit: &str) -> String {
    if n == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{n} {unit}s ago")
    }
}

/// Perceived luminance, used to pick readable text on a coloured row.
/// Rec. 601 coefficients; the Keep palette is all pastel, so this lands on
/// dark text nearly always — but a user-chosen colour could be anything.
pub fn is_light_color(hex: &str) -> bool {
    let h = hex.trim_start_matches('#');
    if h.len() != 6 {
        return true;
    }
    let c = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(128) as f32;
    (0.299 * c(0) + 0.587 * c(2) + 0.114 * c(4)) > 140.0
}

pub fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Epoch millis -> `YYYY-MM-DDTHH:MM:SSZ`, without pulling in a date crate.
pub fn iso8601(millis: i64) -> String {
    let secs = millis.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);

    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mth = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if mth <= 2 { y + 1 } else { y };

    format!("{year:04}-{mth:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(text: &str, checked: bool, order: i32, created: i64) -> ChecklistItem {
        ChecklistItem {
            id: new_uuid(),
            text: text.into(),
            is_checked: checked,
            order,
            original_order: order,
            created_at: created,
            indentation_level: 0,
        }
    }
    fn texts(items: &[ChecklistItem]) -> Vec<&str> {
        items.iter().map(|i| i.text.as_str()).collect()
    }

    #[test]
    fn checklist_sort_alphabetical_interleaves_and_rebaselines_order() {
        let mut items = vec![
            item("βήτα", true, 0, 30),
            item("αλφα", false, 1, 10),
            item("Γάμμα", false, 2, 20),
        ];
        sort_checklist(&mut items, "ALPHABETICAL_ASC");
        assert_eq!(texts(&items), ["αλφα", "βήτα", "Γάμμα"]);
        // The sort becomes the new manual baseline, like on Android.
        assert_eq!(
            items.iter().map(|i| (i.order, i.original_order)).collect::<Vec<_>>(),
            [(0, 0), (1, 1), (2, 2)]
        );
    }

    #[test]
    fn checklist_sort_unchecked_first_is_stable_over_manual_order() {
        let mut items = vec![
            item("done-late", true, 3, 0),
            item("b", false, 2, 0),
            item("a", false, 1, 0),
            item("done-early", true, 0, 0),
        ];
        sort_checklist(&mut items, "UNCHECKED_FIRST");
        assert_eq!(texts(&items), ["a", "b", "done-early", "done-late"]);
        sort_checklist(&mut items, "CHECKED_FIRST");
        assert_eq!(texts(&items), ["done-early", "done-late", "a", "b"]);
    }

    #[test]
    fn checklist_sort_by_creation_keeps_checked_below() {
        let mut items = vec![
            item("new", false, 0, 200),
            item("old", false, 1, 100),
            item("done-new", true, 2, 400),
            item("done-old", true, 3, 300),
        ];
        sort_checklist(&mut items, "CREATION_DATE");
        assert_eq!(texts(&items), ["old", "new", "done-old", "done-new"]);
        sort_checklist(&mut items, "CREATION_DATE_DESC");
        assert_eq!(texts(&items), ["new", "old", "done-new", "done-old"]);
    }

    #[test]
    fn iso8601_matches_known_timestamps() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(1_767_622_222_000), "2026-01-05T14:10:22Z");
    }

    #[test]
    fn unknown_android_fields_survive_a_round_trip() {
        let raw = r#"{"id":"a","title":"T","content":"c","futureField":42}"#;
        let note: Note = serde_json::from_str(raw).unwrap();
        let out = serde_json::to_string(&note).unwrap();
        assert!(out.contains("\"futureField\":42"));
    }

    #[test]
    fn filename_sanitisation_matches_upstream_examples() {
        let mk = |t: &str| {
            Note {
                title: t.into(),
                ..Default::default()
            }
            .markdown_filename()
        };
        assert_eq!(mk("My Shopping List 🛒"), "My_Shopping_List.md");
        assert_eq!(mk("Meeting Notes 2026"), "Meeting_Notes_2026.md");
        assert_eq!(mk("To-Do: Project"), "To-Do_Project.md");
        // Greek must survive — the real-world case that was broken.
        assert_eq!(mk("Σχολείο 🚸"), "Σχολείο.md");
        assert_eq!(mk("Τράπεζα"), "Τράπεζα.md");
        // A paragraph-as-title must not produce a filename over the 255-byte
        // filesystem limit (Greek is 2 bytes/char).
        let long = mk(&"Σ".repeat(300));
        assert!(long.len() <= 204, "filename {} bytes", long.len());
    }
}
