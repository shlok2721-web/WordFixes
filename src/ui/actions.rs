//! Signal wiring for the main window.

use super::window::{note_row, server_hint, toast, Filter, Shared, Widgets};
use crate::model::Note;
use crate::server;
use adw::prelude::*;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyboardAction {
    Delete,
    Archive,
    BulkEdit,
    Colour,
    Search,
    Escape,
    NewChecklistItem,
}

/// Map shortcuts independently of GTK widgets so the conflict rules are easy
/// to test. Ctrl+A/Ctrl+E and Delete retain normal editing behaviour in text
/// fields; their Ctrl+Shift alternatives are deliberately global.
fn keyboard_action(
    key: gtk::gdk::Key,
    modifiers: gtk::gdk::ModifierType,
    text_focused: bool,
) -> Option<KeyboardAction> {
    let ctrl = modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK);
    let shift = modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK);
    if modifiers.contains(gtk::gdk::ModifierType::ALT_MASK) {
        // Ctrl+Alt+N works even while typing in an item entry — that's the
        // point of it. The handler ignores it unless a checklist note is open,
        // so it can't damage a text note or an unrelated entry.
        if ctrl && key.to_unicode().map(|c| c.to_ascii_lowercase()) == Some('n') {
            return Some(KeyboardAction::NewChecklistItem);
        }
        return None;
    }
    if key == gtk::gdk::Key::Delete {
        return (!text_focused).then_some(KeyboardAction::Delete);
    }
    if key == gtk::gdk::Key::Escape {
        return Some(KeyboardAction::Escape);
    }

    let letter = key.to_unicode().map(|c| c.to_ascii_lowercase());
    match (ctrl, shift, letter) {
        (true, true, Some('c')) => Some(KeyboardAction::Colour),
        (true, true, Some('a')) => Some(KeyboardAction::Archive),
        (true, true, Some('e')) => Some(KeyboardAction::BulkEdit),
        (true, false, Some('a')) if !text_focused => Some(KeyboardAction::Archive),
        (true, false, Some('e')) if !text_focused => Some(KeyboardAction::BulkEdit),
        (true, false, Some('f')) => Some(KeyboardAction::Search),
        _ => None,
    }
}

fn text_widget_has_focus(window: &adw::ApplicationWindow) -> bool {
    let mut current = gtk::prelude::RootExt::focus(window);
    while let Some(widget) = current {
        // SearchEntry and SourceView are subclasses of Entry and TextView.
        if widget.is::<gtk::Entry>() || widget.is::<gtk::TextView>() {
            return true;
        }
        current = widget.parent();
    }
    false
}

#[cfg(test)]
mod keyboard_shortcut_tests {
    use super::{keyboard_action, KeyboardAction};
    use gtk::gdk::{Key, ModifierType};

    #[test]
    fn editing_keeps_select_all_and_delete_for_text() {
        assert_eq!(
            keyboard_action(Key::a, ModifierType::CONTROL_MASK, true),
            None
        );
        assert_eq!(
            keyboard_action(Key::Delete, ModifierType::empty(), true),
            None
        );
    }

    #[test]
    fn shifted_alternatives_work_even_while_editing() {
        let ctrl_shift = ModifierType::CONTROL_MASK | ModifierType::SHIFT_MASK;
        assert_eq!(
            keyboard_action(Key::A, ctrl_shift, true),
            Some(KeyboardAction::Archive)
        );
        assert_eq!(
            keyboard_action(Key::E, ctrl_shift, true),
            Some(KeyboardAction::BulkEdit)
        );
        assert_eq!(
            keyboard_action(Key::C, ctrl_shift, true),
            Some(KeyboardAction::Colour)
        );
    }

    #[test]
    fn ctrl_alt_n_fires_everywhere_but_other_alt_combos_stay_dead() {
        let ctrl_alt = ModifierType::CONTROL_MASK | ModifierType::ALT_MASK;
        // Works both while typing (the common case: you're in an item entry)
        // and from the list.
        for focused in [true, false] {
            assert_eq!(
                keyboard_action(Key::n, ctrl_alt, focused),
                Some(KeyboardAction::NewChecklistItem)
            );
        }
        // Alt still suppresses everything else.
        assert_eq!(keyboard_action(Key::a, ctrl_alt, false), None);
        assert_eq!(
            keyboard_action(Key::n, ModifierType::ALT_MASK, false),
            None
        );
    }

    #[test]
    fn list_context_enables_the_requested_shortcuts() {
        assert_eq!(
            keyboard_action(Key::a, ModifierType::CONTROL_MASK, false),
            Some(KeyboardAction::Archive)
        );
        assert_eq!(
            keyboard_action(Key::e, ModifierType::CONTROL_MASK, false),
            Some(KeyboardAction::BulkEdit)
        );
        assert_eq!(
            keyboard_action(Key::Delete, ModifierType::empty(), false),
            Some(KeyboardAction::Delete)
        );
    }
}

/// Refreshing the entire list synchronously from `Entry::changed` or
/// `TextBuffer::changed` tears down and recreates GTK rows during the input
/// method's own key handling. Besides being expensive, that can disrupt
/// dead-key/compose sequences such as Greek tonos on Wayland. Coalesce rapid
/// edits and rebuild only after the input callback has returned.
fn schedule_list_refresh(state: &Shared, widgets: &Rc<Widgets>) {
    {
        let mut s = state.borrow_mut();
        if s.list_refresh_pending {
            return;
        }
        s.list_refresh_pending = true;
    }

    let (state, widgets) = (state.clone(), widgets.clone());
    glib::timeout_add_local_once(Duration::from_millis(120), move || {
        state.borrow_mut().list_refresh_pending = false;
        refresh_list(&state, &widgets);
    });
}

pub fn connect(
    state: &Shared,
    widgets: &Rc<Widgets>,
    new_button: &adw::SplitButton,
    delete_button: &gtk::Button,
) {
    let pin_button = &widgets.pin_button;
    let archive_button = &widgets.archive_button;
    // --- keyboard shortcuts ------------------------------------------------
    {
        let (state, widgets, delete_button) =
            (state.clone(), widgets.clone(), delete_button.clone());
        let window = widgets.window.clone();
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(action) =
                keyboard_action(key, modifiers, text_widget_has_focus(&widgets.window))
            else {
                return glib::Propagation::Proceed;
            };
            match action {
                KeyboardAction::Delete => {
                    if state.borrow().selected.is_none() {
                        return glib::Propagation::Proceed;
                    }
                    delete_button.emit_clicked();
                }
                KeyboardAction::Archive => {
                    if state.borrow().selected.is_none() || !widgets.archive_button.is_sensitive() {
                        return glib::Propagation::Proceed;
                    }
                    widgets
                        .archive_button
                        .set_active(!widgets.archive_button.is_active());
                }
                KeyboardAction::BulkEdit => {
                    widgets
                        .select_toggle
                        .set_active(!widgets.select_toggle.is_active());
                }
                KeyboardAction::Colour => {
                    if state.borrow().selected.is_none() {
                        return glib::Propagation::Proceed;
                    }
                    widgets.color_button.popup();
                }
                KeyboardAction::Search => {
                    widgets.search.grab_focus();
                }
                KeyboardAction::NewChecklistItem => {
                    // Only meaningful with a checklist note open; never converts
                    // a text note into a checklist.
                    let is_checklist = state
                        .borrow()
                        .selected_note()
                        .is_some_and(|n| n.checklist_items.is_some());
                    if !is_checklist {
                        return glib::Propagation::Proceed;
                    }
                    add_checklist_item(&state, &widgets);
                }
                KeyboardAction::Escape => {
                    if widgets.select_toggle.is_active() {
                        widgets.select_toggle.set_active(false);
                    } else if !widgets.search.text().is_empty() {
                        widgets.search.set_text("");
                        widgets.search.grab_focus();
                    } else {
                        return glib::Propagation::Proceed;
                    }
                }
            }
            glib::Propagation::Stop
        });
        window.add_controller(keys);
    }
    // --- sidebar selection ---
    // Collapsing or expanding a split view at a breakpoint remaps the sidebar,
    // and GTK restores focus by advancing the cursor one row. In
    // SelectionMode::Single a cursor move *is* a selection, so resizing used to
    // walk the filter down a row at a time (Home → Pinned → Archived). GTK
    // exposes no way to tell those apart after the fact, so mark the selections
    // we asked for — a click, a key press, or `refresh_sidebar` — and treat an
    // unmarked one as GTK's own bookkeeping: snap the highlight back to what the
    // app state says and change nothing.
    {
        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let s = state.clone();
        click.connect_pressed(move |_, _, _, _| s.borrow_mut().intent = true);
        widgets.sidebar.add_controller(click);

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let s = state.clone();
        keys.connect_key_pressed(move |_, _, _, _| {
            s.borrow_mut().intent = true;
            glib::Propagation::Proceed
        });
        widgets.sidebar.add_controller(keys);
    }
    {
        let (state, widgets) = (state.clone(), widgets.clone());
        widgets.clone().sidebar.connect_row_selected(move |lb, row| {
            let Some(row) = row else { return };
            if state.borrow().rebuilding {
                return;
            }
            let Some(raw) =
                unsafe { row.data::<String>("target") }.map(|p| unsafe { p.as_ref().clone() })
            else {
                return;
            };
            if !std::mem::take(&mut state.borrow_mut().intent) {
                let want = state.borrow().active_target();
                if want != super::window::SidebarTarget::decode(&raw) {
                    if let Some(r) = row_for_target(lb, &want) {
                        state.borrow_mut().intent = true;
                        lb.select_row(Some(&r));
                    }
                }
                return;
            }
            let selection_cleared = {
                let mut s = state.borrow_mut();
                s.apply_target(&super::window::SidebarTarget::decode(&raw));
                s.clear_selection_if_hidden()
            };
            refresh_list(&state, &widgets);
            if selection_cleared {
                load_editor(&state, &widgets);
            }
            // On a collapsed layout, picking a section brings the list forward.
            widgets.outer_split.set_show_content(true);
        });
    }

    // --- adaptive relayout ---------------------------------------------------
    // Collapsing or expanding a split view remaps the sidebar, and GTK restores
    // focus by advancing the cursor one row. In SelectionMode::Single a cursor
    // move *is* a selection, so every breakpoint crossing walked the filter one
    // row down (Home → Pinned → Archived). Same reasoning as `rebuilding`:
    // widget churn is not user intent. Ignore selections mid-relayout, then put
    // the real one back. Covers the note list too — it checks the same flag.
    for split in [&widgets.outer_split, &widgets.inner_split] {
        let (state, widgets) = (state.clone(), widgets.clone());
        split.connect_collapsed_notify(move |_| {
            state.borrow_mut().rebuilding = true;
            let (state, widgets) = (state.clone(), widgets.clone());
            glib::idle_add_local_once(move || {
                state.borrow_mut().rebuilding = false;
                refresh_sidebar(&state, &widgets);
            });
        });
    }

    // --- note selection ---
    {
        let (state, widgets) = (state.clone(), widgets.clone());
        widgets.clone().list.connect_row_selected(move |_, row| {
            // Rebuilding the list removes rows, and GTK reports that as a
            // deselection. Acting on it would wipe the real selection.
            if state.borrow().rebuilding {
                return;
            }
            if state.borrow().selecting {
                // Multi-select: clicking the row body toggles the row's checkbox,
                // whose own handler updates `checked` and the count. (Clicking the
                // checkbox directly does the same thing without coming through
                // here.) The checkbox is the single source of truth.
                if let Some(check) = row
                    .and_then(|r| r.child())
                    .and_downcast::<gtk::Box>()
                    .and_then(|b| b.first_child())
                    .and_downcast::<gtk::CheckButton>()
                {
                    check.set_active(!check.is_active());
                }
                return;
            }
            let id = row.and_then(|r| {
                unsafe { r.data::<String>("note-id") }.map(|p| unsafe { p.as_ref().clone() })
            });
            state.borrow_mut().selected = id;
            load_editor(&state, &widgets);
        });
    }

    // --- new note (the split button's primary click) ---
    {
        let (state, widgets) = (state.clone(), widgets.clone());
        new_button.connect_clicked(move |_| {
            create_note(&state, &widgets, crate::model::NoteType::TEXT);
        });
    }

    // --- title edits ---
    {
        let (state, widgets) = (state.clone(), widgets.clone());
        widgets.clone().title_entry.connect_changed(move |entry| {
            if state.borrow().loading {
                return;
            }
            let text = entry.text().to_string();
            let saved = {
                let mut s = state.borrow_mut();
                let Some(note) = s.selected_note_mut() else {
                    return;
                };
                if note.title == text {
                    return;
                }
                note.title = text;
                note.touch();
                let note = note.clone();
                s.store.save(&note)
            };
            if let Err(e) = saved {
                eprintln!("failed to save note title: {e}");
                mark_save_failed(&widgets);
                return;
            }
            mark_saved(&widgets);
            schedule_list_refresh(&state, &widgets);
        });
    }

    // --- body edits ---
    {
        let (state, widgets) = (state.clone(), widgets.clone());
        widgets.clone().buffer.connect_changed(move |buffer| {
            if state.borrow().loading {
                return;
            }
            let text = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), false)
                .to_string();
            let saved = {
                let mut s = state.borrow_mut();
                let Some(note) = s.selected_note_mut() else {
                    return;
                };
                if note.content == text {
                    return;
                }
                note.content = text;
                note.touch();
                let note = note.clone();
                s.store.save(&note)
            };
            if let Err(e) = saved {
                eprintln!("failed to save note body: {e}");
                mark_save_failed(&widgets);
                return;
            }
            let has_text = buffer.char_count() > 0;
            widgets.body_placeholder.set_visible(!has_text);
            mark_saved(&widgets);
            schedule_list_refresh(&state, &widgets);
        });
    }

    // Button sensitivity tracks the buffer's own can-undo/can-redo properties.
    // These notify AFTER the undo action commits — unlike polling inside
    // `changed`, which sees the pre-commit state and greys the button forever.
    {
        let undo = widgets.undo_button.clone();
        widgets
            .buffer
            .connect_can_undo_notify(move |b| undo.set_sensitive(b.can_undo()));
    }
    {
        let redo = widgets.redo_button.clone();
        widgets
            .buffer
            .connect_can_redo_notify(move |b| redo.set_sensitive(b.can_redo()));
    }

    // --- pin ---
    {
        let (state, widgets) = (state.clone(), widgets.clone());
        pin_button.connect_toggled(move |button| {
            if state.borrow().loading {
                return;
            }
            let active = button.is_active();
            let (selection_cleared, save_error) = {
                let mut s = state.borrow_mut();
                let Some(note) = s.selected_note_mut() else {
                    return;
                };
                if note.is_pinned == active {
                    return;
                }
                note.is_pinned = active;
                note.touch();
                let note = note.clone();
                match s.store.save(&note) {
                    Ok(()) => {
                        // Re-sort: pinned notes float to the top.
                        s.reload();
                        (s.clear_selection_if_hidden(), None)
                    }
                    Err(e) => {
                        // Restore the last good disk state; never show a pin
                        // change that autosave failed to persist.
                        s.reload();
                        (false, Some(e.to_string()))
                    }
                }
            };
            if let Some(error) = save_error {
                load_editor(&state, &widgets);
                toast(&widgets, &format!("Could not update pin: {error}"));
                return;
            }
            refresh_sidebar(&state, &widgets);
            refresh_list(&state, &widgets);
            if selection_cleared {
                load_editor(&state, &widgets);
            }
        });
    }

    // --- search ---
    {
        let (state, widgets) = (state.clone(), widgets.clone());
        widgets.clone().search.connect_search_changed(move |entry| {
            let text = entry.text().to_string();
            if state.borrow().query == text {
                return;
            }
            state.borrow_mut().query = text;
            refresh_list(&state, &widgets);
            // The selected note may have been filtered out; keep the editor
            // in step with what is actually visible.
            let still_visible = {
                let s = state.borrow();
                s.selected
                    .as_ref()
                    .is_some_and(|id| s.visible_notes().iter().any(|n| &n.id == id))
            };
            if !still_visible {
                state.borrow_mut().selected = None;
                load_editor(&state, &widgets);
            }
        });
    }

    // --- archive / unarchive ---
    {
        let (state, widgets) = (state.clone(), widgets.clone());
        archive_button.connect_toggled(move |button| {
            if state.borrow().loading {
                return;
            }
            let active = button.is_active();
            let (message, selection_cleared, save_error) = {
                let mut s = state.borrow_mut();
                let Some(note) = s.selected_note_mut() else {
                    return;
                };
                if note.is_archived() == active {
                    return;
                }
                note.archived_at = active.then(crate::model::now_millis);
                note.touch();
                let note = note.clone();
                match s.store.save(&note) {
                    Ok(()) => (
                        if active { "Archived" } else { "Unarchived" },
                        s.clear_selection_if_hidden(),
                        None,
                    ),
                    Err(e) => {
                        s.reload();
                        ("", false, Some(e.to_string()))
                    }
                }
            };
            if let Some(error) = save_error {
                load_editor(&state, &widgets);
                toast(
                    &widgets,
                    &format!("Could not update archive state: {error}"),
                );
                return;
            }
            refresh_sidebar(&state, &widgets);
            refresh_list(&state, &widgets);
            if selection_cleared {
                load_editor(&state, &widgets);
            }
            toast(&widgets, message);
        });
    }

    // --- trash / delete ---
    {
        let (state, widgets) = (state.clone(), widgets.clone());
        delete_button.connect_clicked(move |_| {
            let result = {
                let mut s = state.borrow_mut();
                let Some(note) = s.selected_note().cloned() else {
                    return;
                };
                let (result, msg) = if note.is_trashed() {
                    // Already in the trash: this is the permanent delete.
                    (s.store.delete(&note.id), "Note deleted")
                } else {
                    let mut note = note;
                    note.trashed_at = Some(crate::model::now_millis());
                    note.touch();
                    (s.store.save(&note), "Moved to trash")
                };
                s.reload();
                if result.is_ok() {
                    s.selected = None;
                }
                result.map(|()| msg)
            };
            let message = match result {
                Ok(message) => message,
                Err(e) => {
                    load_editor(&state, &widgets);
                    toast(&widgets, &format!("Could not delete note: {e}"));
                    return;
                }
            };
            refresh_sidebar(&state, &widgets);
            refresh_list(&state, &widgets);
            load_editor(&state, &widgets);
            toast(&widgets, message);
        });
    }

    // --- restore from trash ---
    {
        let (state, widgets) = (state.clone(), widgets.clone());
        widgets.clone().restore_button.connect_clicked(move |_| {
            let result = {
                let mut s = state.borrow_mut();
                let Some(mut note) = s.selected_note().cloned() else {
                    return;
                };
                note.trashed_at = None;
                note.touch();
                let result = s.store.save(&note);
                s.reload();
                if result.is_ok() {
                    // The note leaves the trash view, so nothing stays selected here.
                    s.selected = None;
                }
                result
            };
            if let Err(e) = result {
                load_editor(&state, &widgets);
                toast(&widgets, &format!("Could not restore note: {e}"));
                return;
            }
            refresh_sidebar(&state, &widgets);
            refresh_list(&state, &widgets);
            load_editor(&state, &widgets);
            toast(&widgets, "Restored");
        });
    }

    // --- colour swatches (built once; act on the selected note) ---
    build_color_swatches(state, widgets);

    // --- undo / redo ---
    {
        let widgets2 = widgets.clone();
        widgets.clone().undo_button.connect_clicked(move |_| {
            widgets2.buffer.undo();
        });
    }
    {
        let widgets2 = widgets.clone();
        widgets.clone().redo_button.connect_clicked(move |_| {
            widgets2.buffer.redo();
        });
    }

    // --- server switch ---
    {
        let (state, widgets) = (state.clone(), widgets.clone());
        widgets
            .clone()
            .server_row
            .connect_active_notify(move |row| {
                set_server_running(&state, &widgets, row.is_active());
            });
    }
}

/// Builds a stylesheet for whatever note colours are currently in use.
///
/// Colours are arbitrary hex from the Android app, so the rules can't be
/// written ahead of time. The provider is replaced wholesale on each refresh.
pub fn install_color_css(state: &Shared) {
    use std::collections::BTreeSet;

    let colors: BTreeSet<String> = state
        .borrow()
        .notes
        .iter()
        .filter_map(|n| n.color.clone())
        .collect();

    // Card styling applies to every row, coloured or not, so the rhythm of the
    // list stays the same when a note has no colour.
    let mut css = String::from(
        "list.notes > row { margin: 3px 6px; border-radius: 10px; }\n         list.notes > row > box { padding: 2px 2px; }\n",
    );
    for color in &colors {
        let class = super::window::color_css_class(color);
        // Pastel fills need dark text; a dark custom colour needs light text.
        let fg = if crate::model::is_light_color(color) {
            "#1c1c1c"
        } else {
            "#ffffff"
        };
        css.push_str(&format!(
            "row.{class} {{ background-color: {color}; color: {fg}; border-radius: 10px; }}\n             row.{class} .dim-label {{ color: {fg}; opacity: 0.7; }}\n             row.{class}:selected {{ background-image: linear-gradient(rgba(0,0,0,.18), rgba(0,0,0,.18)); }}\n"
        ));
    }

    let provider = gtk::CssProvider::new();
    provider.load_from_data(&css);

    if let Some(display) = gtk::gdk::Display::default() {
        // Remove the previous generation before adding the new one, or old
        // colour rules accumulate for the lifetime of the process.
        if let Some(old) = COLOR_PROVIDER.with(|p| p.borrow_mut().take()) {
            gtk::style_context_remove_provider_for_display(&display, &old);
        }
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        COLOR_PROVIDER.with(|p| *p.borrow_mut() = Some(provider));
    }
}

thread_local! {
    static COLOR_PROVIDER: std::cell::RefCell<Option<gtk::CssProvider>> =
        const { std::cell::RefCell::new(None) };
}

/// Finds the sidebar row carrying `target`, if it is still there.
fn row_for_target(
    sidebar: &gtk::ListBox,
    target: &super::window::SidebarTarget,
) -> Option<gtk::ListBoxRow> {
    let mut child = sidebar.first_child();
    while let Some(w) = child {
        if let Some(row) = w.clone().downcast::<gtk::ListBoxRow>().ok() {
            let raw = unsafe { row.data::<String>("target") }.map(|p| unsafe { p.as_ref().clone() });
            if raw.is_some_and(|r| super::window::SidebarTarget::decode(&r) == *target) {
                return Some(row);
            }
        }
        child = w.next_sibling();
    }
    None
}

pub fn refresh_sidebar(state: &Shared, widgets: &Rc<Widgets>) {
    use super::window::SidebarTarget;

    let sidebar = &widgets.sidebar;

    // Remember what was selected so a rebuild doesn't dump the user back on
    // "All notes" every time a tag or folder appears.
    let current = {
        let s = state.borrow();
        match s.filter {
            Filter::Pinned => SidebarTarget::Pinned,
            Filter::Archived => SidebarTarget::Archived,
            Filter::Trash => SidebarTarget::Trash,
            Filter::Folder => s
                .folder
                .clone()
                .map(SidebarTarget::Folder)
                .unwrap_or(SidebarTarget::Notes),
            Filter::Tag => s
                .tag
                .clone()
                .map(SidebarTarget::Tag)
                .unwrap_or(SidebarTarget::Notes),
            Filter::Notes => SidebarTarget::Notes,
        }
    };

    state.borrow_mut().rebuilding = true;
    while let Some(child) = sidebar.first_child() {
        sidebar.remove(&child);
    }

    let mut rows: Vec<(gtk::ListBoxRow, SidebarTarget)> = Vec::new();

    let mut add_row = |icon: &str, label: &str, count: Option<usize>, target: SidebarTarget| {
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(label))
            .build();
        row.add_prefix(&gtk::Image::from_icon_name(icon));
        if let Some(count) = count {
            // The count is the context mobile's folder cards carry and a flat
            // sidebar otherwise loses.
            row.add_suffix(
                &gtk::Label::builder()
                    .label(count.to_string())
                    .css_classes(vec!["dim-label", "caption"])
                    .build(),
            );
        }
        unsafe { row.set_data("target", target.encode()) };
        sidebar.append(&row);
        rows.push((row.clone().upcast(), target));
    };

    for (icon, label, target) in [
        ("view-list-symbolic", "Home", SidebarTarget::Notes),
        ("view-pin-symbolic", "Pinned", SidebarTarget::Pinned),
        ("view-conceal-symbolic", "Archived", SidebarTarget::Archived),
        ("user-trash-symbolic", "Trash", SidebarTarget::Trash),
    ] {
        let count = state.borrow().target_count(&target);
        add_row(icon, label, Some(count), target);
    }

    let (folders, tags) = {
        let s = state.borrow();
        (s.store.folders().unwrap_or_default(), s.all_tags())
    };

    if !folders.is_empty() {
        sidebar.append(&section_header("Folders"));
        for folder in folders {
            let target = SidebarTarget::Folder(folder.clone());
            let count = state.borrow().target_count(&target);
            add_row("folder-symbolic", &folder, Some(count), target);
            // Visible ⋮ menu (discoverable, unlike a hidden right-click) with
            // Delete. Fetch the row from the sidebar — `rows` is borrowed by add_row.
            if let Some(row) = sidebar.last_child().and_downcast::<adw::ActionRow>() {
                attach_row_menu(&row, state, widgets, RowKind::Folder(folder.clone()));
            }
        }
    }

    if !tags.is_empty() {
        sidebar.append(&section_header("Tags"));
        for (tag, count) in tags {
            add_row(
                "tag-symbolic",
                &tag,
                Some(count),
                SidebarTarget::Tag(tag.clone()),
            );
            if let Some(row) = sidebar.last_child().and_downcast::<adw::ActionRow>() {
                attach_row_menu(&row, state, widgets, RowKind::Tag(tag.clone()));
            }
        }
    }

    // Reselect what was active; fall back to the first row if it vanished.
    let restored = rows
        .iter()
        .find(|(_, t)| *t == current)
        .map(|(r, _)| r.clone());
    state.borrow_mut().rebuilding = false;
    if let Some(row) = restored.or_else(|| sidebar.row_at_index(0)) {
        // Deliberate: the fallback row-0 case (the selected folder or tag was
        // just deleted) has to reach the handler and change the filter.
        state.borrow_mut().intent = true;
        sidebar.select_row(Some(&row));
    }
}

/// Non-selectable divider row for the note list ("Pinned" / "Others").
fn list_section_header(label: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::builder()
        .selectable(false)
        .activatable(false)
        .build();
    unsafe { row.set_data("section-header", true) };
    row.set_child(Some(
        &gtk::Label::builder()
            .label(label)
            .css_classes(vec!["dim-label", "caption-heading"])
            .xalign(0.0)
            .margin_start(12)
            .margin_top(10)
            .margin_bottom(2)
            .build(),
    ));
    row
}

fn section_header(label: &str) -> gtk::ListBoxRow {
    gtk::ListBoxRow::builder()
        .selectable(false)
        .activatable(false)
        .child(
            &gtk::Label::builder()
                .label(label)
                .css_classes(vec!["dim-label", "caption-heading"])
                .xalign(0.0)
                .margin_start(12)
                .margin_top(12)
                .margin_bottom(4)
                .build(),
        )
        .build()
}

pub fn refresh_list(state: &Shared, widgets: &Rc<Widgets>) {
    install_color_css(state);

    {
        let s = state.borrow();
        let title = match s.filter {
            Filter::Notes => "Home".to_string(),
            Filter::Pinned => "Pinned".to_string(),
            Filter::Archived => "Archived".to_string(),
            Filter::Trash => "Trash".to_string(),
            Filter::Folder => s.folder.clone().unwrap_or_else(|| "Folder".to_string()),
            Filter::Tag => s
                .tag
                .clone()
                .map(|t| format!("#{t}"))
                .unwrap_or_else(|| "Tag".to_string()),
        };
        let count = s.visible_notes().len();
        widgets.list_page.set_title(&title);
        // A count is the one bit of context the mobile folder cards carry and
        // a flat list otherwise loses.
        widgets.search.set_placeholder_text(Some(&match count {
            0 => "Search notes".to_string(),
            1 => "Search 1 note".to_string(),
            n => format!("Search {n} notes"),
        }));
    }

    let list = &widgets.list;

    // Read the selection first: the removal loop below emits row-selected(None).
    let selected = state.borrow().selected.clone();
    state.borrow_mut().rebuilding = true;

    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    let mut row_to_select = None;

    let (selecting, checked) = {
        let s = state.borrow();
        (s.selecting, s.checked.clone())
    };

    // Section headers ("Pinned" / "Others") only make sense in views that mix
    // both — the "All notes" and folder/tag lists. Pinned/Archived/Trash are
    // already single-category.
    let show_sections = {
        let s = state.borrow();
        matches!(s.filter, Filter::Notes | Filter::Folder | Filter::Tag)
            && s.visible_notes().iter().any(|n| n.is_pinned)
            && s.visible_notes().iter().any(|n| !n.is_pinned)
    };
    let mut last_section: Option<bool> = None;
    let in_trash = state.borrow().filter == Filter::Trash;

    for note in state.borrow().visible_notes() {
        // Insert a non-selectable header when crossing the pinned→others line.
        if show_sections && last_section != Some(note.is_pinned) {
            list.append(&list_section_header(if note.is_pinned {
                "Pinned"
            } else {
                "Others"
            }));
            last_section = Some(note.is_pinned);
        }

        let inner = note_row(note, in_trash);
        // In multi-select the row leads with a checkbox; otherwise it's just
        // the note card.
        let child: gtk::Widget = if selecting {
            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let check = gtk::CheckButton::builder()
                .active(checked.contains(&note.id))
                .can_focus(false)
                .build();
            {
                let (state, widgets) = (state.clone(), widgets.clone());
                let id = note.id.clone();
                check.connect_toggled(move |c| {
                    {
                        let mut s = state.borrow_mut();
                        if c.is_active() {
                            s.checked.insert(id.clone());
                        } else {
                            s.checked.remove(&id);
                        }
                    }
                    update_bulk_bar(&state, &widgets);
                });
            }
            hbox.append(&check);
            inner.set_hexpand(true);
            hbox.append(&inner);
            hbox.upcast()
        } else {
            inner
        };
        let row = gtk::ListBoxRow::builder().child(&child).build();
        unsafe { row.set_data("note-id", note.id.clone()) };
        if !selecting && selected.as_deref() == Some(note.id.as_str()) {
            row_to_select = Some(row.clone());
        }
        list.append(&row);
    }

    if let Some(row) = row_to_select {
        list.select_row(Some(&row));
    }
    state.borrow_mut().rebuilding = false;
}

/// Writes the current note back to disk and refreshes the list.
///
/// Used by every checklist edit; the note is cloned out before saving so the
/// `RefCell` borrow is released before any handler can re-enter.
fn commit(state: &Shared, widgets: &Rc<Widgets>, mutate: impl FnOnce(&mut crate::model::Note)) {
    let saved = {
        let mut s = state.borrow_mut();
        let Some(note) = s.selected_note_mut() else {
            return;
        };
        mutate(note);
        note.touch();
        let snapshot = note.clone();
        s.store.save(&snapshot)
    };
    if let Err(e) = saved {
        eprintln!("failed to save note: {e}");
        mark_save_failed(widgets);
    } else {
        mark_saved(widgets);
    }
    // Deferred, never synchronous: commit runs from TextBuffer::changed
    // handlers (checklist items, colour, tags), and rebuilding list rows
    // during the input method's key dispatch corrupts rows and can drop the
    // selection — which then silently swallows every following commit.
    schedule_list_refresh(state, widgets);
}

/// Rebuilds the checklist body for the selected note.
///
/// Checked items sink below a "N completed" divider, matching the mobile
/// layout — the useful items stay at the top of the list.
pub fn build_checklist(state: &Shared, widgets: &Rc<Widgets>) {
    build_checklist_focus(state, widgets, None);
}

/// Appends a fresh empty item to the open checklist note and focuses its
/// entry. Shared by the "Add item" button and Ctrl+Alt+N.
pub fn add_checklist_item(state: &Shared, widgets: &Rc<Widgets>) {
    let new_id = crate::model::new_uuid();
    {
        let new_id = new_id.clone();
        commit(state, widgets, move |note| {
            let items = note.checklist_items.get_or_insert_with(Vec::new);
            let order = items.iter().map(|i| i.order).max().unwrap_or(-1) + 1;
            items.push(crate::model::ChecklistItem {
                id: new_id.clone(),
                text: String::new(),
                is_checked: false,
                order,
                original_order: order,
                created_at: crate::model::now_millis(),
                indentation_level: 0,
            });
        });
    }
    build_checklist_focus(state, widgets, Some(&new_id));
}

fn build_checklist_focus(state: &Shared, widgets: &Rc<Widgets>, focus_id: Option<&str>) {
    let container = &widgets.checklist_box;
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
    let focus_target = Rc::new(RefCell::new(None::<gtk::TextView>));

    let Some(note) = state.borrow().selected_note().cloned() else {
        return;
    };
    let mut items = note.checklist_items.clone().unwrap_or_default();
    items.sort_by_key(|i| i.order);

    let (open, done): (Vec<_>, Vec<_>) = items.iter().cloned().partition(|i| !i.is_checked);

    let add_row = |item: &crate::model::ChecklistItem| {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .vexpand(false)
            .valign(gtk::Align::Start)
            .build();

        let check = gtk::CheckButton::builder()
            .active(item.is_checked)
            .valign(gtk::Align::Start)
            .build();
        // A TextView, not an Entry: one item may hold several lines (Enter
        // inserts a newline) and must wrap visibly while staying one item
        // with one checkbox.
        let entry = gtk::TextView::builder()
            .hexpand(true)
            .vexpand(false)
            .valign(gtk::Align::Start)
            // A non-expanding TextView otherwise has a zero-height minimum in
            // GTK and can be clipped completely during a rapid rebuild. This
            // is only a floor: wrapped/multiline content may still request
            // more height.
            .height_request(32)
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(4)
            .bottom_margin(4)
            .build();
        entry.buffer().set_text(&item.text);
        entry.add_css_class("inline-item");
        if item.is_checked {
            entry.add_css_class("dim-label");
        }
        // Enter = new item (the fast way to write a list); Ctrl+Enter = newline
        // inside this item. Capture phase, so the TextView's own Return
        // handling never sees the key.
        {
            let (state, widgets) = (state.clone(), widgets.clone());
            let entry_weak = entry.downgrade();
            let keys = gtk::EventControllerKey::new();
            keys.set_propagation_phase(gtk::PropagationPhase::Capture);
            keys.connect_key_pressed(move |_, key, _, modifiers| {
                if key != gtk::gdk::Key::Return && key != gtk::gdk::Key::KP_Enter {
                    return glib::Propagation::Proceed;
                }
                if modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK) {
                    if let Some(entry) = entry_weak.upgrade() {
                        entry.buffer().insert_at_cursor("\n");
                    }
                } else {
                    // Defer: adding an item rebuilds the checklist, and tearing
                    // down the TextView that is still dispatching this very key
                    // event corrupts the rows intermittently.
                    let (state, widgets) = (state.clone(), widgets.clone());
                    glib::idle_add_local_once(move || {
                        add_checklist_item(&state, &widgets);
                    });
                }
                glib::Propagation::Stop
            });
            entry.add_controller(keys);
        }
        let remove = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Remove item")
            .css_classes(vec!["flat"])
            .build();

        let id = item.id.clone();

        {
            let (state, widgets, id) = (state.clone(), widgets.clone(), id.clone());
            check.connect_toggled(move |c| {
                if state.borrow().loading {
                    return;
                }
                let active = c.is_active();
                commit(&state, &widgets, |note| {
                    if let Some(items) = note.checklist_items.as_mut() {
                        if let Some(i) = items.iter_mut().find(|i| i.id == id) {
                            i.is_checked = active;
                        }
                    }
                });
                // Toggling moves the item across the divider, so redraw.
                build_checklist(&state, &widgets);
            });
        }
        {
            let (state, widgets, id) = (state.clone(), widgets.clone(), id.clone());
            entry.buffer().connect_changed(move |b| {
                if state.borrow().loading {
                    return;
                }
                let text = b.text(&b.start_iter(), &b.end_iter(), false).to_string();
                commit(&state, &widgets, |note| {
                    if let Some(items) = note.checklist_items.as_mut() {
                        if let Some(i) = items.iter_mut().find(|i| i.id == id) {
                            i.text = text.clone();
                        }
                    }
                });
            });
        }
        {
            let (state, widgets, id) = (state.clone(), widgets.clone(), id.clone());
            remove.connect_clicked(move |_| {
                commit(&state, &widgets, |note| {
                    if let Some(items) = note.checklist_items.as_mut() {
                        items.retain(|i| i.id != id);
                    }
                });
                build_checklist(&state, &widgets);
            });
        }

        row.append(&check);
        row.append(&entry);
        row.append(&remove);
        container.append(&row);
        if focus_id == Some(item.id.as_str()) {
            *focus_target.borrow_mut() = Some(entry);
        }
    };

    for item in &open {
        add_row(item);
    }

    if !done.is_empty() {
        let divider = gtk::Label::builder()
            .label(format!("{} completed", done.len()))
            .css_classes(vec!["dim-label", "caption"])
            .margin_top(10)
            .margin_bottom(4)
            .build();
        container.append(&divider);
        for item in &done {
            add_row(item);
        }
    }

    // A GtkButton with both `label` and `icon_name` shows only the icon;
    // ButtonContent is how you get an icon *and* a label.
    let add = gtk::Button::builder()
        .child(
            &adw::ButtonContent::builder()
                .icon_name("list-add-symbolic")
                .label("Add item")
                .build(),
        )
        .css_classes(vec!["flat"])
        .halign(gtk::Align::Start)
        .margin_top(8)
        .build();
    {
        let (state, widgets) = (state.clone(), widgets.clone());
        add.connect_clicked(move |_| add_checklist_item(&state, &widgets));
    }

    // One-time item re-sort, mirroring Android's checklist sort menu: picking
    // an option rewrites order/originalOrder to the new positions (the result
    // is the new manual baseline) and remembers it in checklistSortOption.
    const SORT_OPTIONS: &[(&str, &str)] = &[
        ("UNCHECKED_FIRST", "Unchecked first"),
        ("CHECKED_FIRST", "Checked first"),
        ("ALPHABETICAL_ASC", "A → Z"),
        ("ALPHABETICAL_DESC", "Z → A"),
        ("CREATION_DATE", "Oldest first"),
        ("CREATION_DATE_DESC", "Newest first"),
    ];
    let sort_menu_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();
    for (option, label) in SORT_OPTIONS {
        let pick = gtk::Button::builder()
            .label(*label)
            .css_classes(vec!["flat"])
            .build();
        let (state, widgets) = (state.clone(), widgets.clone());
        pick.connect_clicked(move |btn| {
            commit(&state, &widgets, |note| {
                if let Some(items) = note.checklist_items.as_mut() {
                    crate::model::sort_checklist(items, option);
                }
                note.checklist_sort_option = Some(option.to_string());
            });
            if let Some(pop) = btn
                .ancestor(gtk::Popover::static_type())
                .and_downcast::<gtk::Popover>()
            {
                pop.popdown();
            }
            build_checklist(&state, &widgets);
        });
        sort_menu_box.append(&pick);
    }
    let sort_items = gtk::MenuButton::builder()
        .child(
            &adw::ButtonContent::builder()
                .icon_name("view-sort-descending-symbolic")
                .label("Sort items")
                .build(),
        )
        .css_classes(vec!["flat"])
        .halign(gtk::Align::End)
        .hexpand(true)
        .margin_top(8)
        .popover(&gtk::Popover::builder().child(&sort_menu_box).build())
        .build();

    let footer = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .build();
    footer.append(&add);
    footer.append(&sort_items);
    container.append(&footer);

    // Rapid Enter presses rebuild this entire box while the enclosing
    // ScrolledWindow is also adjusting to keep the new focused row visible.
    // GTK can retain an intermediate height allocation until another event
    // (wheel scrolling or opening the Sort popover) starts a later frame. An
    // idle callback is still too early: it runs before the first allocation.
    // Keep rows non-expanding above, then request two real frame allocations
    // after the completed tree is mapped.
    let entry_to_focus = focus_target.borrow_mut().take();
    let container = container.clone();
    let body_stack = widgets.body_stack.clone();
    let frames = Cell::new(0u8);
    widgets.window.add_tick_callback(move |window, _| {
        if frames.get() == 0 {
            if let Some(entry) = entry_to_focus.as_ref() {
                entry.grab_focus();
            }
        }
        container.queue_resize();
        container.queue_allocate();
        body_stack.queue_resize();
        body_stack.queue_allocate();
        window.queue_resize();
        window.queue_allocate();
        frames.set(frames.get() + 1);
        if frames.get() >= 2 {
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

/// Rebuilds the tag chips under the title for the selected note.
pub fn build_tags(state: &Shared, widgets: &Rc<Widgets>) {
    let container = &widgets.tag_box;
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    let Some(note) = state.borrow().selected_note().cloned() else {
        return;
    };

    for tag in note.labels.iter().flatten() {
        let chip = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(2)
            .css_classes(vec!["card"])
            .build();
        chip.append(
            &gtk::Label::builder()
                .label(tag)
                .css_classes(vec!["caption"])
                .margin_start(8)
                .build(),
        );
        let remove = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .css_classes(vec!["flat", "circular"])
            .tooltip_text(format!("Remove tag \"{tag}\""))
            .build();
        {
            let (state, widgets, tag) = (state.clone(), widgets.clone(), tag.clone());
            remove.connect_clicked(move |_| {
                commit(&state, &widgets, |note| {
                    if let Some(labels) = note.labels.as_mut() {
                        labels.retain(|l| l != &tag);
                        if labels.is_empty() {
                            // An empty array and "no tags" should not be two
                            // different states on disk.
                            note.labels = None;
                        }
                    }
                });
                build_tags(&state, &widgets);
                refresh_sidebar(&state, &widgets);
            });
        }
        chip.append(&remove);
        container.append(&chip);
    }

    let add = gtk::Button::builder()
        .icon_name("tag-symbolic")
        .tooltip_text("Add or remove tags")
        .css_classes(vec!["flat", "circular"])
        .build();
    {
        let (state, widgets) = (state.clone(), widgets.clone());
        add.connect_clicked(move |_| prompt_tags(&state, &widgets));
    }
    container.append(&add);
}

/// Tag picker: every tag already in use, pre-ticked for this note, plus a
/// field to create a new one.
///
/// A free-text-only field made typos silently create near-duplicate tags
/// ("work" vs "Work" vs "wrok"), which is corrosive when tags are the main way
/// you navigate. Picking from what exists is the common case; creating is the
/// exception.
pub fn prompt_tags(state: &Shared, widgets: &Rc<Widgets>) {
    let Some(note) = state.borrow().selected_note().cloned() else {
        return;
    };
    let existing: Vec<String> = state
        .borrow()
        .all_tags()
        .into_iter()
        .map(|(t, _)| t)
        .collect();
    let current: Vec<String> = note.labels.clone().unwrap_or_default();

    let dialog = adw::AlertDialog::builder()
        .heading("Tags")
        .body(format!(
            "Tags on \"{}\"",
            if note.title.trim().is_empty() {
                "Untitled"
            } else {
                &note.title
            }
        ))
        .build();

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();

    // Existing tags, pre-ticked. Union of "in use anywhere" and "on this
    // note", so a tag unique to this note still shows.
    let mut all: Vec<String> = existing;
    for tag in &current {
        if !all.iter().any(|t| t == tag) {
            all.push(tag.clone());
        }
    }
    all.sort();

    let checks: Vec<(String, gtk::CheckButton)> = all
        .iter()
        .map(|tag| {
            let check = gtk::CheckButton::builder()
                .label(tag)
                .active(current.iter().any(|c| c == tag))
                .build();
            content.append(&check);
            (tag.clone(), check)
        })
        .collect();

    if checks.is_empty() {
        content.append(
            &gtk::Label::builder()
                .label("No tags yet.")
                .css_classes(vec!["dim-label", "caption"])
                .build(),
        );
    }

    content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let new_entry = gtk::Entry::builder()
        .placeholder_text("Create new tag")
        .activates_default(true)
        .build();
    content.append(&new_entry);

    dialog.set_extra_child(Some(
        &gtk::ScrolledWindow::builder()
            .propagate_natural_height(true)
            .max_content_height(320)
            .child(&content)
            .build(),
    ));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("apply", "Done");
    dialog.set_response_appearance("apply", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("apply"));
    dialog.set_close_response("cancel");

    let parent = widgets.window.clone();
    let (state, widgets) = (state.clone(), widgets.clone());
    dialog.connect_response(None, move |_, response| {
        if response != "apply" {
            return;
        }
        let mut chosen: Vec<String> = checks
            .iter()
            .filter(|(_, c)| c.is_active())
            .map(|(t, _)| t.clone())
            .collect();

        let fresh = new_entry.text().trim().to_string();
        if !fresh.is_empty() && !chosen.iter().any(|t| t.eq_ignore_ascii_case(&fresh)) {
            chosen.push(fresh);
        }

        commit(&state, &widgets, |note| {
            // None rather than an empty array, so "no tags" is one state.
            note.labels = (!chosen.is_empty()).then(|| chosen.clone());
        });
        build_tags(&state, &widgets);
        refresh_sidebar(&state, &widgets);
    });

    dialog.present(Some(&parent));
}

pub fn load_editor(state: &Shared, widgets: &Rc<Widgets>) {
    let note = state.borrow().selected_note().cloned();
    state.borrow_mut().loading = true;

    match note {
        Some(note) => {
            // The visible header title is disabled, but NavigationPage still
            // needs a non-empty title for accessibility and back navigation.
            widgets.editor_page.set_title("Editor");
            widgets.editor_stack.set_visible_child_name("editor");
            widgets.title_entry.set_text(&note.title);
            build_tags(state, widgets);
            // On a collapsed (tablet/phone) layout, opening a note brings the
            // editor pane forward; the back button returns to the list.
            widgets.inner_split.set_show_content(true);

            if note.note_type == crate::model::NoteType::CHECKLIST {
                widgets.body_stack.set_visible_child_name("checklist");
                build_checklist(state, widgets);
            } else {
                widgets.body_stack.set_visible_child_name("text");
                // Loading a note is not an edit. Without this, the switch lands
                // on the undo stack and Ctrl+Z pastes the *previous* note's
                // text into this one — which autosave then writes to disk.
                widgets.buffer.begin_irreversible_action();
                widgets.buffer.set_text(&note.content);
                widgets.buffer.end_irreversible_action();
                update_undo_state(widgets);
                widgets.save_indicator.set_label("Saved");
                widgets
                    .body_placeholder
                    .set_visible(note.content.is_empty());
            }
            widgets.pin_button.set_active(note.is_pinned);
            widgets.archive_button.set_active(note.is_archived());
            widgets
                .archive_button
                .set_tooltip_text(Some(if note.is_archived() {
                    "Unarchive note (Ctrl+A or Ctrl+Shift+A while typing)"
                } else {
                    "Archive note (Ctrl+A or Ctrl+Shift+A while typing)"
                }));
            // In the trash: offer Restore, and the trash button becomes the
            // permanent delete. Archiving a trashed note is meaningless, so its
            // button is greyed out until the note is restored.
            let trashed = note.is_trashed();
            widgets.restore_button.set_visible(trashed);
            widgets.archive_button.set_sensitive(!trashed);
            widgets.delete_button.set_tooltip_text(Some(if trashed {
                "Delete permanently (Delete outside text fields)"
            } else {
                "Move to trash (Delete outside text fields)"
            }));
        }
        None => {
            widgets.editor_page.set_title("Editor");
            widgets.editor_stack.set_visible_child_name("empty");
            widgets.title_entry.set_text("");
            widgets.buffer.begin_irreversible_action();
            widgets.buffer.set_text("");
            widgets.buffer.end_irreversible_action();
            widgets.pin_button.set_active(false);
            widgets.archive_button.set_active(false);
            widgets.archive_button.set_sensitive(true);
            widgets.restore_button.set_visible(false);
        }
    }

    // Clear the guard only after GTK has delivered the change signals the
    // assignments above queued, otherwise they'd be written back as edits.
    let state = state.clone();
    glib::idle_add_local_once(move || {
        state.borrow_mut().loading = false;
    });
}

/// Starts watching the notes directory and reloads the list when something
/// outside this app changes it — a phone sync, or an external editor.
///
/// Returns the watcher, which the caller must keep alive.
pub fn start_watching(state: &Shared, widgets: &Rc<Widgets>) -> Option<notify::RecommendedWatcher> {
    let (root, stamp) = {
        let s = state.borrow();
        (s.store.root().to_path_buf(), s.store.write_stamp.clone())
    };

    let dirty = crate::watcher::DirtyFlag::default();
    let watcher = match crate::watcher::watch(&root, dirty.clone()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("could not watch {}: {e}", root.display());
            return None;
        }
    };

    let mut known_folders = state.borrow().store.folders().unwrap_or_default();
    let (state, widgets) = (state.clone(), widgets.clone());
    glib::timeout_add_local(
        std::time::Duration::from_millis(crate::watcher::DEBOUNCE_MS),
        move || {
            if crate::watcher::should_reload(
                dirty.is_set(),
                crate::model::now_millis(),
                stamp.last_millis(),
            ) {
                // Only consume after the echo window. Previously `take()` ran
                // before this check, so an external write that happened while
                // the desktop was saving was discarded and never retried.
                dirty.take();
                // Keep the selection across the reload; the note is still
                // there unless the change deleted it.
                let selected = state.borrow().selected.clone();
                let (notes_changed, folders_after) = {
                    let mut s = state.borrow_mut();
                    let before = s.notes.clone();
                    s.reload();
                    s.selected = selected.filter(|id| s.notes.iter().any(|n| &n.id == id));
                    s.clear_selection_if_hidden();
                    let folders_after = s.store.folders().unwrap_or_default();
                    (s.notes != before, folders_after)
                };
                let folders_changed = known_folders != folders_after;
                known_folders = folders_after;
                // A delayed self-write event reaches here too. If disk and
                // memory already agree, rebuilding the editor would only move
                // the cursor and disturb typing, so do nothing.
                if notes_changed || folders_changed {
                    refresh_sidebar(&state, &widgets);
                }
                if notes_changed {
                    refresh_list(&state, &widgets);
                    load_editor(&state, &widgets);
                }
            }
            glib::ControlFlow::Continue
        },
    );

    Some(watcher)
}

/// Runs one client-mode sync against the configured remote, off the UI thread.
///
/// Nothing here refreshes the note list: the sync writes into the vault the
/// filesystem watcher is already watching, so the reload happens the same way it
/// does when the phone uploads a note. `quiet` suppresses the "nothing to do"
/// toast for automatic runs, which would otherwise interrupt every 5 minutes.
pub fn sync_now(state: &Shared, widgets: &Rc<Widgets>, quiet: bool) {
    let (dir, mirror, client, busy) = {
        let s = state.borrow();
        (
            s.config.data_dir.clone(),
            s.config.markdown_mirror,
            s.config.remote_client(),
            s.syncing,
        )
    };
    let Some(client) = client else {
        if !quiet {
            super::window::toast(widgets, "No remote server set — add one in Settings");
        }
        return;
    };
    if busy {
        return; // a run is already going; two would race over the same files
    }
    state.borrow_mut().syncing = true;

    let (state, widgets) = (state.clone(), widgets.clone());
    glib::spawn_future_local(async move {
        let result = gtk::gio::spawn_blocking(move || {
            let mut store = crate::store::Store::new(dir)?;
            store.markdown_mirror = mirror;
            crate::sync::run_once(&store, &client)
        })
        .await;

        {
            let mut s = state.borrow_mut();
            s.syncing = false;
            s.last_sync = crate::model::now_millis();
        }
        match result {
            Ok(Ok(summary)) => {
                let moved = summary.pulled + summary.pushed;
                let removed = summary.deleted_local + summary.deleted_remote;
                if !summary.failed.is_empty() {
                    // The run still applied everything it could; say what broke
                    // rather than pretending it all worked.
                    eprintln!("sync problems: {}", summary.failed.join("; "));
                    super::window::toast(
                        &widgets,
                        &format!("Synced with {} problem(s) — see terminal", summary.failed.len()),
                    );
                } else if moved == 0 && removed == 0 {
                    if !quiet {
                        super::window::toast(&widgets, "Already up to date");
                    }
                } else {
                    super::window::toast(
                        &widgets,
                        &format!("Synced — {} in, {} out", summary.pulled, summary.pushed),
                    );
                }
            }
            Ok(Err(e)) => super::window::toast(&widgets, &format!("Sync failed: {e}")),
            Err(_) => super::window::toast(&widgets, "Sync failed: worker thread died"),
        }
    });
}

/// Ticks once a minute and syncs when the configured interval has elapsed.
///
/// Deliberately re-reads the config each tick instead of rescheduling itself
/// when the setting changes — changing the interval then just works, with no
/// timer to tear down.
pub fn start_auto_sync(state: &Shared, widgets: &Rc<Widgets>) {
    let (state, widgets) = (state.clone(), widgets.clone());
    glib::timeout_add_seconds_local(60, move || {
        let due = {
            let s = state.borrow();
            let every = s.config.sync_interval_minutes;
            every > 0
                && crate::model::now_millis() - s.last_sync >= i64::from(every) * 60_000
                && s.config.remote_client().is_some()
        };
        if due {
            sync_now(&state, &widgets, true);
        }
        glib::ControlFlow::Continue
    });
}

pub fn set_server_running(state: &Shared, widgets: &Rc<Widgets>, run: bool) {
    if !run {
        let (handle, config) = {
            let mut s = state.borrow_mut();
            (s.server.take(), s.config.clone())
        };
        if let Some(handle) = handle {
            handle.stop();
        }
        widgets.server_status.set_text(&server_hint(&config, false));
        widgets.server_row.set_active(false);
        return;
    }

    let config = {
        let s = state.borrow();
        if s.server.as_ref().is_some_and(|h| h.is_running()) {
            return;
        }
        s.config.clone()
    };

    if config.password.is_empty() {
        widgets
            .server_status
            .set_text("Set a password before starting the server.");
        // No RefCell borrow is held here: changing the switch synchronously
        // emits its notify signal and re-enters this function.
        widgets.server_row.set_active(false);
        return;
    }

    match server::start(config.server_config()) {
        Ok(handle) => {
            state.borrow_mut().server = Some(handle);
            widgets.server_status.set_text(&server_hint(&config, true));
            widgets.server_row.set_active(true);
        }
        Err(e) => {
            widgets
                .server_status
                .set_text(&format!("Could not start server: {e}"));
            widgets.server_row.set_active(false);
        }
    }
}

/// Creates a note of the given kind in the current folder and opens it.
pub fn create_note(state: &Shared, widgets: &Rc<Widgets>, kind: crate::model::NoteType) {
    let (id, clear_search) = {
        let mut s = state.borrow_mut();
        let active_tag = (s.filter == Filter::Tag).then(|| s.tag.clone()).flatten();
        if matches!(s.filter, Filter::Archived | Filter::Trash) {
            s.apply_target(&super::window::SidebarTarget::Notes);
        }
        let clear_search = !s.query.is_empty();
        s.query.clear();
        let mut note = Note {
            // A note made while viewing a folder belongs to that folder; one
            // made in the Pinned view arrives pinned. Both match what the
            // sidebar selection implies.
            folder_name: if s.filter == Filter::Folder {
                s.folder.clone()
            } else {
                None
            },
            is_pinned: s.filter == Filter::Pinned,
            labels: active_tag.map(|tag| vec![tag]),
            note_type: kind,
            ..Default::default()
        };
        if kind == crate::model::NoteType::CHECKLIST {
            // Start with one empty row, so the note isn't a blank page.
            note.checklist_items = Some(vec![crate::model::ChecklistItem {
                id: crate::model::new_uuid(),
                text: String::new(),
                is_checked: false,
                order: 0,
                original_order: 0,
                created_at: crate::model::now_millis(),
                indentation_level: 0,
            }]);
        }
        if let Err(e) = s.store.save(&note) {
            eprintln!("failed to create note: {e}");
            return;
        }
        s.reload();
        s.selected = Some(note.id.clone());
        (note.id, clear_search)
    };
    let _ = id;
    if clear_search {
        widgets.search.set_text("");
    }
    refresh_sidebar(state, widgets);
    refresh_list(state, widgets);
    load_editor(state, widgets);
    widgets.title_entry.grab_focus();
}

/// Asks for a folder name and creates it.
pub fn prompt_new_folder(state: &Shared, widgets: &Rc<Widgets>) {
    let dialog = adw::AlertDialog::builder()
        .heading("New folder")
        .body("Notes in a folder sync to a matching subfolder on your phone.")
        .build();

    let entry = gtk::Entry::builder()
        .placeholder_text("Folder name")
        .activates_default(true)
        .build();
    dialog.set_extra_child(Some(&entry));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("create", "Create");
    dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("create"));
    dialog.set_close_response("cancel");

    // Keep a handle for `present` below; the closure takes its own clones.
    let parent = widgets.window.clone();
    let (state, widgets) = (state.clone(), widgets.clone());
    let entry_ref = entry.clone();
    dialog.connect_response(None, move |_, response| {
        if response != "create" {
            return;
        }
        let name = entry_ref.text().to_string();
        let result = state.borrow().store.create_folder(&name);
        match result {
            Ok(created) => {
                refresh_sidebar(&state, &widgets);
                toast(&widgets, &format!("Folder \"{created}\" created"));
            }
            // Duplicate or illegal names are ordinary mistakes, not failures
            // worth a crash — say what happened and leave the state alone.
            Err(e) => toast(&widgets, &e.to_string()),
        }
    });

    dialog.present(Some(&parent));
}

/// Applies `profile` to the running config: saves, restarts the server if it
/// was running, and toasts. Shared by the pill buttons and the auto-switcher.
pub fn apply_server_profile(
    state: &Shared,
    widgets: &Rc<Widgets>,
    profile: &crate::config::ServerProfile,
) {
    let (changed, was_running) = {
        let mut s = state.borrow_mut();
        let changed = s.config.apply_profile(profile);
        if changed {
            let _ = s.config.save();
        }
        let running = s.server.as_ref().is_some_and(|h| h.is_running());
        (changed, running)
    };
    if changed && was_running {
        set_server_running(state, widgets, false);
        set_server_running(state, widgets, true);
    }
    if changed {
        toast(widgets, &format!("Server profile: {}", profile.name));
    }
}

/// Watches the active network (poll, 20s) and applies the profile saved for
/// it whenever the network changes — so a laptop moving home ↔ work ↔ hotspot
/// picks the right server settings by itself.
pub fn start_network_watch(state: &Shared, widgets: &Rc<Widgets>) {
    let last = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
    // One immediate check (covers "network changed while the app was off"),
    // then the slow poll.
    let tick = {
        let (state, widgets, last) = (state.clone(), widgets.clone(), last.clone());
        move || {
            let Some(network) = crate::naming::current_network() else {
                return;
            };
            if *last.borrow() == network {
                return;
            }
            *last.borrow_mut() = network.clone();
            let profile = state.borrow().config.profile_for_network(&network).cloned();
            if let Some(profile) = profile {
                apply_server_profile(&state, &widgets, &profile);
            }
        }
    };
    tick();
    glib::timeout_add_seconds_local(20, move || {
        tick();
        glib::ControlFlow::Continue
    });
}

/// Reflects the checked count into the bulk bar label.
pub fn update_bulk_bar(state: &Shared, widgets: &Rc<Widgets>) {
    let n = state.borrow().checked.len();
    widgets.bulk_label.set_label(&format!("{n} selected"));
}

/// Wires the multi-select toggle and the three bulk actions.
pub fn connect_bulk(
    state: &Shared,
    widgets: &Rc<Widgets>,
    select_toggle: &gtk::ToggleButton,
    select_all: &gtk::CheckButton,
    bulk_move: &gtk::Button,
    bulk_pin: &gtk::Button,
    bulk_color_box: &gtk::Box,
    bulk_archive: &gtk::Button,
    bulk_delete: &gtk::Button,
) {
    // Select-all / none: tick every currently-visible note, or clear.
    {
        let (state, widgets) = (state.clone(), widgets.clone());
        select_all.connect_toggled(move |btn| {
            let want_all = btn.is_active();
            let ids: Vec<String> = state
                .borrow()
                .visible_notes()
                .iter()
                .map(|n| n.id.clone())
                .collect();
            {
                let mut s = state.borrow_mut();
                if want_all {
                    s.checked = ids.into_iter().collect();
                } else {
                    s.checked.clear();
                }
            }
            update_bulk_bar(&state, &widgets);
            refresh_list(&state, &widgets);
        });
    }
    {
        let (state, widgets, select_all) = (state.clone(), widgets.clone(), select_all.clone());
        select_toggle.connect_toggled(move |btn| {
            let on = btn.is_active();
            {
                let mut s = state.borrow_mut();
                s.selecting = on;
                s.checked.clear();
            }
            // Leaving the select-all in a stale "all ticked" state would be a lie
            // once checked is cleared, so reset it whenever the mode toggles.
            select_all.set_active(false);
            // Reflect what the bulk archive button will do in this view.
            let unarchiving = state.borrow().filter == Filter::Archived;
            widgets.bulk_archive.set_icon_name(if unarchiving {
                "view-reveal-symbolic"
            } else {
                "view-conceal-symbolic"
            });
            widgets.bulk_archive.set_tooltip_text(Some(if unarchiving {
                "Restore selected"
            } else {
                "Archive selected"
            }));
            widgets.bulk_bar.set_visible(on);
            update_bulk_bar(&state, &widgets);
            refresh_list(&state, &widgets);
        });
    }

    {
        let (state, widgets) = (state.clone(), widgets.clone());
        bulk_archive.connect_clicked(move |_| {
            // In the Archived view this is the restore action; everywhere else
            // it archives. One button, context-aware — mirrors how the sidebar
            // section you're in defines "the obvious bulk action".
            let unarchiving = state.borrow().filter == Filter::Archived;
            let n = state.borrow_mut().for_each_checked(|note| {
                note.archived_at = if unarchiving {
                    None
                } else {
                    Some(crate::model::now_millis())
                };
            });
            finish_bulk(&state, &widgets);
            toast(
                &widgets,
                &format!("{} {n}", if unarchiving { "Restored" } else { "Archived" }),
            );
        });
    }

    {
        let (state, widgets) = (state.clone(), widgets.clone());
        bulk_delete.connect_clicked(move |_| {
            let in_trash = state.borrow().filter == Filter::Trash;
            let msg = if in_trash {
                // Already in the trash → this is the permanent delete (mirrors the
                // single-note button). Hard-delete writes a deletions.json
                // tombstone so the phone doesn't resurrect them.
                let ids: Vec<String> = state.borrow().checked.iter().cloned().collect();
                let mut n = 0;
                for id in &ids {
                    if state.borrow().store.delete(id).is_ok() {
                        n += 1;
                    }
                }
                format!("Deleted {n} permanently")
            } else {
                // Move to trash — never hard-delete outside the trash view.
                let n = state.borrow_mut().for_each_checked(|note| {
                    if note.trashed_at.is_none() {
                        note.trashed_at = Some(crate::model::now_millis());
                    }
                });
                format!("Moved {n} to trash")
            };
            finish_bulk(&state, &widgets);
            toast(&widgets, &msg);
        });
    }

    {
        let (state, widgets) = (state.clone(), widgets.clone());
        bulk_move.connect_clicked(move |_| prompt_move_to_folder(&state, &widgets));
    }

    {
        let (state, widgets) = (state.clone(), widgets.clone());
        bulk_pin.connect_clicked(move |_| {
            // Toggle semantics: if anything checked is unpinned, pin everything;
            // only when the whole selection is already pinned does it unpin.
            let pin = {
                let s = state.borrow();
                s.notes
                    .iter()
                    .any(|n| s.checked.contains(&n.id) && !n.is_pinned)
            };
            let n = state.borrow_mut().for_each_checked(|note| {
                note.is_pinned = pin;
            });
            finish_bulk(&state, &widgets);
            toast(
                &widgets,
                &format!("{} {n}", if pin { "Pinned" } else { "Unpinned" }),
            );
        });
    }

    {
        let (state, widgets) = (state.clone(), widgets.clone());
        fill_swatch_grid(bulk_color_box, move |hex| {
            let n = state.borrow_mut().for_each_checked(|note| {
                note.color = hex.clone();
            });
            // A colour may be entering the list for the first time; the rows
            // can't paint it without a matching CSS rule.
            install_color_css(&state);
            finish_bulk(&state, &widgets);
            toast(&widgets, &format!("Coloured {n}"));
        });
    }
}

/// Fills the sort popover: one radio per field, then an ascending/descending
/// pair. Choices persist to config immediately and re-sort the list in place.
pub fn connect_sort(
    state: &Shared,
    widgets: &Rc<Widgets>,
    container: &gtk::Box,
    button: &gtk::MenuButton,
) {
    const FIELDS: &[(&str, &str)] = &[
        ("updated", "Updated"),
        ("created", "Created"),
        ("title", "Title (A–Z)"),
        ("type", "Type"),
        ("color", "Colour"),
    ];

    let inner = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(10)
        .margin_end(10)
        .build();

    let apply = {
        let (state, widgets, button) = (state.clone(), widgets.clone(), button.clone());
        move |field: Option<&str>, ascending: Option<bool>| {
            {
                let mut s = state.borrow_mut();
                if let Some(f) = field {
                    s.config.sort_field = f.to_string();
                }
                if let Some(a) = ascending {
                    s.config.sort_ascending = a;
                }
                if let Err(e) = s.config.save() {
                    eprintln!("failed to save config: {e}");
                }
                button.set_label(&sort_button_label(&s.config));
            }
            refresh_list(&state, &widgets);
        }
    };
    let apply = Rc::new(apply);

    let current_field = state.borrow().config.sort_field.clone();
    let mut field_group: Option<gtk::CheckButton> = None;
    for (key, label) in FIELDS {
        let radio = gtk::CheckButton::builder()
            .label(*label)
            .active(current_field == *key)
            .build();
        if let Some(g) = &field_group {
            radio.set_group(Some(g));
        } else {
            field_group = Some(radio.clone());
        }
        let apply = apply.clone();
        radio.connect_toggled(move |btn| {
            if btn.is_active() {
                apply(Some(key), None);
            }
        });
        inner.append(&radio);
    }

    inner.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    let ascending = state.borrow().config.sort_ascending;
    let desc = gtk::CheckButton::builder()
        .label("Descending")
        .active(!ascending)
        .build();
    let asc = gtk::CheckButton::builder()
        .label("Ascending")
        .active(ascending)
        .group(&desc)
        .build();
    {
        let apply = apply.clone();
        asc.connect_toggled(move |btn| {
            if btn.is_active() {
                apply(None, Some(true));
            }
        });
    }
    {
        let apply = apply.clone();
        desc.connect_toggled(move |btn| {
            if btn.is_active() {
                apply(None, Some(false));
            }
        });
    }
    inner.append(&desc);
    inner.append(&asc);

    button.set_label(&sort_button_label(&state.borrow().config));
    container.append(&inner);
}

/// "Updated ↓"-style caption for the sort button.
fn sort_button_label(config: &crate::config::Config) -> String {
    let field = match config.sort_field.as_str() {
        "created" => "Created",
        "title" => "Title",
        "type" => "Type",
        "color" => "Colour",
        _ => "Updated",
    };
    format!("{field} {}", if config.sort_ascending { "↑" } else { "↓" })
}

/// Applies bulk changes: reload, leave select-mode, refresh everything.
fn finish_bulk(state: &Shared, widgets: &Rc<Widgets>) {
    {
        let mut s = state.borrow_mut();
        s.reload();
        s.selecting = false;
        s.checked.clear();
    }
    // The open note may have been swept up in the bulk action (deleted, trashed
    // or moved) and is no longer in view — drop the selection so the editor
    // doesn't keep showing a note that left the list.
    let stale = {
        let s = state.borrow();
        match &s.selected {
            Some(id) => !s.visible_notes().iter().any(|n| &n.id == id),
            None => false,
        }
    };
    if stale {
        state.borrow_mut().selected = None;
    }
    widgets.select_toggle.set_active(false);
    widgets.bulk_bar.set_visible(false);
    refresh_sidebar(state, widgets);
    refresh_list(state, widgets);
    load_editor(state, widgets);
}

/// Folder picker for the checked notes. Includes "No folder" so a note can be
/// pulled back to the top level — the move that was previously impossible.
pub fn prompt_move_to_folder(state: &Shared, widgets: &Rc<Widgets>) {
    if state.borrow().checked.is_empty() {
        toast(widgets, "Nothing selected");
        return;
    }
    let folders = state.borrow().store.folders().unwrap_or_default();

    let dialog = adw::AlertDialog::builder()
        .heading("Move to folder")
        .build();
    let list = gtk::ListBox::builder()
        .css_classes(vec!["boxed-list"])
        .build();

    let none_row = gtk::ListBoxRow::new();
    none_row.set_child(Some(
        &gtk::Label::builder()
            .label("No folder (top level)")
            .xalign(0.0)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(8)
            .build(),
    ));
    unsafe { none_row.set_data("folder", String::new()) };
    list.append(&none_row);

    for folder in &folders {
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(
            &gtk::Label::builder()
                .label(folder)
                .xalign(0.0)
                .margin_top(8)
                .margin_bottom(8)
                .margin_start(8)
                .build(),
        ));
        unsafe { row.set_data("folder", folder.clone()) };
        list.append(&row);
    }

    dialog.set_extra_child(Some(&list));
    dialog.add_response("cancel", "Cancel");
    dialog.set_close_response("cancel");

    let parent = widgets.window.clone();
    let dlg = dialog.clone();
    let (state, widgets) = (state.clone(), widgets.clone());
    list.connect_row_activated(move |_, row| {
        let folder = unsafe { row.data::<String>("folder") }
            .map(|p| unsafe { p.as_ref().clone() })
            .unwrap_or_default();
        let target = (!folder.is_empty()).then_some(folder);

        let ids: Vec<String> = state.borrow().checked.iter().cloned().collect();
        let old_folders: std::collections::BTreeSet<String> = state
            .borrow()
            .notes
            .iter()
            .filter(|note| ids.iter().any(|id| id == &note.id))
            .filter_map(|note| note.folder_name.clone())
            .collect();
        let mut n = 0;
        for id in ids {
            let note = state.borrow().notes.iter().find(|x| x.id == id).cloned();
            if let Some(note) = note {
                if state
                    .borrow()
                    .store
                    .move_note(&note, target.clone())
                    .is_ok()
                {
                    n += 1;
                }
            }
        }
        // Moving the last note out of a folder should remove the now-empty
        // shell. The helper is safe: it rechecks the store and refuses to prune
        // a folder that still contains any note.
        for old in old_folders {
            if target.as_deref() != Some(old.as_str()) {
                let _ = state.borrow().store.prune_folder_if_empty(&old);
            }
        }
        finish_bulk(&state, &widgets);
        toast(&widgets, &format!("Moved {n}"));
        dlg.close();
    });
    dialog.present(Some(&parent));
}

/// Fills the colour popover with one button per palette entry. Clicking sets
/// the selected note's colour and closes the popover.
pub fn build_color_swatches(state: &Shared, widgets: &Rc<Widgets>) {
    let (state, widgets) = (state.clone(), widgets.clone());
    let container = widgets.color_box.clone();
    fill_swatch_grid(&container, move |hex| {
        set_note_color(&state, &widgets, hex);
    });
}

/// Empties `container` and fills it with the palette grid. Clicking a swatch
/// calls `on_pick` with the chosen hex (None = default) and closes the popover.
fn fill_swatch_grid(container: &gtk::Box, on_pick: impl Fn(Option<String>) + 'static) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }

    let grid = gtk::Grid::builder()
        .row_spacing(4)
        .column_spacing(4)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();

    let on_pick = Rc::new(on_pick);
    for (i, (name, hex)) in super::window::PALETTE.iter().enumerate() {
        let swatch = gtk::Button::builder()
            .tooltip_text(*name)
            .width_request(28)
            .height_request(28)
            .css_classes(vec!["circular"])
            .build();

        // A CSS provider per swatch paints its fill; "Default" shows a ring.
        let provider = gtk::CssProvider::new();
        let css = match hex {
            Some(h) => format!("button {{ background: {h}; }}"),
            None => "button { background: transparent; }".to_string(),
        };
        provider.load_from_data(&css);
        swatch
            .style_context()
            .add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        if hex.is_none() {
            swatch.set_icon_name("edit-clear-symbolic");
        }

        let target = hex.map(|h| h.to_string());
        {
            let on_pick = on_pick.clone();
            swatch.connect_clicked(move |btn| {
                on_pick(target.clone());
                if let Some(pop) = btn
                    .ancestor(gtk::Popover::static_type())
                    .and_downcast::<gtk::Popover>()
                {
                    pop.popdown();
                }
            });
        }
        grid.attach(&swatch, (i % 4) as i32, (i / 4) as i32, 1, 1);
    }
    container.append(&grid);
}

fn set_note_color(state: &Shared, widgets: &Rc<Widgets>, hex: Option<String>) {
    commit(state, widgets, |note| note.color = hex.clone());
}

/// Flashes "Saving…" then "Saved" so autosave is visible. Called after each save.
pub fn mark_saved(widgets: &Rc<Widgets>) {
    widgets.save_indicator.remove_css_class("error");
    widgets.save_indicator.set_label("Saved");
}

fn mark_save_failed(widgets: &Rc<Widgets>) {
    widgets.save_indicator.add_css_class("error");
    widgets.save_indicator.set_label("Save failed");
}

/// Greys out undo/redo when the buffer has nothing to undo or redo.
/// Syncs undo/redo button sensitivity to the buffer. Called on note load; live
/// edits update via the buffer's can-undo/can-redo notify signals in connect().
pub fn update_undo_state(widgets: &Rc<Widgets>) {
    widgets.undo_button.set_sensitive(widgets.buffer.can_undo());
    widgets.redo_button.set_sensitive(widgets.buffer.can_redo());
}

/// What a sidebar 3-dot menu acts on.
enum RowKind {
    Folder(String),
    Tag(String),
}

/// Adds a visible ⋮ menu button to a sidebar row (folder or tag). Replaces the
/// old hidden right-click — a menu you can't see isn't a feature.
fn attach_row_menu(row: &adw::ActionRow, state: &Shared, widgets: &Rc<Widgets>, kind: RowKind) {
    let popover = gtk::Popover::builder().has_arrow(true).build();
    let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 0);

    let delete = gtk::Button::builder()
        .label(match kind {
            RowKind::Folder(_) => "Delete folder",
            RowKind::Tag(_) => "Delete tag",
        })
        .css_classes(vec!["flat"])
        .build();
    menu_box.append(&delete);
    popover.set_child(Some(&menu_box));

    let button = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .css_classes(vec!["flat"])
        .valign(gtk::Align::Center)
        .popover(&popover)
        .tooltip_text("More")
        .build();
    // Keep the menu out of the way until the row is hovered/focused, so the
    // sidebar stays clean.
    row.add_suffix(&button);

    {
        let (state, widgets) = (state.clone(), widgets.clone());
        let popover = popover.clone();
        delete.connect_clicked(move |_| {
            popover.popdown();
            match &kind {
                RowKind::Folder(f) => confirm_delete_folder(&state, &widgets, f),
                RowKind::Tag(t) => confirm_delete_tag(&state, &widgets, t),
            }
        });
    }
}

/// Confirms, then removes a tag from every note that has it.
fn confirm_delete_tag(state: &Shared, widgets: &Rc<Widgets>, tag: &str) {
    let count = state
        .borrow()
        .notes
        .iter()
        .filter(|n| n.labels.iter().flatten().any(|l| l == tag))
        .count();

    let dialog = adw::AlertDialog::builder()
        .heading(format!("Delete tag \"{tag}\"?"))
        .body(format!(
            "The tag will be removed from {count} note(s). The notes stay."
        ))
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("delete", "Delete tag");
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_close_response("cancel");

    let (state, widgets, tag) = (state.clone(), widgets.clone(), tag.to_string());
    let parent = widgets.window.clone();
    dialog.connect_response(None, move |_, response| {
        if response != "delete" {
            return;
        }
        let result = state.borrow().store.delete_tag(&tag);
        match result {
            Ok(n) => {
                {
                    let mut s = state.borrow_mut();
                    if s.tag.as_deref() == Some(&tag) {
                        s.apply_target(&super::window::SidebarTarget::Notes);
                    }
                    s.reload();
                }
                refresh_sidebar(&state, &widgets);
                refresh_list(&state, &widgets);
                toast(&widgets, &format!("Tag removed from {n} note(s)"));
            }
            Err(e) => toast(&widgets, &format!("Could not delete tag: {e}")),
        }
    });
    dialog.present(Some(&parent));
}

/// Confirms, then deletes a folder — moving its notes back to the top level
/// (never deleting notes).
fn confirm_delete_folder(state: &Shared, widgets: &Rc<Widgets>, folder: &str) {
    let count = state
        .borrow()
        .notes
        .iter()
        .filter(|n| n.folder_name.as_deref() == Some(folder))
        .count();

    let dialog = adw::AlertDialog::builder()
        .heading(format!("Delete folder \"{folder}\"?"))
        .body(if count == 0 {
            "The folder is empty.".to_string()
        } else {
            format!("{count} note(s) will move to the top level. No notes are deleted.")
        })
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("delete", "Delete folder");
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_close_response("cancel");

    let (state, widgets, folder) = (state.clone(), widgets.clone(), folder.to_string());
    let parent = widgets.window.clone();
    dialog.connect_response(None, move |_, response| {
        if response != "delete" {
            return;
        }
        let result = state.borrow().store.delete_folder(&folder);
        match result {
            Ok(moved) => {
                {
                    let mut s = state.borrow_mut();
                    // If we were viewing that folder, fall back to All notes.
                    if s.folder.as_deref() == Some(&folder) {
                        s.apply_target(&super::window::SidebarTarget::Notes);
                    }
                    s.reload();
                }
                refresh_sidebar(&state, &widgets);
                refresh_list(&state, &widgets);
                toast(
                    &widgets,
                    &format!("Folder deleted, {moved} note(s) moved out"),
                );
            }
            Err(e) => toast(&widgets, &format!("Could not delete folder: {e}")),
        }
    });
    dialog.present(Some(&parent));
}

/// Confirms, then permanently deletes every trashed note.
pub fn confirm_empty_trash(state: &Shared, widgets: &Rc<Widgets>) {
    let count = state
        .borrow()
        .notes
        .iter()
        .filter(|n| n.is_trashed())
        .count();
    if count == 0 {
        toast(widgets, "Trash is already empty");
        return;
    }

    let dialog = adw::AlertDialog::builder()
        .heading("Empty trash?")
        .body(format!(
            "{count} note(s) will be permanently deleted. This cannot be undone."
        ))
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("empty", "Delete permanently");
    dialog.set_response_appearance("empty", adw::ResponseAppearance::Destructive);
    dialog.set_close_response("cancel");

    let (state, widgets) = (state.clone(), widgets.clone());
    let parent = widgets.window.clone();
    dialog.connect_response(None, move |_, response| {
        if response != "empty" {
            return;
        }
        let result = state.borrow().store.empty_trash();
        match result {
            Ok(n) => {
                {
                    let mut s = state.borrow_mut();
                    s.selected = None;
                    s.reload();
                }
                refresh_sidebar(&state, &widgets);
                refresh_list(&state, &widgets);
                load_editor(&state, &widgets);
                toast(&widgets, &format!("Deleted {n} note(s)"));
            }
            Err(e) => toast(&widgets, &format!("Could not empty trash: {e}")),
        }
    });
    dialog.present(Some(&parent));
}
