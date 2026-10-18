# test-list.md — what to test together

Focused checklist for the **recent** changes. For the full regression pass see
[TESTING.md](TESTING.md).

Run fresh code first:

```bash
cd "Simple notes desktop (server)"
cargo run --release
```

Tick each item. Anything that fails → tell me the item number + what happened.
Legend: `[y]` pass · `[x]` fail · `[?]` untested · `[ ]` needs retest.

---

## A. Trash retention
- [y] **A.1** ☰ menu → **Server & Sync…** → the storage section has an **"Empty trash"** dropdown.
- [ ] **A.2** It now defaults to **After 30 days** (changed from Never); options are Never / 7 / 30 / 90 days.
- [y] **A.3** Pick a value, close the dialog, reopen it → the choice stuck.

## B. Backup export
- [y] **B.1** ☰ menu → **Back up all notes…** → save dialog pre-filled `notes-backup-YYYY-MM-DD.zip`.
- [y] **B.2** Save it → toast "Backed up N files to …".
- [y] **B.3** Open the `.zip` → `notes/` + `notes-md/`, folders preserved, Greek filenames intact.
- [y] **B.4** Cancel the save dialog → no toast, nothing written, no crash.

## C. Editor header + toolbar (REWORKED — retest)
The toolbar clipping was caused by a too-wide editor header. Fixed by moving the
app menu to the **list** header and folding colour/move/archive into one **⋮ "more
actions"** button in the editor header.
- [ ] **C.1** The **list** header (above the note list) now has a **☰ menu** with Import / Back up / Server & Sync / Open notes folder / Empty trash.
- [ ] **C.2** The **editor** header is now: undo, redo … Saved, pin, **⋮ (more actions)**, trash. Clicking **⋮** opens colour swatches, move-to-folder, and archive.
- [ ] **C.3** Maximise → the format toolbar sits on **one row**.
- [ ] **C.4** Shrink the window narrow → the toolbar **wraps onto 2–3 rows** and **nothing is cut off** (previously the right-hand buttons were clipped).
- [ ] **C.5** Every toolbar button still works after wrapping (select text, click **B** → `**…**`).

## E. Server config saved once (Automatic tab)
- [ ] **E.1** In **Automatic**, turn the server on → it generates a name/password/port and connects.
- [ ] **E.2** Close Settings, reopen it → the **same** name/password/port are shown; it did **not** make a new server.
- [ ] **E.3** Toggle off/on again → still the same details (phone keeps syncing to the same address).
- [ ] **E.4** Advanced tab → **Reset server credentials** stops the server and clears them; next Automatic on generates fresh ones.

## F. Renamed to "Notes" + data migration
- [ ] **F.1** Nothing user-facing says "simple notes desktop" anymore (window title, Settings → Notes folder path, About).
- [ ] **F.2** Settings → **Notes folder** now shows `~/.local/share/Notes`.
- [ ] **F.3** **Your existing notes are all still there** after first launch of the renamed build (they were auto-moved from the old folder).

## D. .deb install (new package name)
**Remove the old package first** — it shipped the same desktop-file path:
```bash
sudo apt remove simple-notes-desktop        # if it was ever installed
sudo apt install "./target/debian/notes_0.1.0-1_amd64.deb"
```
- [ ] **D.1** Install completes with no dependency or file-conflict errors.
- [ ] **D.2** "Notes" appears in the launcher with its blue note icon.
- [ ] **D.3** Launch from the menu → window opens; sidebar/toolbar icons all render.
- [ ] **D.4** Your notes are intact (migrated to `~/.local/share/Notes`).
- [ ] **D.5** Uninstall cleanly: `sudo apt remove notes`.

---

## Regression spot-check (should be unchanged)
- [ ] **R.1** Create a note, type body → no dropped characters, "Saved" appears.
- [ ] **R.2** Pin a note → it jumps to the Pinned section.
- [ ] **R.3** Search box filters the list.
- [ ] **R.4** Quit and relaunch → notes still there.
