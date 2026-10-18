# Changelog

All notable changes to Notes are documented here.

## [0.0.2] — 2026-07-23

### Added

- Editable **Live Preview** for Markdown. Headings, bold, italic,
  strikethrough and code render without syntax markers while the stored note
  remains Markdown.
- Formatting shortcuts in Live Preview: `Ctrl+B`, `Ctrl+I`, `Ctrl+S`, and
  `Ctrl+1`/`2`/`3` for headings.
- Three focused Settings pages: Automatic, Server and Storage.
- Named server profiles tied to the active network. Profiles can be applied
  manually or selected automatically when the network changes.
- Optional background keep-alive so phone sync continues after the window is
  closed; reopening Notes raises the same instance.
- A tag-to-folder converter with a dry-run summary and automatic backup.
  Already-foldered notes are never moved again.
- Note-list sorting by updated date, created date, title, type or colour, in
  either direction.
- Checklist sorting by completion, alphabetic order or creation time.
- Exact build identification in **About Notes**.

### Changed

- Settings are available with `Ctrl+,`.
- Live Preview now sits beside Pin with the other note-level controls.
- Markdown and Google Keep imports preserve multiline checklist items and
  offer explicit organisation choices.
- Bulk editing now includes pin and colour operations.

### Fixed

- Rapid Enter presses in checklists no longer lose items.
- Checklist rows retain compact, visible text without intermittent blank
  spacing or clipped editors.
- The tag converter respects folders chosen manually.
- Background mode and single-instance relaunch no longer interrupt the sync
  server.

Validation: 241 automated tests pass, with two environment-dependent tests
ignored.

## [0.0.1] — 2026-07-22

First public release: native GTK4 note editing, folders, tags, search,
checklists, colours, bulk actions, Keep/Markdown import, backup/restore, trash,
and an embedded WebDAV server compatible with Simple Notes Sync on Android.

[0.0.2]: https://github.com/kalotrapezis/Notes/compare/v0.0.1...v0.0.2
[0.0.1]: https://github.com/kalotrapezis/Notes/releases/tag/v0.0.1
