# MAP.md — files and functions

Line numbers drift on every edit. Regenerate with:

```bash
for f in $(find src -name '*.rs'); do echo "### $f"; \
  grep -nE '^(pub )?(fn|struct|enum|impl|const|type) |^    pub fn ' "$f"; done
```

Tests omitted here — see PLAN.md for coverage. **97 tests pass** as of this writing.

---

## Data & storage

### src/model.rs — the wire format (Android-JSON compatible)
| Line | Item | Description |
|---|---|---|
| 11/21 | `NoteType`, `SyncStatus` | `TEXT`/`CHECKLIST`; `LOCAL_ONLY`/`SYNCED`/… (SCREAMING_CASE to match JSON) |
| 30/48 | `ChecklistItem`, `Note` | `Note.extra` (`#[serde(flatten)]`) preserves unknown Android fields |
| 142 | `preview` | List-row subtitle; checklists show ☐/☑ stacked |
| 186 | `to_markdown` | `/notes-md/` mirror, byte-compatible with upstream `Note.kt:126` |
| 251 | `markdown_filename` | Keeps Unicode letters (Greek!), drops emoji/path chars, caps at 200 bytes |
| 287/310/331 | `relative_time`, `is_light_color`, `iso8601` | "3 mins ago"; luminance for row text; epoch→ISO |

### src/store.rs — on-disk `notes/` + `notes-md/`
| Line | Item | Description |
|---|---|---|
| 47 | `load_all` | Loads notes one folder-level deep, sorted pinned-first; skips bad files loudly |
| 78/98 | `save`, `delete` | Atomic write + mirror; delete = hard-delete (trash is metadata via save) |
| 131 | `create_folder` | Makes a folder dir in both trees |
| 151/160 | `move_note`, `delete_folder` | Move relocates files; delete folder moves notes to top level first |
| 180 | `prune_folder_if_empty` | Auto-removes a folder that lost its last note |
| 98 | `delete` | Hard-delete; also writes a `notes/deletions.json` tombstone via `record_deletion` so the phone doesn't resurrect it |
| — | `record_deletion` | Appends `{id, deletedAt, deviceId}` to the shared `notes/deletions.json` ledger (upstream format). `collect()` skips this file |
| — | `migrate_legacy_dirs` / `legacy_data_dir` | One-time move of the old `simple-notes-desktop` config/data dirs → `Notes` on first launch |
| 196 | `empty_trash` | Permanently deletes all trashed notes |
| ~207 | `sweep_trash` | Hard-deletes trash older than N days; `0` = keep forever. Runs once at launch |
| 210 | `delete_tag` | Strips a label from every note (notes kept) |
| 316 | `device_id` | Stable per-install id |
| ~356 | `config_dir`/`default_data_dir` | Now `~/.config/Notes` + `~/.local/share/Notes` |
| ~370 | `migrate_legacy_dirs`/`legacy_data_dir` | One-time startup move of the pre-rename `simple-notes-desktop` folders to `Notes` |

### src/config.rs — `~/.config/simple-notes-desktop/config.json` (mode 0600)
data_dir, bind, port, credentials, autostart, markdown_mirror, default_view,
trash_retention_days. `default_view_index` maps "notes/pinned/archived" →
sidebar row. `TRASH_RETENTION_OPTIONS` + `trash_retention_index`/
`trash_retention_days_for` back the settings dropdown (Never / 7 / 30 / 90).

### src/backup.rs — whole-vault `.zip` export
`export(data_dir, dest)` zips the entire data dir (vault-relative paths, skips
unreadable entries). `suggested_filename(date)` → `notes-backup-YYYY-MM-DD.zip`.
Uses the `zip` v2 dep. Read-only over the vault.

---

## Server & sync

### src/server.rs — embedded WebDAV (replaces upstream's Docker container)
`start` binds before returning (port conflict → UI error); `authorized` = constant-time Basic auth; `local_urls` lists LAN addresses for the phone.

### src/sync.rs — client mode: sync *to* a remote WebDAV vault
| Item | Description |
|---|---|
| `plan` | Pure decision function: local + remote + tombstones → `Vec<Action>`. Last-write-wins on `updatedAt`; the deletion ledger beats even a newer local edit (matching Android — diverging makes zombies); a note that changed folder pushes to the new path and deletes the file it left behind. **The piece an Android port reimplements.** 8 tests, no I/O |
| `Action` | `Pull` / `Push` / `DeleteLocal` / `DeleteRemote`, one note each |
| `merge_tombstones`, `tombstone_ledger` | Union of both sides' `deletions.json`, earliest timestamp wins; rebuilt in the wire format the phone reads |
| `remote_index` | Lists remote `notes/*.json` and reads each one's `updatedAt` — a file mtime is when it was *copied*, which would make the newer side lose |
| `run_once` | Executes a plan; a failed action is collected into `Summary.failed`, never fatal |

### src/webdav.rs — the transport half
`Client::{list_json, get, put, delete}` over `ureq`, Basic auth, `MKCOL` for
missing parent folders. Walks with `Depth: 1` — `infinity` is refused (403) by
our own server and by Apache/nginx. `hrefs()` is a string scan, not an XML parse
(`ponytail:` comment marks the ceiling); `percent_decode` keeps Greek folder
names intact.

### src/naming.rs — automatic server setup helpers
`friendly_name` ("happy-panda"), `random_password` (no look-alike chars), `find_available_port` (scans for a free one).

### src/watcher.rs — inotify on `notes/`
`watch` + `should_reload`: reloads when the phone writes, suppresses our own writes via a `WriteStamp` echo window (1.2s). Debounced 400ms.

---

## Import

### src/import_keep.rs — Google Keep Takeout `.zip`
`scan_zip` reads `Takeout/Keep/*.json` → `Note`s. usec→ms timestamps, palette-name→hex colours, labels→tags (Keep has no folders). Attachments parked in `extra`. Verified against the real 161-note archive.

### src/import_md.rs — Obsidian / Markdown vault
`scan_vault` walks a folder; `parse` handles both the Keep-export and upstream-mirror frontmatter dialects. `ImportReport::commit` writes with last-write-wins (idempotent via UUIDv5 path ids). `OrganizeMode` / `reorganize` — the tags-vs-folders import choice (Keep→tags, Obsidian→folders).

### src/richtext.rs — **PARKED** markdown↔formatting converter
For the future dual edit/view mode. `parse` (md → marker-free text + spans), `to_markdown` (inverse). 7 round-trip tests prove it's lossless. `#![allow(dead_code)]` — not wired to any UI yet.

---

## UI

### src/ui/window.rs (~1442 lines) — widget tree + editor
| Line | Item | Description |
|---|---|---|
| 14/27 | `Filter`, `SidebarTarget` | Active view; what a sidebar row selects (encoded on the row, not by index) |
| 62 | `AppState` | store, notes, selection, filter, multi-select `checked`, server handle |
| 87 | `visible_notes` | Filter + search + pinned-first sort. Home = loose notes only (foldered excluded); global search when a query is set from Home; trash sorts by recency (no pin) |
| — | `all_tags` | Tag counts across all-but-trash (archived included) |
| 233/270 | `Widgets`, `build` | Handles the action layer needs; constructs the whole window |
| — | breakpoints (in `build`) | phone <900 / tablet 900–1199 / desktop ≥1200; window floor 500×500. Two `MaxWidth` collapse breakpoints + a disjoint `MinWidth 1200` desktop breakpoint that reparents the editor header's colour/archive between inline and the ⋮ overflow (AdwWindow applies only ONE breakpoint — disjoint ranges are mandatory) |
| — | `restore_button` / `note_row(note, in_trash)` | Editor "Restore" button (shown for trashed notes); trash rows show a trash icon not a pin |
| — | empty-state guide (in `build`) | "Welcome to Notes" StatusPage `child` = adw::Clamp + markup Label tour of the tools |
| 784 | `apply_source_style` | GtkSourceView scheme, follows light/dark |
| 1048 | `note_row` | List row: colour tint, pin/checklist icon, relative time |
| 1308–1360 | `md_wrap`/`md_line_prefix`/`md_transform`/`build_format_toolbar` | Formatting toolbar (inserts markdown). `build_format_toolbar` returns an `adw::WrapBox` — wraps to extra rows on a narrow editor, one row when wide (needs adw feature `v1_7`) |
| — | `body_placeholder` (in `build`) | Markdown showcase shown over an empty note (Pango markup) |

### src/ui/actions.rs (~1603 lines) — all signal wiring
| Line | Item | Description |
|---|---|---|
| 11 | `connect` | Sidebar, list, new, title, body (autosave), pin, search, archive, delete, undo/redo |
| 285 | `install_color_css` | Per-note-colour CSS provider, rebuilt each refresh |
| 332/454 | `refresh_sidebar`, `refresh_list` | Rebuild sidebar (filters/folders/tags + 3-dot menus) and the note list |
| 566/710 | `build_checklist`, `build_tags` | Checklist editor; tag chips + add-tag entry |
| 871 | `load_editor` | Populates editor; `set_show_content(true)` for phone nav |
| 935 | `start_watching` | inotify → reload on foreign change |
| 1020/1059 | `create_note`, `prompt_new_folder` | New note/checklist; folder dialog |
| 1108 | `connect_bulk` | Multi-select bar: select-all/none, bulk archive/unarchive (context-aware), move, trash |
| — | `prompt_move_to_folder` | Bulk move-to-folder. (Single-note `build_move_popover`/`move_selected_to_folder` were REMOVED — the editor move button duplicated bulk move) |
| 1254 | `build_color_swatches` | 12-swatch Keep palette + clear |
| 1431 | `attach_row_menu` | Visible ⋮ on folder/tag rows → delete |
| 1564 | `confirm_empty_trash` | Empty-trash confirm dialog |

### src/ui/settings.rs (~449 lines) — dialogs + menu actions
`install_actions` (menu → import/server/folder/empty-trash/export-backup); `show_dialog` (Server & Sync; Advanced page has the retention dropdown + a **Reset server credentials** button); `auto_page` (Automatic tab: generates name+password+port **once** on first enable, then reuses — never regenerates on reopen); `pick_and_import_keep/vault` + `confirm_import`; `pick_and_export_backup`.

**Editor header** is compact (undo/redo, Saved, pin, **⋮ overflow** = colour+move+archive in one popover, delete). The **app menu** (import/backup/server/open-folder + empty-trash) lives on the **list header** (`list_menu`, always visible), not the editor header — this keeps the editor's min width small so the toolbar (`adw::WrapBox`) wraps instead of clipping on narrow windows.

---

## Non-source
| Path | Purpose |
|---|---|
| `Assets/icon.png` | App icon (user-provided); generated into `data/icons/hicolor/<size>/apps/` |
| `data/icons/hicolor/` | Bundled symbolic icons + app icon, placed ahead of the desktop theme |
| `data/dev.local.Notes.desktop` | Desktop entry |
| `tests/fixtures/vault/` | Synthetic Obsidian/Keep vault for import tests |
| `TESTING.md` | Manual test checklist for the user |
| `upstream/`, `References/` | Reference clone + user screenshots (gitignored) |
