# PLAN.md

## Goal

A GTK4 / libadwaita notes app for Debian-based Linux that is *also* the sync
server for the Android app
[simple-notes-sync](https://github.com/inventory69/simple-notes-sync) — same
data format, same feature set, no Docker.

---

## Status

### Done and verified

| Area | Evidence |
|---|---|
| Note model, Android-JSON compatible | 3 tests; unmodelled fields survive a round trip |
| Store: folders, atomic writes, trash vs. delete | 6 tests |
| Embedded WebDAV + Basic auth | 3 unit tests **and** live `curl`: 401 unauth, 401 wrong password, 207 `PROPFIND`, 201 `PUT`, 201 `MKCOL` |
| Markdown import (Obsidian / Keep export) | 8 tests, incl. a walk of `tests/fixtures/vault` |
| **Keep Takeout `.zip` import** | 7 tests **and** a dry run over the user's real 161-note archive: 0 skipped, 0 bad dates; counts cross-check against an independent survey of the same zip |
| Archive / unarchive | 5 filter tests: leaves home, stays reachable, beats pinning, distinct from trash, hidden in folder views |
| **Markdown mirror format** | Corrected against `Note.kt:126`; round-trip test proves colour, pin, folder, archive and labels survive. Previously silent data loss on every write |
| **File watcher** | 7 tests incl. a real inotify test. Self-write echo suppression so typing doesn't reload the list underneath you |
| **Search** | 4 tests: title/body/items/labels, Greek case-insensitivity, all-terms-must-match, composes with the active filter |
| **Checklist editor** | Built and rendered: checkboxes, per-item delete, "N completed" divider, add item |
| **UI seen rendering** | `tools/shoot.sh` renders offscreen on Xvfb and captures — no longer blind |
| **Trash retention sweep** | `Store::sweep_trash(days)` deletes trash past the window; runs once at launch; `0` = keep forever (default). Settings dropdown (Never / 7 / 30 / 90 days). 4 tests |
| **Backup export** | `backup::export(data_dir, dest)` zips the whole vault; ⋮ menu → "Back up all notes…" → save dialog (dated default name). Round-trip test proves bytes + paths + Greek survive; skips unreadable entries. 3 tests |
| **Wrapping format toolbar** | `adw::WrapBox` (libadwaita 1.7 feature `v1_7`) replaces the horizontal `ScrolledWindow`: one row when the editor is wide, wraps to 2–3 rows when narrow — no button ever hidden behind a scroll. Verified by render at wide + default width |
| **Delete-sync tombstone** | `Store::delete` writes `notes/deletions.json` (upstream's ledger format) so a PC hard-delete propagates to the phone instead of being resurrected. `collect()` skips the ledger. 1 test |
| **Adaptive layout (rebuilt)** | phone <900 / tablet 900–1199 / desktop ≥1200; window floor 500×500; editor header buttons inline on desktop, ⋮ overflow below. Disjoint breakpoints (AdwWindow applies only one). Render-verified at 750/1000/1280 |
| **Multi-select (fixed)** | Row checkbox is the source of truth (had no handler → "0 selected"); select-all/none in the bulk bar; bulk move/archive/delete work |
| **Tags & search scope** | Tags span all-but-trash (archived included); search from Home is global (archived + trash) when a query is present. 2 tests |
| **Folders exclusive** | A foldered note shows only in its folder + tags + global search, not in Home. Editor move button removed (use bulk select). "All notes" renamed **Home** |
| **Trash view + restore** | Trash icons on every row, no pin/pin-sort; a **Restore** button clears `trashedAt`; trash button → "Delete permanently" |
| **Empty-state guide** | "Welcome to Notes" StatusPage with a tool tour (editor bar + list bar), shown on first launch / no selection |
| **`.deb` beta pipeline** | `cargo-deb` → `notes_0.0.1~beta.N-1_amd64.deb`; user installs and tests on their real desktop |
| Builds clean, 97/97 tests pass | `cargo test` |

### Real-archive dry run (2026-07-20)

`takeout-20260713T092623Z-2-001.zip` — 161 notes:

| | |
|---|---|
| imported / skipped | 161 / 0 |
| checklists | 4 |
| coloured | 61 |
| labelled | 108 |
| pinned / archived / trashed | 8 / 116 / 0 |
| untitled | 18 |
| attachments referenced | 6 |

Reproduce (read-only):

```bash
KEEP_ZIP=/path/to/takeout.zip cargo test dry_run_real_takeout -- --ignored --nocapture
```

### Built but NOT verified

- **The Markdown vault importer has never run against a real vault.** Tested
  only against the 5-file synthetic fixture. The real 332-file export
  (`~/Έγγραφα/Claude/Projects/Defaulty/MyNotes`) has not been scanned.
  `scan_vault` writes nothing, so a dry run is safe. (The *Keep zip* importer
  HAS been run against the real 161-note archive.)
- **Nothing has been clicked.** The renders are static: the app launches and
  draws correctly, but no interaction has been exercised. Typing, toggling a
  checkbox, searching and archiving are all wired and unit-tested at the state
  level, but the click paths are unproven.

### Verified by DRIVING the app (xdotool on Xvfb) — no longer just static

- Typing into a title persists to disk (fixed selection-loss bug holds live)
- Checklist checkbox toggles + persists + auto-sorts below the divider
- Tag picker dialog opens with existing tags + create field
- Multi-select bulk-archive: 2 ticked → 2 archived on disk, 0 panics
- Folder-create dialog renders and is reachable

Two earlier interaction "failures" were **stale test coordinates**, not app
bugs — coords read from a screenshot taken before the tag row existed. Always
re-read coordinates from a *current* render.

### Bugs found by importing REAL data (2026-07-21)

Running the actual 161-note Keep archive through the new import path surfaced
two bugs no synthetic fixture had:
1. **Greek titles → `<uuid>.md` mirrors.** `markdown_filename` filtered to
   `is_ascii()`, so 110 of 161 Greek titles sanitised to empty and fell back to
   an unreadable UUID name — defeating the mirror's purpose. Now keeps
   Unicode alphanumerics, drops emoji/symbols/path chars. 110 → 18 (the 18 are
   genuinely blank-title notes).
2. **Over-long filename → mirror write failed.** One note's "title" was a
   693-byte paragraph; the JSON master wrote but the `.md` exceeded the
   255-byte fs limit and silently failed. Now capped at 200 bytes on a char
   boundary.

### Bugs the render loop caught (that tests could not)

1. **Crash on launch** — `if let Some(x) = state.borrow()` holds the borrow
   through the body, so the `borrow_mut()` inside panicked. Compiled fine.
2. **Typing broke after one character** — `refresh_list` read `selected` *after*
   clearing the list, but removing rows makes GTK emit `row-selected(None)`,
   which the handler obeyed. Every keystroke rebuilds the list, so the second
   character found no selected note and was silently dropped. Now guarded by
   `AppState::rebuilding`, with two regression tests.
3. **Icons were invisible** — bundled icons need
   `<path>/<theme>/<size>/<context>/`; the theme directory level was missing.

### Known bugs

1. **Importers are unreachable from the UI.** `scan_vault` and `scan_zip` exist
   and are tested, but nothing calls them.
2. **The UI beyond the first screenshot is unverified.** Icons, colours,
   timestamps, search box and watcher were all added without being seen.

---

## Working now (built, most verified by render/drive)

Notes CRUD, autosave, pinning (re-sorts), archive/unarchive (single + bulk),
trash + empty-trash, folders (create / move / delete / auto-prune empty),
tags (add/remove chips, sidebar section with counts, delete-tag), search,
checklist editor, colour picker (12 Keep swatches), undo/redo + "Saved",
pill **markdown formatting toolbar** (inserts markdown), **markdown syntax
highlighting** (GtkSourceView), **empty-body markdown showcase hint**,
folder/tag **3-dot menus**, **adaptive layout** (phone/tablet/desktop
breakpoints + back nav), renamed to **Notes** + app icon.

Server & import: embedded **WebDAV server** (curl-verified), **Automatic tab**
(one switch → generated name/password/free-port + copyable details), **Keep
`.zip` import** (verified on the real 161-note archive), **Obsidian/Markdown
vault import**, **import organise choice** (as-is / all-tags / all-folders),
**file watcher** (live phone-sync updates).

---

## CHECKPOINT 2026-07-22 (resume here)

State: shipping `.deb` betas to the user (currently **beta5**,
`notes_0.0.1~beta.5-1_amd64.deb`); they test on their real desktop each round.
**97 tests pass.** All committed on `main`. See MEMORY "2026-07-22" for the full
narrative of the beta rounds (adaptive layout rebuild, multi-select fix, tags/
search scope, exclusive folders, Home rename, trash+restore, empty-state guide,
delete-sync tombstone).

**Next milestone — cut v0.0.1:** once the user blesses a beta build, bump
`version` to `0.0.1`, build the release `.deb`, write the README (feature cards +
captioned screenshots + the problem statement: a Keep/Obsidian-compatible Linux
notes app that *is* its own sync server, no Docker), and push the first release to
`github.com/kalotrapezis/Notes` (AGPL-3.0). **Ask before pushing** (public
publish). License already matches (repo AGPL-3.0 = Cargo.toml).

**Still to do / open:**
- Folder-create refresh: user reported a new empty folder didn't appear until
  selecting another note (they said "forget it for now"). `prompt_new_folder`
  already calls `refresh_sidebar` — check whether the sidebar sources folders from
  `store.folders()` (disk) so an empty dir shows immediately.
- Review `NotForGit Phone side of things/` screenshots for UI/sync clues.
- Prune old `deletions.json` entries eventually (upstream ages them; warns >1000).
- Confirm-dialog when deleting pinned notes (user asked earlier; not yet built).
- Images/attachments still not synced (Keep `keepAttachments`, Obsidian `![[...]]`).

---

## What's LEFT (for the new chat)

### Parked (biggest, deliberate)
- **Dual edit/view "Live Preview" mode** (Typora/Obsidian style): an *editable*
  view where `**bold**` shows as **bold** with markers hidden; toolbar + markdown
  still work; file on disk stays markdown. **The risky core is done:**
  `src/richtext.rs` converts markdown↔formatting spans with **7 round-trip tests
  proving it's lossless** (critical — content syncs to the phone). What remains
  is the fiddly GTK live-editing: cursor behaviour at hidden markers, typing at
  tag boundaries, mode toggle. Rebuild the tags↔buffer bridge (was written then
  removed) from the converter. Do it carefully, not rushed.

### Client mode — DONE (2026-07-29)
The desktop can now sync *to* a remote WebDAV vault, not just host one.
`sync::plan` is pure and tested; `webdav::Client` is the transport;
`tests/sync_roundtrip.rs` runs the client against this app's own server — the
same code the phone talks to — covering convergence, last-write-wins both ways,
the zombie check across two runs, folders, and unknown-field round trips.
Settings → Server → "Sync to another server" holds the URL/credentials, an
interval (manual / 5 / 15 / 60 min) and a **Sync now** button. A launch pass runs
quietly. Nothing refreshes the list explicitly: sync writes into the vault the
filesystem watcher already watches. `cargo run --example serve -- <dir> <port>`
serves a directory for testing without a second machine.

### Requested but not yet built
- **Context menus as bottom sheets on small screens** — the 3-dot / folder-move
  menus are `gtk::Popover`s; on a phone they should be full-width bottom sheets.
  Use `adw::Dialog` (auto bottom-sheet when narrow) gated on window width.
- **Tablet layout = sidebar + list TOGETHER, editor separate.** Current tablet
  tier collapses list/editor instead; needs re-nesting the split views.
- **Client mode** — connect to a *remote* WebDAV server (user deprioritised; can't
  test it). Leave for last.

### Smaller
- **Undo scope** covers the note body only, not tag/title/colour (those have their
  own controls). Fine for now.
- **Decide untitled-note naming.** 18 of 161 Keep notes have no title → all show
  "Untitled". Deriving from the first content line changes the mirror filename —
  decide before a bulk import.
- ~~Package the `.deb`~~ **DONE** — `cargo-deb` builds
  `target/debian/simple-notes-desktop_0.1.0-1_amd64.deb`. Assets fixed to ship the
  real desktop file (`dev.local.Notes.desktop`), app icons to
  `/usr/share/icons/hicolor`, and the bundled icon set to
  `/usr/share/simple-notes-desktop/icons` (the path `install_icons()` reads).
- **Asset / image sync.** Keep attachments are parked in `Note::extra`
  (`keepAttachments`), never extracted; Obsidian `![[image.png]]` links resolve
  to nothing.
- **Checklist drag-to-reorder** (upstream has it).

### Not applicable / Android-only
Biometric-PIN lock, home-screen widgets, HTML-paste conversion.
6. Asset/image sync (Keep attachments are parked in `extra`, not extracted).

---

## Decisions worth not re-litigating

- **The Docker container is gone.** Upstream's `server/` is stock
  `hacdias/webdav` over a directory; there is no custom protocol. So the app
  serves its own note directory. That is what makes this a *server*, not
  another WebDAV client.
- **JSON stays the master, Markdown stays a mirror.** Matching upstream. Two
  masters means conflicts.
- **Unknown JSON fields round-trip** via `Note::extra`. Two apps share these
  files bidirectionally; silent field loss was the main data-integrity risk.
- **Imported note ids are UUIDv5 of the vault-relative path.** Re-importing
  updates in place instead of duplicating. Tradeoff: renaming a file in the
  vault makes it import as a new note.
- **A stray `- [ ]` in prose does not make a checklist.** Conversion requires
  every non-blank line to be a task line.
- **Server binds `127.0.0.1` by default.** LAN exposure is a deliberate choice
  in the settings dialog, not a default.

---

## Security notes

- `config.json` holds the WebDAV password **in plaintext**, mode `0600`.
  Anything better needs libsecret.
- A personal vault containing password, medical and banking notes was once
  copied into this repo. It has been removed, and `/MyNotes` and `/References`
  are gitignored. Nothing was ever committed — the repo has no commits.
  **Do not put personal notes back in this directory.**
