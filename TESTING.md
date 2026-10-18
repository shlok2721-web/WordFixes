# Manual test checklist

Run the app:

```bash
cd "Simple notes desktop (server)"
cargo run --release      # or: ./target/release/simple-notes-desktop
```

Your notes live in `~/.local/share/simple-notes-desktop/` by default. To test
against a throwaway folder instead, set it in **⋮ → Server & Sync… → Notes
folder** is read-only there — edit `~/.config/simple-notes-desktop/config.json`
(`"data_dir"`) if you want an isolated sandbox. Otherwise it uses your real
notes, which is fine — nothing here destroys data (delete = trash).

Tick each item. Anything that fails, tell me the step number + what happened.

---

## 1. Notes basics
- [ ] **1.1** Click **+** (top-left of the note list) → a new blank note opens, cursor in the title.
- [ ] **1.2** Type a title → it appears in the list immediately.
- [ ] **1.3** Type in the body → text stays as you type (no dropped characters), "Saved" shows in the header.
- [ ] **1.4** Click another note, then back → both notes kept their own text.
- [ ] **1.5** Quit and relaunch → your notes are still there.

## 2. Markdown highlighting (new)
- [ ] **2.1** In a note body type: `# Heading`, `**bold**`, `_italic_`, `` `code` ``, `- list`, `> quote`, `[link](http://x)`.
- [ ] **2.2** Each is highlighted **as you type** (headings coloured, bold looks bold, etc.).
- [ ] **2.3** Switch your desktop light/dark → the editor colours follow (may need the app focused).

## 3. Undo / redo (new)
- [ ] **3.1** Type some text. The **↶ undo** button (top-left of editor) becomes active.
- [ ] **3.2** Click undo → your last typing is removed.
- [ ] **3.3** Click **↷ redo** → it comes back.
- [ ] **3.4** `Ctrl+Z` / `Ctrl+Shift+Z` also work.

## 4. Colour (new)
- [ ] **4.1** Click the **colour** button (eyedropper icon) → a 4×3 grid of pastel swatches opens.
- [ ] **4.2** Pick a colour → the note's row in the list is tinted that colour.
- [ ] **4.3** Reopen the picker, click the **clear** button (top-left of the grid) → colour removed.

## 5. Pin & Archive
- [ ] **5.1** Click **pin** → note jumps to the top of the list; **Pinned** in the sidebar shows it.
- [ ] **5.2** Click **archive** (the eye/conceal icon) → note leaves the main list; **Archived** shows it.
- [ ] **5.3** In **Archived**, open the note, click archive again → it returns to the main list.

## 6. Trash
- [ ] **6.1** Click **trash** → note leaves the list; **Trash** shows it.
- [ ] **6.2** In **Trash**, open the note, click trash again → it's permanently deleted (gone from Trash).

## 7. Folders
- [ ] **7.1** Click **new folder** (folder+ icon, top-right of note list) → name it → it appears under **Folders** in the sidebar.
- [ ] **7.2** With a note open, click the **move-to-folder** button (folder icon in the editor header) → pick the folder → toast "Moved to …".
- [ ] **7.3** Click that folder in the sidebar → the note is there.
- [ ] **7.4** Move it back via **No folder** → it leaves the folder.
- [ ] **7.5** Create a note **while inside a folder** → it belongs to that folder.

## 8. Checklists
- [ ] **8.1** **+ → New checklist** → a checklist note opens with one empty item.
- [ ] **8.2** Type an item, press Enter / "Add item" → more items.
- [ ] **8.3** Tick a checkbox → the item sinks below a "N completed" divider with a strikethrough.
- [ ] **8.4** Untick → it comes back up.
- [ ] **8.5** Remove an item with its × .

## 9. Tags (new)
- [ ] **9.1** With a note open, type a tag in **"Add tag…"** (under the title) + Enter → a chip appears.
- [ ] **9.2** The tag shows under **Tags** in the sidebar with a count.
- [ ] **9.3** Click that tag in the sidebar → only notes with it are listed.
- [ ] **9.4** Remove the chip with its × → tag gone; sidebar count drops (or the tag disappears if it was the last one).

## 10. Search
- [ ] **10.1** Type in the search box → the list narrows to matches (title, body, checklist items, tags).
- [ ] **10.2** Search Greek text with different case → still matches.
- [ ] **10.3** Two words → only notes containing **both** match.
- [ ] **10.4** Clear search → full list returns.

## 11. Multi-select / bulk
- [ ] **11.1** Click the **select** toggle (top-right of note list) → checkboxes appear on rows; a bulk bar appears.
- [ ] **11.2** Tick several → "N selected" updates.
- [ ] **11.3** Bulk **archive** → all selected are archived at once.
- [ ] **11.4** In **Archived**, select several, click the bulk button (now a **restore** icon) → they return.
- [ ] **11.5** Bulk **move to folder** → pick a folder → all selected move.
- [ ] **11.6** Bulk **trash** → all selected go to Trash.

## 12. Import your Keep notes (the big one)
- [ ] **12.1** **⋮ → Import notes → Google Keep export (.zip)** → pick your Takeout `.zip`.
- [ ] **12.2** A dialog says roughly "Found 161 notes (161 new)". Click **Import**.
- [ ] **12.3** Toast: "Imported 161 new". Notes, colours, tags, checklists all appear.
- [ ] **12.4** **Import the same zip again** → dialog says "0 new" (or all "already imported"); no duplicates created.
- [ ] **12.5** Greek-titled notes are readable in `~/.local/share/simple-notes-desktop/notes-md/` (not `<uuid>.md`).

## 13. Sync server (test with your phone)
- [ ] **13.1** **⋮ → Server & Sync…** → set a password → **Listen on: All interfaces** → toggle the server **on**.
- [ ] **13.2** The dialog lists a URL like `http://192.168.x.x:8080/`.
- [ ] **13.3** In the Android app: Settings → server URL + username `noteuser` + your password → sync.
- [ ] **13.4** A note made on the phone appears on the desktop **without restarting** (file watcher).
- [ ] **13.5** A note edited on the desktop shows up on the phone after its sync.

---

## Known-not-done (don't test these — not built yet)
- Formatting toolbar (Bold/Italic buttons) — highlighting works, buttons don't exist.
- Checklist drag-to-reorder.
- Image / attachment sync (Keep image filenames are stored but images aren't copied).
- `.deb` packaging.
- Undo covers the note **body** only — not tag/title/colour changes (those have their own controls).
