//! Markdown vault importer.
//!
//! Handles two frontmatter dialects:
//!
//! * **Upstream mirror** — `id`, `created`, `updated`, `tags`, as written by
//!   the Android app into `/notes-md/`.
//! * **Obsidian / Google Keep export** — `title`, `created`, `updated`,
//!   `pinned`, `archived`, `tags`, `source`, with quoted values.
//!
//! Notes without an `id` get a deterministic UUIDv5 derived from their path in
//! the vault, so re-importing the same vault updates notes in place instead of
//! duplicating them.

use crate::model::{ChecklistItem, Note, NoteType};
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Namespace for path-derived ids. Arbitrary but fixed — changing it would
/// make every previously imported note re-import as a duplicate.
const NAMESPACE: uuid::Uuid = uuid::Uuid::from_bytes([
    0x6b, 0xa7, 0xb8, 0x11, 0x9d, 0xad, 0x11, 0xd1, 0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30, 0xc8,
]);

#[derive(Debug, Default, Clone)]
pub struct ImportReport {
    pub imported: Vec<Note>,
    pub skipped: Vec<(PathBuf, String)>,
}

/// How to organise imported notes. Keep exports carry labels but no folders;
/// Obsidian vaults carry folders but no labels — this lets the user pick one
/// scheme regardless of source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrganizeMode {
    /// Keep whatever the source had (Keep → labels, Obsidian → folders).
    AsIs,
    /// Fold each note's folder into a label, then clear the folder.
    Tags,
    /// Promote each note's first label to its folder, if it has no folder.
    Folders,
}

impl ImportReport {
    /// Rewrites every note's folder/label organisation per `mode`, in place.
    pub fn reorganize(&mut self, mode: OrganizeMode) {
        for note in &mut self.imported {
            match mode {
                OrganizeMode::AsIs => {}
                OrganizeMode::Tags => {
                    if let Some(folder) = note.folder_name.take() {
                        let labels = note.labels.get_or_insert_with(Vec::new);
                        // A nested Obsidian path becomes one tag per level. A
                        // single synthetic tag such as "School / Projects" is
                        // neither what the user selected nor useful for filtering.
                        for component in folder_components(&folder) {
                            if !contains_label(labels, &component) {
                                labels.push(component);
                            }
                        }
                    }
                }
                OrganizeMode::Folders => {
                    if note.folder_name.is_none() {
                        if let Some(first) = note.labels.as_ref().and_then(|l| l.first()).cloned() {
                            note.folder_name = Some(first);
                        }
                    }
                    // "Folders" is an exclusive organisation choice. Keeping
                    // every original tag as well made the result look arbitrary
                    // and indistinguishable from As-is.
                    note.labels = None;
                }
            }
        }
    }

    /// Commits an import while applying an explicit organisation choice even
    /// to notes imported before. Newer local text is preserved: only folder and
    /// labels are changed when the local note wins the timestamp comparison.
    pub fn commit_reorganized(
        &self,
        store: &crate::store::Store,
        existing: &[Note],
        force_organization: bool,
    ) -> (usize, usize, usize) {
        use std::collections::HashMap;
        let by_id: HashMap<&str, &Note> = existing
            .iter()
            .map(|note| (note.id.as_str(), note))
            .collect();

        let (mut added, mut updated, mut failed) = (0, 0, 0);
        for imported in &self.imported {
            match by_id.get(imported.id.as_str()).copied() {
                None => {
                    if store.save(imported).is_ok() {
                        added += 1;
                    } else {
                        failed += 1;
                    }
                }
                Some(local) if local.updated_at < imported.updated_at => {
                    if store.save(imported).is_ok() {
                        updated += 1;
                    } else {
                        failed += 1;
                    }
                }
                Some(local) if force_organization => {
                    let mut merged = local.clone();
                    if merged.folder_name == imported.folder_name
                        && merged.labels == imported.labels
                    {
                        continue;
                    }
                    merged.folder_name = imported.folder_name.clone();
                    merged.labels = imported.labels.clone();
                    merged.touch();
                    if store.save(&merged).is_ok() {
                        updated += 1;
                    } else {
                        failed += 1;
                    }
                }
                Some(_) => {}
            }
        }
        (added, updated, failed)
    }

    /// Writes every parsed note to the store, treating a note whose id already
    /// exists as an update (last-write-wins on `updated_at`, matching the sync
    /// rule). Returns `(added, updated, failed)`.
    ///
    /// Deterministic ids mean re-importing the same export is idempotent rather
    /// than duplicating — see the id derivation in each importer.
    pub fn commit(&self, store: &crate::store::Store, existing: &[Note]) -> (usize, usize, usize) {
        use std::collections::HashMap;
        let by_id: HashMap<&str, i64> = existing
            .iter()
            .map(|n| (n.id.as_str(), n.updated_at))
            .collect();

        let (mut added, mut updated, mut failed) = (0, 0, 0);
        for note in &self.imported {
            match by_id.get(note.id.as_str()) {
                // Existing but older on disk → overwrite. Existing and newer →
                // skip, so an import never clobbers a fresher local edit.
                Some(&on_disk) if on_disk >= note.updated_at => continue,
                Some(_) => {
                    if store.save(note).is_ok() {
                        updated += 1
                    } else {
                        failed += 1
                    }
                }
                None => {
                    if store.save(note).is_ok() {
                        added += 1
                    } else {
                        failed += 1
                    }
                }
            }
        }
        (added, updated, failed)
    }
}

/// Reads a vault without writing anything. The caller decides whether to
/// commit the result, so an import can be previewed first.
pub fn scan_vault(root: &Path) -> Result<ImportReport> {
    let mut report = ImportReport::default();
    walk(root, root, &mut report)?;
    report
        .imported
        .sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(report)
}

fn walk(root: &Path, dir: &Path, report: &mut ImportReport) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();

        if entry.file_type()?.is_dir() {
            // Obsidian's own state, deleted notes and the attachment store are
            // not notes.
            if name.starts_with('.') || name == "Attachments" {
                continue;
            }
            walk(root, &path, report)?;
            continue;
        }

        if !path.extension().is_some_and(|e| e == "md") {
            continue;
        }
        // A vault README is Obsidian scaffolding, not a note.
        if dir == root && name.eq_ignore_ascii_case("README.md") {
            continue;
        }

        match std::fs::read_to_string(&path)
            .map_err(anyhow::Error::from)
            .and_then(|text| {
                parse(&text, &path, root).ok_or_else(|| anyhow::anyhow!("no parsable content"))
            }) {
            Ok(note) => report.imported.push(note),
            Err(e) => report.skipped.push((path, e.to_string())),
        }
    }
    Ok(())
}

/// Parses one Markdown file into a [`Note`]. Files without frontmatter are
/// still imported, using the filename as the title.
pub fn parse(text: &str, path: &Path, root: &Path) -> Option<Note> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let (front, body) = split_frontmatter(&text);

    let rel = path.strip_prefix(root).unwrap_or(path);
    let stem = path.file_stem()?.to_string_lossy().into_owned();

    let title = front
        .iter()
        .find(|(k, _)| k == "title")
        .map(|(_, v)| v.clone())
        .filter(|t| !t.is_empty())
        .or_else(|| heading_title(body))
        .unwrap_or(stem);

    let created = front
        .iter()
        .find(|(k, _)| k == "created")
        .and_then(|(_, v)| parse_iso8601(v));
    let updated = front
        .iter()
        .find(|(k, _)| k == "updated")
        .and_then(|(_, v)| parse_iso8601(v));
    // Fall back to the file's own mtime rather than "now", so an import
    // doesn't flatten every note to the same timestamp and destroy sort order.
    let mtime = file_mtime_millis(path).unwrap_or_else(crate::model::now_millis);

    let id = front
        .iter()
        .find(|(k, _)| k == "id")
        .map(|(_, v)| v.clone())
        .filter(|v| uuid::Uuid::parse_str(v).is_ok())
        .unwrap_or_else(|| {
            uuid::Uuid::new_v5(&NAMESPACE, rel.to_string_lossy().as_bytes()).to_string()
        });

    let folder = rel
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .and_then(|p| normalize_folder_name(&p.to_string_lossy()));

    // Upstream writes `labels:`; the Obsidian export writes `tags:`.
    let labels = front
        .iter()
        .find(|(k, _)| k == "labels" || k == "tags")
        .map(|(k, v)| parse_tags(v, &text, k))
        .filter(|t: &Vec<String>| !t.is_empty());

    let color = front
        .iter()
        .find(|(k, _)| k == "color")
        .map(|(_, v)| v.clone())
        .filter(|v| v.starts_with('#'));

    let pinned = front.iter().any(|(k, v)| k == "pinned" && v == "true");

    // Two dialects: upstream's mirror writes `archived: <epoch ms>`, the
    // Obsidian/Keep export writes `archived: true`. Accept both.
    let archived_at = front
        .iter()
        .find(|(k, _)| k == "archived")
        .and_then(|(_, v)| {
            if v == "true" {
                Some(updated.or(created).unwrap_or_else(crate::model::now_millis))
            } else {
                v.parse::<i64>().ok().filter(|ms| *ms > 0)
            }
        });

    // Upstream also records the folder in frontmatter; it wins over the
    // directory, since a mirror can be read outside its directory layout.
    let folder_from_front = front
        .iter()
        .find(|(k, _)| k == "folder")
        .and_then(|(_, v)| normalize_folder_name(v));

    // `type: checklist` is authoritative — upstream writes it precisely so the
    // "every line is a task" heuristic isn't needed.
    let declared_type = front
        .iter()
        .find(|(k, _)| k == "type")
        .map(|(_, v)| v.to_ascii_lowercase());

    // Strip a leading `# Title` that just repeats the frontmatter title —
    // upstream's own mirror writes one, and keeping it would duplicate it on
    // every round trip.
    let body_text = strip_redundant_heading(body, &title);
    let items = parse_checklist(&body_text);

    let mut note = Note {
        id,
        title,
        created_at: created.unwrap_or(mtime),
        updated_at: updated.or(created).unwrap_or(mtime),
        device_id: crate::store::device_id(),
        is_pinned: pinned,
        archived_at,
        folder_name: folder_from_front.or(folder),
        labels,
        color,
        ..Default::default()
    };

    let items = match declared_type.as_deref() {
        // Declared checklist: trust it, and parse loosely rather than
        // requiring every line to be a task.
        Some("checklist") => items.or_else(|| Some(Vec::new())),
        Some("text") => None,
        _ => items,
    };

    if let Some(items) = items {
        note.note_type = NoteType::CHECKLIST;
        note.checklist_items = Some(items);
        note.content = String::new();
    } else {
        note.content = body_text.trim().to_string();
    }

    Some(note)
}

/// Splits `---\n…\n---\n` off the front. Returns `(key, value)` pairs and the
/// remaining body.
fn split_frontmatter(text: &str) -> (Vec<(String, String)>, &str) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return (Vec::new(), text);
    };
    let Some(end) = rest.find("\n---") else {
        return (Vec::new(), text);
    };
    let (block, after) = rest.split_at(end);
    let body = after.trim_start_matches("\n---").trim_start_matches('\n');

    let mut pairs = Vec::new();
    for line in block.lines() {
        // List continuations (`  - "tag"`) belong to the preceding key and are
        // re-read from the raw block by parse_tags.
        if line.starts_with([' ', '\t', '-']) {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            pairs.push((k.trim().to_string(), unquote(v.trim())));
        }
    }
    (pairs, body)
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    v.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(v)
        .to_string()
}

/// The desktop/Android wire format supports one folder level. Flatten nested
/// Obsidian paths with a visible safe separator instead of leaving `/` in
/// `folderName` while the filesystem sanitizer silently removes it.
fn normalize_folder_name(raw: &str) -> Option<String> {
    let parts: Vec<&str> = raw
        .split(['/', '\\'])
        .map(str::trim)
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    (!parts.is_empty()).then(|| parts.join(" — "))
}

fn folder_components(folder: &str) -> Vec<String> {
    folder
        .split(" — ")
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn contains_label(labels: &[String], candidate: &str) -> bool {
    let candidate = candidate.to_lowercase();
    labels.iter().any(|label| label.to_lowercase() == candidate)
}

/// Labels appear either inline (`labels: [a, b]` / `tags: [a, b]`) or as a
/// YAML list on the following lines. `key` is whichever spelling was matched.
fn parse_tags(inline: &str, full_text: &str, key: &str) -> Vec<String> {
    let inline = inline.trim();
    if inline.starts_with('[') {
        return inline
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split(',')
            .map(|s| unquote(s.trim()))
            .filter(|s| !s.is_empty())
            .collect();
    }
    if !inline.is_empty() {
        return vec![unquote(inline)];
    }
    // YAML list form: collect the indented `- item` lines after the key.
    let marker = format!("\n{key}:");
    let Some(pos) = full_text.find(&marker) else {
        return Vec::new();
    };
    full_text[pos + marker.len()..]
        .lines()
        .skip(1)
        .take_while(|l| l.starts_with([' ', '\t']) && l.trim_start().starts_with('-'))
        .map(|l| unquote(l.trim_start().trim_start_matches('-').trim()))
        .filter(|s| !s.is_empty())
        .collect()
}

fn heading_title(body: &str) -> Option<String> {
    body.lines()
        .find(|l| l.starts_with("# "))
        .map(|l| l.trim_start_matches("# ").trim().to_string())
        .filter(|t| !t.is_empty())
}

fn strip_redundant_heading(body: &str, title: &str) -> String {
    let trimmed = body.trim_start();
    let Some(first_line) = trimmed.lines().next() else {
        return body.to_string();
    };
    if first_line.strip_prefix("# ").map(str::trim) == Some(title) {
        return trimmed[first_line.len()..]
            .trim_start_matches('\n')
            .to_string();
    }
    body.to_string()
}

/// Returns checklist items when the body is *predominantly* task lines —
/// a single stray `- [ ]` inside prose shouldn't convert a text note.
fn parse_checklist(body: &str) -> Option<Vec<ChecklistItem>> {
    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return None;
    }
    let mut items: Vec<ChecklistItem> = Vec::new();
    for line in &lines {
        let indent = line.len() - line.trim_start().len();
        let t = line.trim_start();
        let parsed = t
            .strip_prefix("- [")
            .or_else(|| t.strip_prefix("* ["))
            .or_else(|| t.strip_prefix("["));
        let Some(rest) = parsed else {
            // An indented non-task line right after an item is a markdown list
            // continuation — the rest of a multiline item (what `to_markdown`
            // writes). An unindented one is prose, so this isn't a checklist.
            if indent > 0 {
                if let Some(last) = items.last_mut() {
                    last.text.push('\n');
                    last.text.push_str(t.trim_end());
                    continue;
                }
            }
            return None;
        };
        let (mark, tail) = rest.split_at(1);
        let text = tail.strip_prefix("] ").or_else(|| tail.strip_prefix("]"))?;
        let order = items.len() as i32;
        items.push(ChecklistItem {
            id: crate::model::new_uuid(),
            text: text.trim().to_string(),
            is_checked: mark.eq_ignore_ascii_case("x"),
            order,
            original_order: order,
            created_at: crate::model::now_millis(),
            indentation_level: (indent / 2) as i32,
        });
    }
    (!items.is_empty()).then_some(items)
}

/// `2025-04-18T06:08:01.820Z` / `2025-04-18T06:08:01Z` -> epoch millis.
pub fn parse_iso8601(s: &str) -> Option<i64> {
    let s = s.trim().trim_end_matches('Z');
    let (date, time) = s.split_once('T')?;

    let mut d = date.split('-');
    let (y, m, day): (i64, i64, i64) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );

    let (time, frac) = match time.split_once('.') {
        Some((t, f)) => (t, format!("{f:0<3}")[..3].parse::<i64>().ok()?),
        None => (time, 0),
    };
    let mut t = time.split(':');
    let (h, min): (i64, i64) = (t.next()?.parse().ok()?, t.next()?.parse().ok()?);
    let sec: i64 = t.next().unwrap_or("0").parse().ok()?;

    // Days-from-civil (Howard Hinnant), the inverse of model::iso8601.
    let y_adj = if m <= 2 { y - 1 } else { y };
    let era = y_adj.div_euclid(400);
    let yoe = y_adj - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;

    Some(((days * 86_400 + h * 3600 + min * 60 + sec) * 1000) + frac)
}

fn file_mtime_millis(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let d = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(d.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEEP_NOTE: &str = "---\ntitle: \"Youtube Videos\"\ncreated: \"2025-04-18T06:08:01.820Z\"\nupdated: \"2025-04-18T12:36:51.339Z\"\narchived: true\npinned: false\ntags:\n  - \"google-keep\"\n  - \"Fun\"\nsource: \"Google Keep\"\n---\nhttps://example.com/one\nhttps://example.com/two\n";

    fn parse_str(text: &str, rel: &str) -> Note {
        parse(
            text,
            &PathBuf::from("/vault").join(rel),
            Path::new("/vault"),
        )
        .unwrap()
    }

    #[test]
    fn iso8601_round_trips_through_the_model_formatter() {
        let millis = parse_iso8601("2026-01-05T14:10:22Z").unwrap();
        assert_eq!(crate::model::iso8601(millis), "2026-01-05T14:10:22Z");
        assert_eq!(parse_iso8601("1970-01-01T00:00:00Z"), Some(0));
    }

    #[test]
    fn parses_milliseconds() {
        let a = parse_iso8601("2025-04-18T06:08:01.820Z").unwrap();
        let b = parse_iso8601("2025-04-18T06:08:01Z").unwrap();
        assert_eq!(a - b, 820);
    }

    #[test]
    fn imports_a_keep_export_note() {
        let note = parse_str(KEEP_NOTE, "Fun/Youtube Videos.md");
        assert_eq!(note.title, "Youtube Videos");
        assert_eq!(note.folder_name.as_deref(), Some("Fun"));
        assert!(!note.is_pinned);
        assert!(note.is_archived());
        assert_eq!(
            note.labels.as_deref(),
            Some(&["google-keep".to_string(), "Fun".to_string()][..])
        );
        assert!(note.content.starts_with("https://example.com/one"));
        assert_eq!(
            note.created_at,
            parse_iso8601("2025-04-18T06:08:01.820Z").unwrap()
        );
    }

    #[test]
    fn nested_obsidian_paths_are_flattened_consistently() {
        let note = parse_str("body", "School/Projects/Plan.md");
        assert_eq!(note.folder_name.as_deref(), Some("School — Projects"));
    }

    #[test]
    fn ids_are_stable_across_reimports_but_differ_per_file() {
        let a = parse_str(KEEP_NOTE, "Fun/Youtube Videos.md");
        let b = parse_str(KEEP_NOTE, "Fun/Youtube Videos.md");
        let c = parse_str(KEEP_NOTE, "Other/Youtube Videos.md");
        assert_eq!(a.id, b.id);
        assert_ne!(a.id, c.id);
    }

    #[test]
    fn an_existing_upstream_id_wins_over_the_derived_one() {
        let md = "---\nid: 11111111-2222-3333-4444-555555555555\ncreated: 2026-01-05T12:00:00Z\nupdated: 2026-01-05T12:00:00Z\n---\n\n# Hi\n\nbody\n";
        let note = parse_str(md, "Hi.md");
        assert_eq!(note.id, "11111111-2222-3333-4444-555555555555");
        // The `# Hi` heading duplicates the title, so it shouldn't stay in the body.
        assert_eq!(note.content, "body");
        assert_eq!(note.title, "Hi");
    }

    #[test]
    fn detects_checklists() {
        let md = "---\ntitle: \"Shopping\"\n---\n- [ ] milk\n- [x] bread\n  - [ ] rye\n";
        let note = parse_str(md, "Shopping.md");
        assert_eq!(note.note_type, NoteType::CHECKLIST);
        let items = note.checklist_items.unwrap();
        assert_eq!(items.len(), 3);
        assert!(!items[0].is_checked);
        assert!(items[1].is_checked);
        assert_eq!(items[2].indentation_level, 1);
        assert_eq!(items[2].text, "rye");
    }

    #[test]
    fn prose_containing_one_task_line_stays_a_text_note() {
        let md = "---\ntitle: \"Plan\"\n---\nSome prose here.\n- [ ] a stray task\nMore prose.\n";
        assert_eq!(parse_str(md, "Plan.md").note_type, NoteType::TEXT);
    }

    /// Walks `tests/fixtures/vault`, which mirrors the shape of a real
    /// Obsidian + Google Keep export: emoji folder names, quoted frontmatter,
    /// a checklist, a bare file, and scaffolding that must be skipped.
    #[test]
    fn scans_the_fixture_vault() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vault");
        let report = scan_vault(&root).unwrap();
        assert!(
            report.skipped.is_empty(),
            "unexpected skips: {:?}",
            report.skipped
        );

        let titles: Vec<&str> = report.imported.iter().map(|n| n.title.as_str()).collect();
        assert_eq!(titles.len(), 5, "got {titles:?}");
        for skipped in ["README", "Deleted Note", "app"] {
            assert!(
                !titles.contains(&skipped),
                "{skipped} should not be imported"
            );
        }

        let by = |t: &str| report.imported.iter().find(|n| n.title == t).unwrap();
        assert_eq!(
            by("Youtube Videos").folder_name.as_deref(),
            Some("Σχολείο 🚸")
        );
        assert!(by("Youtube Videos").is_archived());
        assert!(by("Pinned Note").is_pinned);
        assert_eq!(by("Shopping").note_type, NoteType::CHECKLIST);
        assert_eq!(by("Shopping").checklist_items.as_ref().unwrap().len(), 3);
        assert!(by("Loose Note").folder_name.is_some());
        assert_eq!(
            by("Upstream Mirror").id,
            "11111111-2222-3333-4444-555555555555"
        );
        // Top-level notes belong to no folder.
        assert_eq!(by("Upstream Mirror").folder_name, None);
    }

    /// The mirror must survive a round trip through the importer. Before the
    /// format was corrected against `Note.kt:126`, colour, pin, folder and
    /// archive state were dropped on every write — silently.
    #[test]
    fn the_markdown_mirror_round_trips_without_losing_state() {
        let original = Note {
            id: "11111111-2222-3333-4444-555555555555".into(),
            title: "Quarterly".into(),
            content: "body text".into(),
            created_at: 1_700_000_000_000,
            updated_at: 1_700_000_500_000,
            color: Some("#F28B82".into()),
            is_pinned: true,
            folder_name: Some("Work".into()),
            archived_at: Some(1_700_000_900_000),
            labels: Some(vec!["a".into(), "b".into()]),
            ..Default::default()
        };

        let md = original.to_markdown();
        let back = parse_str(&md, "Work/Quarterly.md");

        assert_eq!(back.id, original.id);
        assert_eq!(back.title, original.title);
        assert_eq!(back.content, original.content);
        assert_eq!(back.created_at, original.created_at);
        assert_eq!(back.updated_at, original.updated_at);
        assert_eq!(back.color, original.color, "colour lost");
        assert_eq!(back.is_pinned, original.is_pinned, "pin lost");
        assert_eq!(back.folder_name, original.folder_name, "folder lost");
        assert_eq!(
            back.archived_at, original.archived_at,
            "archive timestamp lost"
        );
        assert_eq!(back.labels, original.labels, "labels lost");
    }

    #[test]
    fn a_checklist_mirror_round_trips_as_a_checklist() {
        let original = Note {
            title: "Shopping".into(),
            note_type: NoteType::CHECKLIST,
            checklist_items: Some(vec![
                ChecklistItem {
                    text: "milk".into(),
                    order: 0,
                    ..blank_item()
                },
                ChecklistItem {
                    text: "bread".into(),
                    is_checked: true,
                    order: 1,
                    ..blank_item()
                },
            ]),
            ..Default::default()
        };

        let back = parse_str(&original.to_markdown(), "Shopping.md");
        assert_eq!(back.note_type, NoteType::CHECKLIST);
        let items = back.checklist_items.unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].text, "milk");
        assert!(!items[0].is_checked);
        assert!(items[1].is_checked);
    }

    /// A multiline item writes indented continuation lines to the mirror and
    /// folds back into ONE item with the full text — the sync-critical
    /// property of multiline checklist items.
    #[test]
    fn a_multiline_item_round_trips_as_one_item() {
        let original = Note {
            title: "Recipe".into(),
            note_type: NoteType::CHECKLIST,
            checklist_items: Some(vec![
                ChecklistItem {
                    text: "ζύμη:\n500γρ αλεύρι\nνερό χλιαρό".into(),
                    order: 0,
                    ..blank_item()
                },
                ChecklistItem {
                    text: "φούρνος στους 220".into(),
                    is_checked: true,
                    order: 1,
                    ..blank_item()
                },
            ]),
            ..Default::default()
        };

        let md = original.to_markdown();
        assert!(md.contains("- [ ] ζύμη:\n  500γρ αλεύρι\n  νερό χλιαρό"));

        let back = parse_str(&md, "Recipe.md");
        assert_eq!(back.note_type, NoteType::CHECKLIST);
        let items = back.checklist_items.unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].text, "ζύμη:\n500γρ αλεύρι\nνερό χλιαρό");
        assert_eq!(items[1].text, "φούρνος στους 220");
        assert!(items[1].is_checked);
    }

    /// An unindented non-task line still disqualifies the body from being a
    /// checklist — continuations are only ever indented.
    #[test]
    fn unindented_prose_still_blocks_checklist_conversion() {
        let md = "---\ntitle: \"Plan\"\n---\n- [ ] a task\nplain prose\n";
        assert_eq!(parse_str(md, "Plan.md").note_type, NoteType::TEXT);
    }

    /// `type: checklist` is authoritative, so an empty checklist stays one
    /// instead of degrading into a text note.
    #[test]
    fn a_declared_checklist_survives_even_when_empty() {
        let original = Note {
            title: "Empty list".into(),
            note_type: NoteType::CHECKLIST,
            checklist_items: Some(Vec::new()),
            ..Default::default()
        };
        assert_eq!(
            parse_str(&original.to_markdown(), "Empty.md").note_type,
            NoteType::CHECKLIST
        );
    }

    fn blank_item() -> ChecklistItem {
        ChecklistItem {
            id: crate::model::new_uuid(),
            text: String::new(),
            is_checked: false,
            order: 0,
            original_order: 0,
            created_at: 0,
            indentation_level: 0,
        }
    }

    #[test]
    fn a_file_without_frontmatter_still_imports() {
        let note = parse_str("just some text\n", "Loose Note.md");
        assert_eq!(note.title, "Loose Note");
        assert_eq!(note.content, "just some text");
    }
}

#[cfg(test)]
mod commit_tests {
    use super::*;
    use crate::store::Store;

    fn store() -> Store {
        Store::new(std::env::temp_dir().join(format!("sn-imp-{}", crate::model::new_uuid())))
            .unwrap()
    }
    fn note(id: &str, updated: i64) -> Note {
        Note {
            id: id.into(),
            title: format!("n{id}"),
            updated_at: updated,
            ..Default::default()
        }
    }

    #[test]
    fn adds_new_notes() {
        let s = store();
        let report = ImportReport {
            imported: vec![note("a", 100), note("b", 100)],
            skipped: vec![],
        };
        assert_eq!(report.commit(&s, &[]), (2, 0, 0));
        assert_eq!(s.load_all().unwrap().len(), 2);
    }

    #[test]
    fn newer_import_updates_older_local() {
        let s = store();
        let local = note("a", 100);
        s.save(&local).unwrap();
        let report = ImportReport {
            imported: vec![note("a", 200)],
            skipped: vec![],
        };
        assert_eq!(report.commit(&s, &[local]), (0, 1, 0));
    }

    #[test]
    fn older_import_does_not_clobber_newer_local() {
        let s = store();
        let local = note("a", 300);
        s.save(&local).unwrap();
        let report = ImportReport {
            imported: vec![note("a", 100)],
            skipped: vec![],
        };
        // Nothing changes: the local copy is fresher.
        assert_eq!(report.commit(&s, &[local]), (0, 0, 0));
    }

    #[test]
    fn explicit_reorganization_changes_only_organization_on_a_newer_local_note() {
        let s = store();
        let mut local = note("a", 300);
        local.title = "newer local title".into();
        local.content = "newer local text".into();
        local.folder_name = Some("Old folder".into());
        s.save(&local).unwrap();

        let mut imported = note("a", 100);
        imported.title = "stale imported title".into();
        imported.content = "stale imported text".into();
        imported.folder_name = None;
        imported.labels = Some(vec!["Obsidian".into()]);
        let report = ImportReport {
            imported: vec![imported],
            skipped: vec![],
        };

        assert_eq!(report.commit_reorganized(&s, &[local], true), (0, 1, 0));
        let saved = s.load_all().unwrap().pop().unwrap();
        assert_eq!(saved.title, "newer local title");
        assert_eq!(saved.content, "newer local text");
        assert_eq!(saved.folder_name, None);
        assert_eq!(saved.labels, Some(vec!["Obsidian".into()]));
    }

    #[test]
    fn as_is_still_refuses_to_clobber_a_newer_local_note() {
        let s = store();
        let mut local = note("a", 300);
        local.folder_name = Some("Local".into());
        s.save(&local).unwrap();
        let mut imported = note("a", 100);
        imported.folder_name = Some("Import".into());
        let report = ImportReport {
            imported: vec![imported],
            skipped: vec![],
        };

        assert_eq!(report.commit_reorganized(&s, &[local], false), (0, 0, 0));
        assert_eq!(
            s.load_all().unwrap()[0].folder_name.as_deref(),
            Some("Local")
        );
    }

    #[test]
    fn reimport_is_idempotent() {
        let s = store();
        let report = ImportReport {
            imported: vec![note("a", 100)],
            skipped: vec![],
        };
        report.commit(&s, &[]);
        let existing = s.load_all().unwrap();
        // Same export again, same timestamps → no-op, not a duplicate.
        assert_eq!(report.commit(&s, &existing), (0, 0, 0));
        assert_eq!(s.load_all().unwrap().len(), 1);
    }
}

#[cfg(test)]
mod organize_tests {
    use super::*;

    fn note(folder: Option<&str>, labels: &[&str]) -> Note {
        Note {
            folder_name: folder.map(str::to_string),
            labels: (!labels.is_empty()).then(|| labels.iter().map(|s| s.to_string()).collect()),
            ..Default::default()
        }
    }

    #[test]
    fn tags_mode_folds_folder_into_labels() {
        let mut r = ImportReport {
            imported: vec![note(Some("Work"), &["urgent"])],
            skipped: vec![],
        };
        r.reorganize(OrganizeMode::Tags);
        let n = &r.imported[0];
        assert_eq!(n.folder_name, None);
        assert!(n.labels.as_ref().unwrap().contains(&"Work".to_string()));
        assert!(n.labels.as_ref().unwrap().contains(&"urgent".to_string()));
    }

    #[test]
    fn tags_mode_turns_each_nested_folder_level_into_a_tag() {
        let mut r = ImportReport {
            imported: vec![note(Some("School — Projects"), &["school"])],
            skipped: vec![],
        };
        r.reorganize(OrganizeMode::Tags);
        let n = &r.imported[0];
        assert_eq!(n.folder_name, None);
        assert_eq!(
            n.labels.as_deref(),
            Some(&["school".into(), "Projects".into()][..])
        );
    }

    #[test]
    fn folders_mode_promotes_first_label() {
        let mut r = ImportReport {
            imported: vec![note(None, &["Recipes", "food"])],
            skipped: vec![],
        };
        r.reorganize(OrganizeMode::Folders);
        assert_eq!(r.imported[0].folder_name.as_deref(), Some("Recipes"));
        assert_eq!(r.imported[0].labels, None);
    }

    #[test]
    fn folders_mode_keeps_existing_folder() {
        let mut r = ImportReport {
            imported: vec![note(Some("Keep"), &["ignored"])],
            skipped: vec![],
        };
        r.reorganize(OrganizeMode::Folders);
        assert_eq!(r.imported[0].folder_name.as_deref(), Some("Keep"));
        assert_eq!(r.imported[0].labels, None);
    }

    #[test]
    fn as_is_changes_nothing() {
        let mut r = ImportReport {
            imported: vec![note(Some("Work"), &["a"])],
            skipped: vec![],
        };
        r.reorganize(OrganizeMode::AsIs);
        assert_eq!(r.imported[0].folder_name.as_deref(), Some("Work"));
    }
}
