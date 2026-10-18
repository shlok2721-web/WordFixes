# MEMORY.md — session log

Append-only. Newest at the bottom. Facts and decisions worth carrying into a
fresh session; see PLAN.md for what to do next and MAP.md for where things are.

---

## 2026-07-20 — fork, port, importer

**What this is.** A fork/port of the Android app
[simple-notes-sync](https://github.com/inventory69/simple-notes-sync) to a GTK4
Linux desktop app that also acts as its sync server.

**Upstream research.** Read via a shallow clone in `upstream/` (gitignored):

- Android, Kotlin, Jetpack Compose, AGPL-3.0, ~276 Kotlin files.
- `server/` is stock `hacdias/webdav` in Docker over a plain directory. **There
  is no custom sync protocol** — this was the key finding. Sync is just files
  over WebDAV, last-write-wins on `updated`.
- Storage: `/notes/<uuid>.json` master, `/notes-md/<Title>.md` mirror. Folders
  map to one level of subdirectory. Local-only folders never upload.
- JSON field names came from `models/Note.kt` `readNoteField` — the port copies
  them exactly.

**Stack chosen.** Rust + gtk4-rs + libadwaita, `dav-server` + hyper for WebDAV,
`cargo-deb` for packaging. Installed: GTK 4.22.4, libadwaita 1.9.0 on Ubuntu
26.04. `libadwaita` needs feature `v1_5` for `PreferencesDialog`.

**Verified live**, not just by unit test: WebDAV returns 401 unauthenticated,
401 on a wrong password, 207 on `PROPFIND`, and accepts `PUT`/`MKCOL` from
`curl` — files land in the right place on disk.

**Never verified: the GUI renders.** A screenshot attempt captured the user's
file manager (Dolphin) instead of the app; it was mistaken for the app's UI in
conversation, then corrected. Do not repeat that claim. Nobody has seen this
app's window.

**User's vault.** A 332-file Obsidian export of Google Keep, mostly folders of
`.md` with consistent quoted YAML frontmatter (`title`, `created`, `updated`,
`archived`, `pinned`, `tags`, `source`). 327/329 have frontmatter, 8 contain
task lines, 7 reference attachments.

**Security incident, resolved.** That vault — including folders named `κωδικοί`
(passwords), `Ιατρικά` (medical) and `Τράπεζα` (bank) — was copied into this
repo, untracked but not ignored. Flagged before doing anything else; `/MyNotes`
and `/References` are now gitignored; the repo has no commits, so nothing was
ever exposed. The copy was deleted only after confirming an identical copy at
`~/Έγγραφα/Claude/Projects/Defaulty/MyNotes` (`diff -rq` clean). It was
replaced with a synthetic fixture at `tests/fixtures/vault/`. Contents of the
sensitive folders were never opened.

**Importer.** `src/import_md.rs`, pure — reads, returns a report, writes
nothing. Handles both the Keep/Obsidian dialect and upstream's own mirror
format. Not wired to any UI yet, and never run against the real vault.

---

## 2026-07-20 (later) — Keep Takeout importer

**Second import route added.** The user pointed at a real Google Takeout
archive (`Defaulty/KeepExportIthing/takeout-20260713T092623Z-2-001.zip`) and
noted that "export the notes and import the zip" is how other tools do it. So
`src/import_keep.rs` reads Takeout `.zip` directly — no Obsidian round trip.

**Takeout schema** (surveyed across all 161 notes, keys only, values not read):
`title`, `textContent`, `createdTimestampUsec`, `userEditedTimestampUsec`,
`isPinned`, `isArchived`, `isTrashed`, `color`, `labels[].name`,
`listContent[].{text,isChecked}`, `attachments[].{filePath,mimetype}`,
`annotations[]`. Timestamps are **microseconds**. Colours are palette names
(`DEFAULT`, `RED`, …), not hex. Keep has **no folders** — labels carry the
organisation, so `folder_name` stays `None`.

**Dry run on the real archive passed**: 161 imported, 0 skipped, 0 impossible
timestamps. Counts (8 pinned, 116 archived, 61 coloured, 108 labelled, 4
checklists, 6 attachments) match an independent Python survey of the same zip
exactly — two methods agreeing, not one method trusted.

**Kept, deliberately.** Attachments and annotations go into `Note::extra` as
`keepAttachments` / `keepAnnotations`. Assets aren't synced yet; parking them
means a future version can pick them up without a re-import.

**Open question for the user.** 18 of 161 notes have no title. They'd all show
as "Untitled", and title drives the Markdown mirror's filename — so this wants
deciding before a real import, not after.

**Caveat.** The archive is named `-2-001`, which suggests a multi-part Takeout;
161 zip notes vs 332 vault `.md` files. The zip may not be the whole picture.

---

## 2026-07-20 (later still) — archive

**Asked whether archiving exists.** Upstream has it in full (v2.11.0: editor +
multi-select, archive view, bulk, undo snackbar). The port had it *half* built —
`archived_at`, `is_archived()`, `Filter::Archived` and the sidebar row all
existed, but **nothing ever set `archived_at`**, so the sidebar entry was a
dead end. Now wired: a toggle in the editor header.

**Bug found while checking.** `load_editor` never synced the pin button, so it
displayed the *previously* selected note's pinned state. Now both pin and
archive sync on selection.

**Icon note.** Adwaita ships no archive glyph — verified by listing the theme,
not assumed. Used `view-conceal-symbolic` (the hide/eye icon), which also
matches how the user described the feature: "hidden one click away". All icon
names used in the UI were checked against the installed theme; a missing name
renders blank, which would be invisible in code review.

**Semantics chosen** (see the 5 tests in `ui/window.rs`): archive beats pin, so
a pinned+archived note is hidden rather than sitting on the pinned shelf;
trash beats archive; folder views hide archived notes too.

---

## 2026-07-20 (autonomous stretch) — first render, and what it exposed

**The app was seen rendering for the first time.** Method that worked: launch
and capture in the *same* shell command, while it still holds focus
(`spectacle -b -a -n -o out.png`). Two later attempts grabbed unrelated
windows instead — active-window capture follows focus, and there is no
scriptable way to focus a window on this KDE/Wayland session. **Do not retry
fullscreen capture**; it grabs the user's desktop and personal files.
`sudo apt install xvfb` is the real fix and is pending.

Also note: `pkill -f 'target/debug/simple-notes-desktop'` **kills its own
shell**, because the pattern matches the bash command running it (exit 144).
Kill by PID instead.

**What the render exposed**, none of which was visible in code:
- Icons rendered as broken red squares. Earlier verification was wrong
  methodology — checked Adwaita, but the active theme is **Breeze**, which
  ships none of `view-pin-symbolic`, `view-conceal-symbolic`,
  `view-list-symbolic`.
- Every note colour was missing; the mobile app's whole visual identity.
- Three separate header bars look disjointed; the empty-state pane is huge
  and says nothing.
- The 3-pane layout itself works well on desktop.

**Decisions this session.** Icons now bundled in `data/icons/` and placed
*ahead* of the desktop theme in the search path — deliberately overriding the
user's theme, for a consistent look. Converted from the user's own SVG library
at `Coding/PixelRuller/Assets/SVGs` (SVG Repo icons, 24×24, stroke-based, some
hardcoded `#fff` which would have been invisible on light themes). **Not
visually verified** — no rasterizer available (`rsvg-convert`, `cairosvg`,
Inkscape all absent).

**Upstream check that mattered.** The official
`inventory69/simple-notes-desktop` (Tauri 2.0, v0.12.0) exists and is mature —
15 themes, search, image attachments. But it is a WebDAV **client**; it needs
an external server. The user rejected it as looking like a stretched phone app,
so the port continues. Our differentiators: it *is* the server, plus Keep/
Obsidian import.

---

## 2026-07-20 (evening) — the render loop, and what it caught

**`tools/shoot.sh` is the thing that changed everything.** Xvfb on `:99` +
`GDK_BACKEND=x11` + Pillow's `ImageGrab.grab(xdisplay=':99')`. Renders the app
offscreen and captures it — never touches the real desktop, never captures the
user's windows. Use this, not Spectacle.

**Three bugs it caught that the 48 unit tests could not:**

1. **Crash on launch.** `if let Some(x) = state.borrow()...` keeps the borrow
   alive through the body; the `borrow_mut()` inside panics. Bind to a separate
   `let` first. Compiled clean.
2. **Typing broke after one character.** `refresh_list` read `selected` *after*
   the row-removal loop, and removing rows makes GTK emit `row-selected(None)`,
   which the handler acted on. Since every keystroke rebuilds the list, the
   second character found no selected note and was dropped silently. Fixed with
   an `AppState::rebuilding` guard + reading the selection before teardown.
   Two regression tests.
3. **Bundled icons never loaded.** GTK needs
   `<search-path>/<theme>/<size>/<context>/`; `data/icons/scalable/actions/`
   was missing the theme level. Now `data/icons/hicolor/…` with an
   `index.theme`.

**Design fixes made by looking:** wider note column (340–460px), card-style
rows with rounded corners and margins, colour tint with luminance-picked text,
relative timestamps, checklist previews stacked with ☐/☑ instead of flattened
with `·`, list title tracks the active view, search placeholder carries the
count, editor header shows the note title, first note auto-selected on launch,
much quieter empty state.

**Checklist editor now exists** — checkboxes, per-item delete, "N completed"
divider with checked items sunk below it, Add item. No drag-reorder yet.

**GTK gotcha:** a `gtk::Button` with both `label` and `icon_name` shows only the
icon. Use `adw::ButtonContent` for icon + label.

**Still unproven: interaction.** Every render is static. Nothing has been
clicked or typed into. `xdotool` on the Xvfb display is the next step.

---

## 2026-07-20 (late, storm) — driving the app with xdotool

`tools/shoot.sh` renders; a sibling `drive.sh` (in scratchpad) adds xdotool on
Xvfb :99 to click and type, then capture. This is how interaction gets proven.

**Verified live** (not just unit tests): title typing persists, checklist
toggle persists + auto-sorts, tag picker opens, multi-select bulk-archive puts
`archivedAt` on exactly the ticked notes. Zero panics across all flows.

**Two gotchas burned time:**
1. **Stale click coordinates.** Two "bugs" were just my xdotool clicks landing
   on an older layout (coords read from a pre-tag-row screenshot). ALWAYS
   recapture a current render and read coords from it before driving.
2. **Float timestamps in test JSON.** Seeding notes with `now-8e7` writes a
   FLOAT to `createdAt`; serde `i64` rejects it, so `load_all` silently skips
   every note (folders still show, since `folders()` reads dirs). The app was
   fine — the fixture was malformed. Use integer literals. Also worth noting:
   a single unparseable note is skipped silently, which is correct for
   robustness but means malformed data just vanishes from view.

**Features added this session:** multi-select mode (checkbox rows + bulk bar),
bulk archive / trash / move, single+bulk move-to-folder (`Store::move_note`;
folder was previously only set at creation and unchangeable), and a Keep-style
tag picker dialog replacing the inline chip+x (the x sat exactly where you'd
aim to add, with no undo — a real trap).

**User's Keep tag design, adopted:** button opens a modal listing existing tags
as checkboxes, with a "create new tag" field below. Built and opened live.

---

## 2026-07-21 — import UI wired + two real-data bugs

**Import is reachable now.** ⋮ menu → "Import notes" submenu → Keep .zip or
Obsidian folder → native picker → preview dialog ("Found N, X new") → commit.
`ImportReport::commit(store, existing)` does last-write-wins by id; deterministic
ids make reimport idempotent (4 tests + driven against the real archive: 161
added, 0 failed, rerun 0).

**Two bugs the REAL 161-note Keep archive exposed that fixtures never did:**
1. `markdown_filename` used `is_ascii()` → 110/161 Greek titles collapsed to
   empty → `<uuid>.md`. Fix: keep Unicode alphanumerics, drop emoji/symbols +
   path chars. This is THE lesson: the user's data is Greek; ASCII assumptions
   silently fail on the majority of it. Test with Greek, not "My Shopping List".
2. A 693-byte paragraph-as-title exceeded the 255-byte filename limit (Greek =
   2 bytes/char); JSON master wrote but `.md` write failed silently. Fix: cap
   at 200 bytes on a char boundary.

**Added a `[lib]` target + `src/lib.rs`** so `examples/import_smoke.rs` can
exercise the real scan→commit path headlessly against the archive. Kept — it's
the fastest way to verify importers on real data without the GUI.

---

## 2026-07-21 — editor header: colour picker, undo/redo, Saved

Added to the editor header: undo/redo (left), "Saved" indicator, and a colour
MenuButton opening a 4×3 grid of Keep's 12 swatches + a clear/no-colour button.
Colour picker verified end-to-end (clicked yellow → `#FFF475` on disk).

**The undo bug worth remembering.** Undo/redo buttons stayed greyed after
typing, so undo was unclickable and appeared broken. Root cause: I polled
`buffer.can_undo()` synchronously inside the buffer's `changed` handler — but
GTK commits the undo action AFTER `changed` returns, so mid-signal it always
reads false, greying the button permanently. An isolated probe (examples/, since
deleted) proved the API itself was fine: insert → can_undo=true. **Fix: drive
button sensitivity from `buffer.connect_can_undo_notify` / `can_redo_notify`,
not from polling inside `changed`.** General lesson: GTK undo state is only
valid to read on its own notify signals, never synchronously during `changed`.

Also: `set_max_undo_levels(0)` on a plain TextBuffer means ZERO steps (undo
off), not "unlimited" — that's GtkSourceView's convention, not GtkTextBuffer's.
Removed it.

**Scope note for undo:** it covers the text body only (native TextBuffer undo).
Tags/title/colour are not on that stack. Colour has an explicit reset (the
no-colour swatch); tag chips have per-chip ×. A full app-level undo across note
operations would be a separate, larger feature — not built.

---

## 2026-07-21 (morning) — Markdown highlighting via GtkSourceView

`libgtksourceview-5-dev` installed. Added `sourceview5 = "0.9"` (0.11 needs a
newer gtk4-sys than our gtk 0.9 — version-match matters). Swapped the plain
`gtk::TextView` for `sourceview5::View` + `sourceview5::Buffer` with the
`markdown` language and `set_highlight_syntax(true)`.

Key compatibility win: `sourceview5::Buffer` is a `GtkTextBuffer` subclass, so
it `.upcast()`s into the existing `Widgets.buffer: gtk::TextBuffer` field and
ALL the existing change/undo/save wiring kept working untouched. No churn.

Style scheme follows light/dark via `apply_source_style()` (Adwaita /
Adwaita-dark, with fallbacks) and re-applies on `StyleManager::connect_dark_notify`.

Verified by render: headings teal, bold/italic, code spans blue, list markers
orange/red, links, blockquote — and LIVE re-highlight while typing
`**new bold**`. Light mode confirmed visually; dark path is symmetric but not
visually confirmed offscreen (ADW_DEBUG_COLOR_SCHEME didn't force dark in the
Xvfb run).

Needs `use sourceview5::prelude::*;` in window.rs for View/Buffer setter methods.

---

## 2026-07-21 (cont.) — move-to-folder, bulk unarchive, test handoff

Single-note move: `build_move_popover` (folder-symbolic button in editor header)
lists folders + "No folder" + "New folder…"; `move_selected_to_folder` calls
`Store::move_note` (relocates JSON + mirror, no duplicate) then reload+refresh.
Rebuilt in `load_editor`. 3 tests.

Bulk move already existed (`prompt_move_to_folder` + `store.move_note`). Added
**bulk unarchive**: the bulk-archive button is now context-aware — in the
Archived view it clears `archived_at` (icon flips to view-reveal, tooltip
"Restore selected"), elsewhere it archives. 1 test.

69 tests pass. Release binary built. Wrote `TESTING.md` — a 13-section manual
checklist for the user to run themselves (they asked to test rather than watch
me drive xdotool). Handoff point.

**xdotool coordinate lesson, again:** header buttons shift when one is added
(the move button pushed archive/delete right), so hardcoded click coords go
stale every layout change. Re-read coords from a CURRENT render, or better —
let the user test real interactions.

---

## 2026-07-21 (user testing round 1) — bug fixes from real use

User ran the app and reported bugs. Fixed:

1. **Pin/unpin didn't move the note between Pinned/Others sections.**
   `visible_notes()` filtered but never re-sorted, so an in-memory pin change
   kept the note's old position (only `load_all` sorted, on reload). Now
   `visible_notes()` sorts pinned-first every call. This ALSO fixed the
   "no separator" complaint — the "Pinned"/"Others" section headers existed but
   looked broken because unsorted notes made them interleave. Pin handler also
   now `reload()`s.

2. **Couldn't toggle the server on.** The password was only written to config on
   settings-dialog *close*, but the server switch is INSIDE the dialog — so
   flipping it read an empty password and bounced off. Fix: credential fields
   (bind/port/user/pass) now commit to config live on change.

3. **"Obsidian import didn't nest into folders" — NOT a bug.** Verified against
   the real 328-note vault: scan_vault assigns folders 328/328, commit puts all
   328 into folder subdirs, 0 at top level. The notes ARE foldered; they also
   appear in "All notes" (correct), which reads as "not organized".

Added features the user asked for:
- **Import "Organise by" choice** (As-is / Folders / Tags) in the confirm
  dialog. `ImportReport::reorganize`: Tags folds folder→label; Folders promotes
  first label→folder. Addresses Keep-makes-tags vs Obsidian-makes-folders. 4 tests.
- **Folder delete** via right-click on a sidebar folder row. `Store::delete_folder`
  moves notes to top level first (never deletes notes), then removes the empty
  dir. 2 tests.

54 tests pass.

### Still requested, NOT yet done (next batch)
- **Client mode** — connect to a remote WebDAV server (for users with several
  PCs). Big: needs a sync engine (pull/push/merge), not just hosting. The app is
  currently server-only.
- **Backup export** — zip the notes dir to a chosen location.
- **Trash retention setting** — "delete after X days" dropdown + a sweep.
- **Drag-and-drop notes into folders** (right-click "Move" exists via the editor
  header button; a list-row context menu + DnD would be nicer).
- Formatting toolbar, checklist drag-reorder, image sync, `.deb`.

---

## 2026-07-21 (user testing round 2) — auto server setup + visible 3-dot menus

**Automatic/Advanced server tabs.** `show_dialog` now adds two adw
PreferencesPages (libadwaita renders a tab switcher). New `auto_page`: one
"Server running" switch that generates a friendly username (LocalSend-style
`happy-panda`), a look-alike-free password, picks a free port
(`naming::find_available_port`), binds 0.0.0.0, starts — then shows Address /
Username / Password as copy-button rows. Verified by driving: toggle →
`happy-panda` / `VuRPmL6Ukwtf` / port 8089 / bind 0.0.0.0, server actually
started (also re-confirms the toggle-bug fix). New `src/naming.rs` (friendly
name, password, port scan) + `rand = 0.10` — note rand 0.10 renamed
`gen_range`; use `IndexedRandom::choose`, not `random_range`.

**Visible 3-dot menus replace hidden right-click.** User asked how folder
delete was triggered and rightly flagged the hidden secondary-click as
undiscoverable. Now every folder AND tag sidebar row has a `view-more-symbolic`
⋮ MenuButton suffix → Delete. `Store::delete_tag` strips a label from all notes
(notes kept). `RowKind` enum drives the shared `attach_row_menu`. Removed the
old GestureClick approach.

80 tests pass. Still deferred: client mode (last, user can't test it), backup
export, trash retention setting, note drag-to-folder.

---

## 2026-07-21 (round 3) — rename to "Notes", adaptive layout, prune, empty-trash

**Naming.** Upstream name can't be reused. App is now "Notes" everywhere
(window/sidebar title, desktop file). APP_ID → `dev.local.Notes` (neutral
placeholder; config dir string is unchanged — `dirs` uses the literal
"simple-notes-desktop", not the app-id, so configs didn't move). User provided
`Assets/icon.png` (blue note+pencil); generated 48–512px into
`data/icons/hicolor/<size>/apps/` and **registered those dirs in index.theme**
(the icon showed as a broken red square until the theme index listed the apps
directories — Fixed-type entries per size).

**Adaptive layout** via `adw::Breakpoint` on the window:
- ≥860px desktop: sidebar | list | editor (3 panes)
- 500–860 tablet: inner split collapsed → sidebar | (list⇄editor + back button)
- <500px phone: both splits collapsed → one pane at a time, back buttons
`set_show_content(true)` on sidebar-select (→list) and note-select (→editor)
drives the navigation. Verified by rendering at 420/720/1280px.
NOTE: user wanted tablet = sidebar+list TOGETHER + editor separate; current
middle tier instead swaps list/editor. Exact tablet layout needs re-nesting the
split views — deferred.

**Removed note title from the editor header** (`editor_page.set_title("")`);
the name lives only in the big title field, so it's not shown twice.

**Auto-prune empty folders:** moving a note out of a folder that becomes empty
removes it (`Store::prune_folder_if_empty`, targeted — does NOT prune
freshly-created empty folders, only ones emptied by a move). Tags already vanish
at 0 (derived from notes).

**Empty trash:** `⋮` MenuButton in the list header, visible only in Trash view
(toggled in refresh_list), → confirm → `Store::empty_trash` permanently deletes
all trashed. 82 tests.

**DEFERRED (user's 1st bullet):** context menus as full-width bottom sheets on
small screens. They're `gtk::Popover`s now — functional on mobile but not
bottom-sheet styled. Needs swapping to `adw::Dialog` (auto bottom-sheet when
narrow) gated on window width. Polish item.

---

## 2026-07-21 (round 4) — editor: markdown tips hint; dual-view PARKED

**Long back-and-forth clarifying one feature.** The user wants a Typora/Obsidian
"Live Preview": an EDITABLE view mode where `**bold**` shows as **bold** (markers
hidden), toolbar + markdown still work, and the file on disk stays markdown. NOT
a read-only preview, NOT replacing the source editor — an *added* editable mode.
They then said: if it's a pain, PARK it. Parked.

**Built + kept for later:** `src/richtext.rs` — pure markdown↔formatting-spans
converter, 7 round-trip tests proving it's LOSSLESS (critical: content syncs to
the phone; a lossy convert would corrupt notes). `#![allow(dead_code)]` on it.
The GTK bridge (tags↔buffer) was written then removed; rebuild it from the
converter when doing the dual view. The hard part remaining is GTK live-edit
behaviour (cursor at markers, typing at boundaries), which is why it's parked.

---

## 2026-07-21 (round 5) — trash retention sweep

Implemented the requested "delete after N days" trash setting.
`Store::sweep_trash(retention_days)` hard-deletes trashed notes whose
`trashedAt` is older than the window; `retention_days == 0` is a no-op ("keep
forever"), so nobody loses trash without opting in. Runs **once at launch** in
`window::build`, before the list is built — deliberately NOT on setting-change,
to avoid a surprising mass-delete the instant you pick "7 days" with old trash.
Config gains `trash_retention_days: u32` (default 0) + `TRASH_RETENTION_OPTIONS`
menu constant and index↔days helpers. Settings adds an "Empty trash" ComboRow
(Never / 7 / 30 / 90 days) applied on dialog close. 92 tests (4 new). Launch
render clean, no regression. User does interaction testing per usual.

---

## 2026-07-21 (round 6) — backup export

New `src/backup.rs`: `export(data_dir, dest)` zips the ENTIRE data dir (notes/,
notes-md/, anything else) into a `.zip`, entry paths relative to data_dir so an
unzip reproduces the vault layout. Iterative dir walk (no recursion blowup),
skips unreadable entries rather than aborting. `suggested_filename(date)` →
`notes-backup-YYYY-MM-DD.zip` (date from `model::iso8601(now)[..10]`, no date
crate). Uses the existing `zip` v2 dep (`ZipWriter` + `SimpleFileOptions`,
Deflated). ⋮ menu → "Back up all notes…" → `FileDialog::save` with dated
initial_name → toast with file count. 3 tests incl. a real round-trip that
re-opens the archive and asserts bytes+paths survive, Greek filename included,
and an empty vault still makes a valid (0-entry) zip. 95 tests total. Release
binary rebuilt. Did NOT zip the user's real vault even in scratchpad — no copies
of sensitive notes; the Greek round-trip test covers the real-data risk.

---

## 2026-07-21 (round 7) — wrapping format toolbar (WrapBox)

Replaced the format toolbar's horizontal `ScrolledWindow` with a wrapping bar so
no button hides behind a scroll on a narrow editor.

**Dead end worth remembering: `gtk::FlowBox` is NOT a wrap box.** It's a *grid* —
every cell gets the widest child's width, and each vertical separator eats a full
column while rendering as a thin line. Result: it wrapped early even with room to
spare (13/14 buttons, one lonely on row 2), regardless of `halign`. Don't reach
for FlowBox to wrap a toolbar.

**Fix: `adw::WrapBox`** (libadwaita 1.7). Packs variable-width children tightly,
so a wide editor keeps all 14 on one row and a narrow one wraps to 2–3. Needed
bumping the `libadwaita` crate feature `v1_5` → `v1_7` in Cargo.toml (crate 0.7.2
already binds it; system lib is 1.9). API: `WrapBox::builder().child_spacing()
.line_spacing().justify(adw::JustifyMode::None)`, `.append(&child)`. Separators
are cheap here (natural width), so they're back for group dividers.

**Xvfb render gotcha that cost time:** `tools/shoot.sh`'s `WxH` arg sizes the
*Xvfb screen*, NOT the app window. There's no window manager on `:99`, so the
window always opens at its `default_width` (1100) — a bigger screen just adds
black around it, a smaller one clips it. To render the app at a different WIDTH,
temporarily change `default_width` in `build()`, render, then revert. At the 1100
default the editor pane is only ~540px (outer sidebar 220 + list min 340), so the
toolbar shows 2 rows by default — that's expected, and still better than the old
scroll that hid the link button. 95 tests unchanged (UI-only change).

---

## 2026-07-21 (round 8) — .deb packaging

`cargo install cargo-deb` (v3.7.0), then `cargo-deb` →
`target/debian/simple-notes-desktop_0.1.0-1_amd64.deb` (1.9 MB). The existing
`[package.metadata.deb]` assets were STALE: referenced
`data/dev.dettmer.SimpleNotesDesktop.desktop` (deleted in the rename to "Notes")
and shipped no icons. Fixed the asset list:
- desktop file → `data/dev.local.Notes.desktop`
- app PNGs (48–512) → `usr/share/icons/hicolor/<size>/apps/` so the **launcher**
  resolves `Icon=dev.local.Notes`.
- the whole bundled icon tree (index.theme + symbolic action SVGs + app PNGs) →
  `usr/share/simple-notes-desktop/icons/hicolor/…` — the EXACT path
  `install_icons()` in main.rs reads at runtime (it checks that installed path
  first, then falls back to `CARGO_MANIFEST_DIR/data/icons` for `cargo run`).
  Without this the toolbar/sidebar glyphs would be blank on an installed copy.

cargo-deb glob assets (`.../apps/*.png`) map each glob to one dest dir — used
per-size rather than one recursive `**` (structure-preservation is ambiguous).
Auto-deps resolved right: libadwaita ≥1.7, gtk4 ≥4.9, gtksourceview5, libc6, etc.

**Did NOT install it** — no passwordless sudo, and installing a system package is
the user's to run. Handed off the file + `sudo apt install ./<deb>` (apt, not
`dpkg -i`, so deps resolve). Test steps in test-list.md section D.

---

## 2026-07-21 (round 9) — user testing bugs: server, rename, header/toolbar

User ran the deb and reported 6 things. Fixed all:

1. **Automatic server made a NEW server on every settings reopen.** Root: the
   auto-tab handler regenerated port (`find_available_port`) / creds on each
   toggle-on. Now the config is generated + saved EXACTLY ONCE (guard: empty
   password = not-yet-configured); afterwards it's reused verbatim so the phone's
   saved address never changes. Only Advanced edits it. Added a **Reset server
   credentials** button (destructive) in Advanced = the "delete it" path.
2. **Trash retention default 0 → 30 days** (`Config::default`). Most-apps default.
3. **Full rename to "Notes".** Package/binary `simple-notes-desktop` → `notes`
   (Cargo `name`, `[lib] name`, deb assets, desktop `Exec`, shoot.sh, README,
   `/usr/share/notes/icons`). Config/data dirs → `~/.config/Notes` +
   `~/.local/share/Notes` with **one-time migration** (`store::migrate_legacy_dirs`
   renames the old folders at startup if the new ones don't exist; window::build
   repoints a stored `data_dir` that still pointed at the old default). APP_ID
   stays `dev.local.Notes`. NOTE: installing `notes` needs the old
   `simple-notes-desktop` package removed first — both ship the same
   `dev.local.Notes.desktop` path.
4. **Toolbar clipped on narrow windows, didn't wrap.** DIAGNOSIS (via a temporary
   width-probe that walked parents printing `measure().0`): the WrapBox was fine
   (min 68). The **editor HEADER** forced the editor's min width to **~627px**, so
   the whole app needed ~1187px or the header+toolbar clipped. Fix: moved the app
   menu OUT of the editor header into the **list** header's ⋮ (merged into
   `list_menu`, now always visible), and folded colour+move+archive into ONE
   "more actions" overflow MenuButton (the existing button widgets, just
   reparented — wiring untouched). Editor now shrinks to a ~360px pane; toolbar
   wraps instead of clipping. Verified by render at 560px (wraps, header fits) and
   1100 default (3 panes fit, toolbar 10+4).

**Render-harness fact (important):** `tools/shoot.sh`'s `WxH` sizes the Xvfb
SCREEN, not the window — no WM on :99/:101, so the app opens at its
`default_width`. To render at a width, temporarily edit `default_width`, render,
revert. The app auto-selects the first note, so the editor is visible in
phone/tablet mode for narrow renders.

95 tests pass. Rebuilt `target/debian/notes_0.1.0-1_amd64.deb`.

---

## 2026-07-21 (round 10) — verified empty-state + import; FIXED delete sync

**Verified (isolated render, no real data touched):** ran the app with temp
`XDG_DATA_HOME`/`XDG_CONFIG_HOME` (the app respects them via `dirs`). Empty app →
"No note selected"; an empty note → the Markdown cheat-sheet hint renders
correctly. Obsidian import of `tests/fixtures/vault` via a new
`examples/import_check.rs` (headless scan→commit): 5 notes / 2 Greek folders /
google-keep tag / checklist preview all render; re-import idempotent (0 added).

**BIG sync finding + fix.** The Android app propagates deletions via a shared
ledger `notes/deletions.json` (`DeletionSyncManager`, format
`{version:1, deletedNotes:[{id, deletedAt, deviceId}]}`; notes URL = `<base>/notes/`,
default sync folder "notes"). Our desktop app was NOT writing it, so a hard
delete on the PC left no tombstone → **the phone would RESURRECT the note**
(zombie) on next sync. Fixed: `Store::delete` now calls new
`Store::record_deletion(id)` which appends a tombstone to `notes/deletions.json`
in that exact format, using `store::device_id()`. `collect()` skips
`deletions.json` so it isn't parsed as a note. empty_trash + sweep_trash go
through `delete()`, so they tombstone too. 1 test. Trashing (trashedAt) already
syncs safely (file updated, not removed) — so BOTH trash and hard-delete now
converge on the phone. Not pruning old ledger entries yet (upstream warns >1000).

96 tests pass.

**Shipped an earlier turn:** the empty-body placeholder is now a **Markdown cheat
sheet** (bold/italic/strike/code/headings/lists/link/quote), dim, non-clickable
(`can_target(false)`), toggled by buffer emptiness — helps the user learn syntax
while typing. Title placeholder is "Note title". 89 tests pass.

**Reminder for next session:** the user runs an OLD binary until they `cargo run
--release`. Editing source doesn't change a running app — this caused a "why
isn't it working" confusion (window still said "Simple Notes" pre-rename). And:
**do not GUI-test with xdotool** — the user does the testing; only run `cargo
test` + offscreen renders to sanity-check layout.

---

## 2026-07-22 — beta packaging + user-testing rounds (beta1→beta5)

Now shipping the user `.deb` betas (`notes_0.0.1~beta.N-1_amd64.deb` via
`cargo-deb`); they install and test on their real desktop (a WM, unlike Xvfb).
Version is `0.0.1-beta.N` → Debian `0.0.1~beta.N` (sorts before the eventual
0.0.1). Each round = user reports bugs, I fix, rebuild the beta. **97 tests pass.**

**Adaptive layout — the big lesson: `AdwWindow` applies only ONE breakpoint at a
time.** A header breakpoint at the same `MaxWidth` as the tablet one *shadowed*
the pane-collapse entirely — nothing collapsed, the editor just overflowed. Fix:
give each concern a disjoint range. Final thresholds (set by the user, measured in
PixelRuller): **phone < 900, tablet 900–1199, desktop ≥ 1200**; window floor
**500×500** (`width_request`/`height_request`); default opens 1280 (3-pane).
Collapse via two `MaxWidth` breakpoints (tablet 1199 → inner collapsed; phone 899
→ both collapsed, added AFTER tablet so it wins the overlap). The editor header's
inline-vs-⋮ overflow is driven by a DISJOINT `MinWidth 1200` desktop breakpoint
(apply→inline, unapply→⋮; built default = ⋮), reparenting `overflow_box` between
an inline slot and the ⋮ popover via `connect_apply`/`connect_unapply`. Verified
collapse by render at 750/1000/1280.

**Xvfb gotcha reconfirmed:** no WM on `:99`, so the window can't shrink below its
content's natural width — `default_width` only sets the *open* size; to test a
width, temporarily edit `default_width`, render, revert. Small-width collapse is
only truly testable on the user's machine.

**Multi-select was broken:** the row `CheckButton` had NO toggle handler, so
`checked` stayed empty → "0 selected", bulk actions were no-ops, "can't select
pinned". Fix: checkbox `connect_toggled` is the source of truth; clicking the row
body toggles the checkbox (routes through it). Added a **select-all/none**
`CheckButton` to the bulk bar (`connect_bulk` gained a param).

**Tags & search scope widened (user reports):** tags now span everything EXCEPT
trash — archived notes keep their tags, so `all_tags()` and `Filter::Tag` include
archived (were excluding it → count read 0). Search from **Home** is GLOBAL when a
query is present (`global_search` guard in `visible_notes`): matches archived +
trashed too, since you search when you don't know where a note lives. Other views
keep search scoped.

**Folders are now EXCLUSIVE:** a foldered note no longer shows in Home
(`Filter::Notes` also requires `folder_name` empty) — it lives only in its folder,
under its tags, and in a global search. This reverses the earlier "foldered notes
also appear in All notes is correct" decision — the user found it pointless.
Removed the editor's move-to-folder button (duplicated bulk select-mode move);
deleted `build_move_popover` + `move_selected_to_folder`. Folder creation stays in
the list header (+ ⋮ menu).

**"All notes" → "Home"** everywhere (sidebar row, list title, default-view
setting) — with folders exclusive, "All notes" was a lie.

**Trash view fixes:** every note shows a trash icon (not a pin); pinning no longer
reorders trash (sort by recency only); a **Restore** button (`restore_button`,
labelled, in the editor header pack_start) appears for a trashed note and clears
`trashedAt`; the trash button's tooltip becomes "Delete permanently". `note_row`
gained an `in_trash: bool` param.

**Empty-state guide:** the empty editor pane is now a "Welcome to Notes"
StatusPage with a `child` guide (adw::Clamp + markup Label) covering the editor
bar and list bar tools — first-run help. Kept accurate to the CURRENT toolbar (no
folder button).

**Delete-sync tombstone (carried from round 10, in every beta):** `Store::delete`
writes `notes/deletions.json` so the phone removes rather than resurrects.

**License flag, unresolved:** repo `kalotrapezis/Notes` is AGPL-3.0 (matches
Cargo.toml). GitHub push NOT done yet — waiting for the user to bless a build as
v0.0.1, then README (feature cards + screenshots + problem statement) + first
release. Ask before pushing (public publish).
