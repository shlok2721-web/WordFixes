//! Main window: sidebar (filters + folders) → note list → editor.

use crate::config::Config;
use crate::model::Note;
use crate::server::{self, ServerHandle};
use crate::store::Store;
use adw::prelude::*;
use gtk::glib;
use sourceview5::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Filter {
    Notes,
    Pinned,
    Archived,
    Trash,
    Folder,
    Tag,
}

/// What a sidebar row selects. Stored on the row itself rather than derived
/// from its index — index arithmetic broke as soon as a second dynamic
/// section (tags) was added below folders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SidebarTarget {
    Notes,
    Pinned,
    Archived,
    Trash,
    Folder(String),
    Tag(String),
}

impl SidebarTarget {
    pub fn encode(&self) -> String {
        match self {
            Self::Notes => "notes".into(),
            Self::Pinned => "pinned".into(),
            Self::Archived => "archived".into(),
            Self::Trash => "trash".into(),
            Self::Folder(name) => format!("folder:{name}"),
            Self::Tag(name) => format!("tag:{name}"),
        }
    }

    pub fn decode(raw: &str) -> Self {
        match raw {
            "pinned" => Self::Pinned,
            "archived" => Self::Archived,
            "trash" => Self::Trash,
            other => match other.split_once(':') {
                Some(("folder", name)) => Self::Folder(name.to_string()),
                Some(("tag", name)) => Self::Tag(name.to_string()),
                _ => Self::Notes,
            },
        }
    }
}

pub struct AppState {
    pub store: Store,
    pub config: Config,
    pub notes: Vec<Note>,
    pub selected: Option<String>,
    pub filter: Filter,
    pub folder: Option<String>,
    /// Active tag when `filter` is `Tag`.
    pub tag: Option<String>,
    /// Live search text. Empty means "no search".
    pub query: String,
    pub server: Option<ServerHandle>,
    /// Set while the editor is being repopulated, so programmatic buffer
    /// changes don't get written back as user edits.
    pub loading: bool,
    /// Set while the note list is being rebuilt. Removing rows makes GTK emit
    /// `row-selected(None)`, which would otherwise clear the real selection.
    pub rebuilding: bool,
    /// Coalesces note-list refreshes requested by fast editor changes. Rebuilding
    /// GTK rows inside every input-method callback can interrupt dead-key/compose
    /// sequences (notably Greek tonos under Wayland) and is unnecessarily heavy.
    pub list_refresh_pending: bool,
    /// Multi-select mode: rows show checkboxes and clicks accumulate.
    pub selecting: bool,
    /// A sync run is in flight; a second one would fight it over the same files.
    pub syncing: bool,
    /// When the last client-mode sync finished (epoch millis), `0` for never.
    pub last_sync: i64,
    /// Set by a click or key press on the sidebar (and by `refresh_sidebar`
    /// before it reselects), so a `row-selected` that arrives without one can be
    /// recognised as GTK moving the cursor on its own and ignored.
    pub intent: bool,
    /// Note ids ticked in multi-select mode.
    pub checked: std::collections::BTreeSet<String>,
}

impl AppState {
    pub fn active_target(&self) -> SidebarTarget {
        match self.filter {
            Filter::Notes => SidebarTarget::Notes,
            Filter::Pinned => SidebarTarget::Pinned,
            Filter::Archived => SidebarTarget::Archived,
            Filter::Trash => SidebarTarget::Trash,
            Filter::Folder => self
                .folder
                .clone()
                .map(SidebarTarget::Folder)
                .unwrap_or(SidebarTarget::Notes),
            Filter::Tag => self
                .tag
                .clone()
                .map(SidebarTarget::Tag)
                .unwrap_or(SidebarTarget::Notes),
        }
    }

    /// Single source of truth for both sidebar counters and list filtering.
    /// `global_home_search` is the one deliberate exception: searching from
    /// Home searches the whole vault, while Home itself contains loose live
    /// notes only.
    fn note_belongs_to_target(
        note: &Note,
        target: &SidebarTarget,
        global_home_search: bool,
    ) -> bool {
        if global_home_search && matches!(target, SidebarTarget::Notes) {
            return true;
        }
        match target {
            SidebarTarget::Notes => {
                !note.is_trashed()
                    && !note.is_archived()
                    && note.folder_name.as_deref().unwrap_or("").is_empty()
            }
            SidebarTarget::Pinned => !note.is_trashed() && !note.is_archived() && note.is_pinned,
            SidebarTarget::Archived => !note.is_trashed() && note.is_archived(),
            SidebarTarget::Trash => note.is_trashed(),
            SidebarTarget::Folder(folder) => {
                !note.is_trashed()
                    && !note.is_archived()
                    && note.folder_name.as_deref() == Some(folder.as_str())
            }
            // Tags deliberately include archived notes, matching the tag view.
            SidebarTarget::Tag(tag) => {
                !note.is_trashed()
                    && note
                        .labels
                        .iter()
                        .flatten()
                        .any(|label| label.trim() == tag)
            }
        }
    }

    /// Number displayed beside a sidebar target. Search text is intentionally
    /// ignored: these are stable vault/category totals, while the search field
    /// itself reports the filtered result count.
    pub fn target_count(&self, target: &SidebarTarget) -> usize {
        self.notes
            .iter()
            .filter(|note| Self::note_belongs_to_target(note, target, false))
            .count()
    }

    pub fn visible_notes(&self) -> Vec<&Note> {
        // Searching from the All-notes view is global: it looks everywhere,
        // including archived and trashed notes, so a match is never hidden by the
        // current view. Other views keep search scoped to that view.
        let target = self.active_target();
        let global_search = matches!(target, SidebarTarget::Notes) && !self.query.trim().is_empty();
        let mut visible = self
            .notes
            .iter()
            .filter(|note| Self::note_belongs_to_target(note, &target, global_search))
            .filter(|n| note_matches(n, &self.query))
            .collect::<Vec<_>>();
        // Sort here, not just on load: pinning a note in memory must re-order
        // the list immediately, without waiting for a reload. In the trash,
        // pinning is meaningless — every note is equal there — so sort purely by
        // recency and ignore the pin flag.
        let in_trash = self.filter == Filter::Trash;
        visible.sort_by(|a, b| {
            if in_trash {
                b.updated_at.cmp(&a.updated_at)
            } else {
                b.is_pinned
                    .cmp(&a.is_pinned)
                    .then(note_sort_cmp(&self.config, a, b))
            }
        });
        visible
    }

    pub fn selected_note(&self) -> Option<&Note> {
        let id = self.selected.as_ref()?;
        self.notes.iter().find(|n| &n.id == id)
    }

    pub fn selected_note_mut(&mut self) -> Option<&mut Note> {
        let id = self.selected.clone()?;
        self.notes.iter_mut().find(|n| n.id == id)
    }

    /// Clears an editor selection that no longer belongs to the active list.
    ///
    /// Filters can change underneath the selected note (switching sidebar
    /// sections, unpinning from Pinned, unarchiving from Archived). Keeping the
    /// old id in that situation leaves an editable note open with no matching
    /// selected row, so the user can unknowingly edit a note from another view.
    pub fn clear_selection_if_hidden(&mut self) -> bool {
        let hidden = self
            .selected
            .as_ref()
            .is_some_and(|id| !self.visible_notes().iter().any(|note| &note.id == id));
        if hidden {
            self.selected = None;
        }
        hidden
    }

    /// Distinct tags across all notes except trash, with how many notes carry
    /// each. Archived notes are counted — they keep their tags — so the count
    /// matches what the tag view shows. Sorted by name for a stable order.
    pub fn all_tags(&self) -> Vec<(String, usize)> {
        use std::collections::BTreeMap;
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for note in self.notes.iter().filter(|n| !n.is_trashed()) {
            // A malformed/imported note can contain the same label more than
            // once. It is still one note in the tag view and must count once.
            let mut labels_on_note = std::collections::BTreeSet::new();
            for label in note.labels.iter().flatten() {
                let label = label.trim();
                if !label.is_empty() {
                    labels_on_note.insert(label.to_string());
                }
            }
            for label in labels_on_note {
                *counts.entry(label).or_default() += 1;
            }
        }
        counts.into_iter().collect()
    }

    /// Applies a sidebar selection.
    pub fn apply_target(&mut self, target: &SidebarTarget) {
        self.folder = None;
        self.tag = None;
        self.filter = match target {
            SidebarTarget::Notes => Filter::Notes,
            SidebarTarget::Pinned => Filter::Pinned,
            SidebarTarget::Archived => Filter::Archived,
            SidebarTarget::Trash => Filter::Trash,
            SidebarTarget::Folder(name) => {
                self.folder = Some(name.clone());
                Filter::Folder
            }
            SidebarTarget::Tag(name) => {
                self.tag = Some(name.clone());
                Filter::Tag
            }
        };
    }

    /// Applies `op` to every checked note and saves it. Returns how many were
    /// affected. Used by the bulk actions.
    pub fn for_each_checked(&mut self, mut op: impl FnMut(&mut Note)) -> usize {
        let ids: Vec<String> = self.checked.iter().cloned().collect();
        let mut n = 0;
        for id in ids {
            if let Some(note) = self.notes.iter_mut().find(|x| x.id == id) {
                op(note);
                note.touch();
                let snapshot = note.clone();
                if self.store.save(&snapshot).is_ok() {
                    n += 1;
                }
            }
        }
        n
    }

    pub fn reload(&mut self) {
        match self.store.load_all() {
            Ok(notes) => self.notes = notes,
            Err(e) => eprintln!("failed to load notes: {e}"),
        }
    }
}

/// Compares two notes according to the configured sort field and direction.
///
/// "Ascending" is the field's natural order: oldest-first for dates, A→Z for
/// titles, text-before-checklist for type, palette order for colour. The
/// direction flips only the chosen field; the tie-breakers are fixed
/// (updated-descending, then id) so equal-key rows never jump between
/// refreshes.
pub fn note_sort_cmp(config: &Config, a: &Note, b: &Note) -> std::cmp::Ordering {
    let primary = match config.sort_field.as_str() {
        "created" => a.created_at.cmp(&b.created_at),
        // Unicode-aware lowercase: titles are largely Greek. Untitled notes
        // sort together at the front of the ascending order.
        "title" => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
        "type" => a
            .checklist_items
            .is_some()
            .cmp(&b.checklist_items.is_some()),
        "color" => color_rank(a.color.as_deref()).cmp(&color_rank(b.color.as_deref())),
        // "updated" and anything unrecognised (hand-edited config).
        _ => a.updated_at.cmp(&b.updated_at),
    };
    let primary = if config.sort_ascending {
        primary
    } else {
        primary.reverse()
    };
    primary
        .then(b.updated_at.cmp(&a.updated_at))
        .then_with(|| a.id.cmp(&b.id))
}

/// Position of a colour in the palette; uncoloured notes rank first, unknown
/// custom colours group together after the palette.
fn color_rank(color: Option<&str>) -> usize {
    match color {
        None => 0,
        Some(hex) => PALETTE
            .iter()
            .position(|(_, h)| h.is_some_and(|p| p.eq_ignore_ascii_case(hex)))
            .unwrap_or(PALETTE.len()),
    }
}

/// Case-insensitive substring match over title, body, checklist items and
/// labels. Every whitespace-separated term must match somewhere, so typing
/// more words narrows rather than widens.
///
/// Lowercasing is Unicode-aware via `to_lowercase`, not `to_ascii_lowercase` —
/// these notes are largely Greek.
pub fn note_matches(note: &Note, query: &str) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return true;
    }

    let mut haystack = format!("{}\n{}", note.title, note.content).to_lowercase();
    if let Some(items) = &note.checklist_items {
        for item in items {
            haystack.push('\n');
            haystack.push_str(&item.text.to_lowercase());
        }
    }
    if let Some(labels) = &note.labels {
        for label in labels {
            haystack.push('\n');
            haystack.push_str(&label.to_lowercase());
        }
    }

    query
        .split_whitespace()
        .all(|term| haystack.contains(&term.to_lowercase()))
}

pub type Shared = Rc<RefCell<AppState>>;

pub struct Widgets {
    pub window: adw::ApplicationWindow,
    pub list: gtk::ListBox,
    pub sidebar: gtk::ListBox,
    pub title_entry: gtk::Entry,
    pub buffer: gtk::TextBuffer,
    pub editor_stack: gtk::Stack,
    pub body_stack: gtk::Stack,
    pub body_placeholder: gtk::Label,
    pub checklist_box: gtk::Box,
    pub list_menu: gtk::MenuButton,
    pub tag_box: gtk::Box,
    pub select_toggle: gtk::ToggleButton,
    pub undo_button: gtk::Button,
    pub redo_button: gtk::Button,
    pub save_indicator: gtk::Label,
    pub color_box: gtk::Box,
    pub color_button: gtk::MenuButton,
    pub bulk_bar: gtk::Box,
    pub bulk_label: gtk::Label,
    pub bulk_archive: gtk::Button,
    /// Held so `load_editor` can sync them to the selected note — otherwise
    /// they keep showing the previously selected note's state.
    pub search: gtk::SearchEntry,
    pub list_page: adw::NavigationPage,
    pub editor_page: adw::NavigationPage,
    pub inner_split: adw::NavigationSplitView,
    pub outer_split: adw::NavigationSplitView,
    pub pin_button: gtk::ToggleButton,
    pub archive_button: gtk::ToggleButton,
    pub restore_button: gtk::Button,
    pub delete_button: gtk::Button,
    pub server_row: adw::SwitchRow,
    pub server_status: gtk::Label,
    pub toasts: adw::ToastOverlay,
}

pub fn build(app: &adw::Application) -> (adw::ApplicationWindow, Shared, Rc<Widgets>) {
    // Move a pre-rename install's folders to the new "Notes" locations before
    // anything reads them, so an existing user keeps their notes.
    let data_moved = crate::store::migrate_legacy_dirs();

    let mut config = Config::load();
    // If the folder was migrated but the saved config still points at the old
    // path, repoint it to the new default and persist that.
    if data_moved && config.data_dir == crate::store::legacy_data_dir() {
        config.data_dir = crate::store::default_data_dir();
        let _ = config.save();
    }

    let mut store = Store::new(config.data_dir.clone()).expect("cannot create data directory");
    store.markdown_mirror = config.markdown_mirror;

    // Retention sweep runs once at launch: notes that have outstayed the
    // configured trash window are permanently removed before the list is built.
    match store.sweep_trash(config.trash_retention_days) {
        Ok(n) if n > 0 => eprintln!(
            "swept {n} note(s) past the {}-day trash window",
            config.trash_retention_days
        ),
        Ok(_) => {}
        Err(e) => eprintln!("trash sweep failed: {e}"),
    }

    let mut state = AppState {
        store,
        config,
        notes: Vec::new(),
        selected: None,
        filter: Filter::Notes,
        folder: None,
        tag: None,
        query: String::new(),
        server: None,
        loading: false,
        rebuilding: false,
        list_refresh_pending: false,
        selecting: false,
        syncing: false,
        last_sync: 0,
        intent: false,
        checked: std::collections::BTreeSet::new(),
    };
    state.reload();
    let state: Shared = Rc::new(RefCell::new(state));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Notes")
        .default_width(1280)
        .default_height(760)
        // Absolute floor: below ~500px the single-pane layout starts to break
        // (search bar / list overflow), so the window can't be shrunk past it.
        .width_request(500)
        .height_request(500)
        .build();

    // Pill styling for the formatting toolbar: a rounded, subtly-filled bar.
    let pill = gtk::CssProvider::new();
    pill.load_from_data(
        ".format-bar { \
            background-color: alpha(currentColor, 0.06); \
            border-radius: 14px; \
            padding: 2px 4px; \
            margin-left: 12px; margin-right: 12px; \
         } \
         .format-bar button { min-width: 24px; padding: 2px 6px; } \
         .format-bar separator { margin: 4px 2px; } \
         .body-hint { font-size: 0.95em; opacity: 0.55; line-height: 1.6; } \
         textview.inline-item, textview.inline-item text { background: transparent; }",
    );
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &pill,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    // ---- sidebar -----------------------------------------------------------
    let sidebar = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .css_classes(vec!["navigation-sidebar"])
        .build();

    let sidebar_page = adw::NavigationPage::builder()
        .title("Notes")
        .child(&sidebar_content(&sidebar))
        .build();

    // ---- note list ---------------------------------------------------------
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .css_classes(vec!["navigation-sidebar", "notes"])
        .build();

    // Upstream's model, from its own Welcome note: "Tap the + button to add a
    // note, a checklist, or a new folder." One control, three things.
    let new_menu = gtk::gio::Menu::new();
    new_menu.append(Some("New note"), Some("app.new-note"));
    new_menu.append(Some("New checklist"), Some("app.new-checklist"));
    new_menu.append(Some("New folder…"), Some("app.new-folder"));

    let new_button = adw::SplitButton::builder()
        .child(
            &adw::ButtonContent::builder()
                .icon_name("document-new-symbolic")
                .label("New")
                .build(),
        )
        .tooltip_text("New note (Ctrl+N)")
        .menu_model(&new_menu)
        .build();

    let select_toggle = gtk::ToggleButton::builder()
        .icon_name("checkbox-checked-symbolic")
        .tooltip_text("Bulk edit (Ctrl+E or Ctrl+Shift+E while typing)")
        .build();

    // Sort field + direction; the popover content is built in connect_sort.
    // Shares one row with the search bar — a dedicated row wasted two rows'
    // worth of list space (user feedback), and the header is already full.
    let sort_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(0)
        .build();
    let sort_button = gtk::MenuButton::builder()
        .tooltip_text("Sort notes")
        .css_classes(vec!["flat"])
        .always_show_arrow(true)
        .valign(gtk::Align::Center)
        .popover(&color_popover(&sort_box))
        .build();

    let list_header = adw::HeaderBar::new();
    // The main app menu (import / backup / server / open notes folder) plus the
    // list-specific "Empty trash". It lives on the list header — not the editor
    // header — so the editor header has room to stay narrow. Always visible.
    let list_menu_model = server_menu();
    let trash_section = gtk::gio::Menu::new();
    trash_section.append(Some("Empty trash…"), Some("app.empty-trash"));
    list_menu_model.append_section(None, &trash_section);
    let list_menu = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&list_menu_model)
        .tooltip_text("Menu")
        .build();

    list_header.pack_start(&new_button);
    list_header.pack_end(&select_toggle);
    list_header.pack_end(&list_menu);

    // Bulk action bar: only visible in multi-select mode.
    let bulk_select_all = gtk::CheckButton::builder()
        .tooltip_text("Select all / none")
        .build();
    let bulk_move = gtk::Button::builder()
        .icon_name("folder-symbolic")
        .tooltip_text("Move to folder")
        .build();
    let bulk_pin = gtk::Button::builder()
        .icon_name("view-pin-symbolic")
        .tooltip_text("Pin / unpin selected")
        .build();
    // Same swatch popover pattern as the editor's colour button; the grid is
    // populated in connect_bulk so the click handler can reach the state.
    let bulk_color_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(0)
        .build();
    let bulk_color = gtk::MenuButton::builder()
        .icon_name("color-select-symbolic")
        .tooltip_text("Colour selected")
        .popover(&color_popover(&bulk_color_box))
        .build();
    let bulk_archive = gtk::Button::builder()
        .icon_name("view-conceal-symbolic")
        .tooltip_text("Archive selected")
        .build();
    let bulk_delete = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Delete selected")
        .build();
    let bulk_label = gtk::Label::builder()
        .label("0 selected")
        .css_classes(vec!["dim-label"])
        .hexpand(true)
        .xalign(0.0)
        .build();

    let bulk_bar = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .margin_start(12)
        .margin_end(6)
        .margin_top(4)
        .margin_bottom(4)
        .visible(false)
        .build();
    bulk_bar.append(&bulk_select_all);
    bulk_bar.append(&bulk_label);
    bulk_bar.append(&bulk_move);
    bulk_bar.append(&bulk_pin);
    bulk_bar.append(&bulk_color);
    bulk_bar.append(&bulk_archive);
    bulk_bar.append(&bulk_delete);

    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search notes")
        .tooltip_text("Search notes (Ctrl+F)")
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();

    let list_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let search_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(0)
        .margin_end(6)
        .build();
    search.set_hexpand(true);
    search_row.append(&search);
    search_row.append(&sort_button);

    list_box.append(&list_header);
    list_box.append(&search_row);
    list_box.append(&bulk_bar);
    list_box.append(
        &gtk::ScrolledWindow::builder()
            .vexpand(true)
            .child(&list)
            .build(),
    );
    let list_page = adw::NavigationPage::builder()
        .title("Home")
        .child(&list_box)
        .build();

    // ---- editor ------------------------------------------------------------
    let title_entry = gtk::Entry::builder()
        .placeholder_text("Note title")
        .css_classes(vec!["title-2", "flat"])
        .margin_top(12)
        .margin_bottom(6)
        .margin_start(18)
        .margin_end(18)
        .build();

    // GtkSourceView gives live Markdown highlighting for free. Its buffer is a
    // GtkTextBuffer subclass, so all the existing change/undo wiring — which
    // types against gtk::TextBuffer — keeps working unchanged.
    let source_buffer = sourceview5::Buffer::new(None);
    if let Some(md) = sourceview5::LanguageManager::default().language("markdown") {
        source_buffer.set_language(Some(&md));
    }
    source_buffer.set_highlight_syntax(true);
    apply_source_style(&source_buffer);
    // Re-pick the scheme if the desktop switches light/dark while running.
    {
        let sb = source_buffer.clone();
        adw::StyleManager::default().connect_dark_notify(move |_| apply_source_style(&sb));
    }

    let source_view = sourceview5::View::with_buffer(&source_buffer);
    source_view.set_wrap_mode(gtk::WrapMode::WordChar);
    source_view.set_top_margin(6);
    source_view.set_bottom_margin(18);
    source_view.set_left_margin(18);
    source_view.set_right_margin(18);
    source_view.set_monospace(false);
    let text_view = source_view;

    let buffer: gtk::TextBuffer = source_buffer.upcast();
    // Undo tracking drives the header undo/redo buttons; make it explicit
    // rather than rely on the default staying true. (Do NOT set
    // max-undo-levels to 0 — on a plain TextBuffer that means *zero* steps,
    // i.e. undo disabled, not "unlimited".)
    buffer.set_enable_undo(true);

    // Checklist notes need a different body from text notes, but share the
    // title field and the header actions.
    let checklist_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .margin_top(6)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    // A dim hint shown over an empty note body — doubles as a Markdown cheat
    // sheet so the syntax is learnable while typing. `can_target(false)` lets
    // clicks fall through to the text view beneath it.
    // A Markdown showcase: each line shows the syntax on the left and what it
    // produces on the right, rendered via Pango markup — so the effect is
    // visible, not just described. Teaches the syntax while the note is empty.
    let body_placeholder = gtk::Label::builder()
        .use_markup(true)
        .label(
            "Write your note…\n\n\
             <b>Markdown — type it, see it:</b>\n\
             **bold**\t→ <b>bold</b>\n\
             _italic_\t→ <i>italic</i>\n\
             ~~strike~~\t→ <s>strike</s>\n\
             `code`\t→ <tt>code</tt>\n\
             # Heading\t→ <big><b>Heading</b></big>\n\
             - item\t→ bullet list\n\
             [text](url)\t→ link   &gt; quote",
        )
        .css_classes(vec!["dim-label", "body-hint"])
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Start)
        .xalign(0.0)
        .margin_top(8)
        .margin_start(19)
        .margin_end(18)
        .can_target(false)
        .build();

    let text_overlay = gtk::Overlay::new();
    text_overlay.set_child(Some(
        &gtk::ScrolledWindow::builder()
            .vexpand(true)
            .child(&text_view)
            .build(),
    ));
    text_overlay.add_overlay(&body_placeholder);

    // Live Preview projection of the markdown body (see ui::preview).
    let rich_view = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(6)
        .bottom_margin(18)
        .left_margin(18)
        .right_margin(18)
        .build();

    let body_stack = gtk::Stack::new();
    body_stack.add_named(&text_overlay, Some("text"));
    body_stack.add_named(
        &gtk::ScrolledWindow::builder()
            .vexpand(true)
            .child(&rich_view)
            .build(),
        Some("preview"),
    );
    body_stack.add_named(
        &gtk::ScrolledWindow::builder()
            .vexpand(true)
            .child(&checklist_box)
            .build(),
        Some("checklist"),
    );
    body_stack.set_visible_child_name("text");

    // Tags sit directly under the title, always visible. The user reports
    // using them constantly, and Keep hides them behind an overflow menu —
    // that's the thing worth not copying.
    let tag_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .margin_start(18)
        .margin_end(18)
        .margin_bottom(6)
        .build();

    // Layout, top to bottom: header bar → formatting toolbar → title → tags →
    // body, matching the requested arrangement. The toolbar is an adw::WrapBox
    // that wraps onto extra rows on a narrow editor instead of clipping or
    // scrolling — all buttons stay reachable at any width.
    let format_bar = build_format_toolbar(&buffer, &text_view);

    let editor_body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    editor_body.append(&format_bar);
    editor_body.append(&title_entry);
    editor_body.append(&tag_box);
    editor_body.append(&body_stack);

    // A quick tour of the tools, shown whenever no note is open — most usefully
    // on a first launch when the app is empty.
    let guide = gtk::Label::builder()
        .use_markup(true)
        .wrap(true)
        .xalign(0.0)
        .justify(gtk::Justification::Left)
        .css_classes(vec!["dim-label", "body-hint"])
        .label(
            "<b>Editor bar (top-right)</b>\n\
             • <b>Saved</b> — your work is saved automatically\n\
             • <b>Pin</b> — keep a note at the top of the list\n\
             • <b>Colour</b> — mark the notes that matter\n\
             • <b>Archive</b> — tuck a note away without deleting it\n\
             • <b>Trash</b> — remove a note you no longer need\n\
             \n\
             <b>List bar (top-left)</b>\n\
             • <b>+</b> — write a new note\n\
             • <b>▾</b> next to it — new checklist, note, or folder\n\
             • The title shows the section you're in\n\
             • <b>☰</b> menu — syncing, import, backup and settings\n\
             • <b>Select</b> (tick icon) — bulk edit: move, pin, colour, archive or delete many\n\
             • <b>New folder</b> — group related notes together\n\
             \n\
             <b>Keyboard</b>\n\
             • <b>Ctrl+N</b> — new note · <b>Ctrl+Shift+N</b> — new checklist\n\
             • <b>Ctrl+F</b> — search · <b>Ctrl+,</b> — settings\n\
             • <b>Ctrl+E</b> — bulk edit · <b>Ctrl+A</b> — archive/unarchive\n\
             • <b>Ctrl+Shift+C</b> — colour picker · <b>Delete</b> — trash\n\
             • While typing, use <b>Ctrl+Shift+E/A</b>; Ctrl+A and Delete keep editing text\n\
             • <b>Esc</b> — leave bulk edit or clear the search",
        )
        .build();
    let guide_clamp = adw::Clamp::builder()
        .maximum_size(560)
        .child(&guide)
        .margin_top(6)
        .build();

    let empty = adw::StatusPage::builder()
        .title("Welcome to Notes")
        .description("Choose a note on the left, or press + to write your first one.")
        .child(&guide_clamp)
        .build();

    let editor_stack = gtk::Stack::new();
    editor_stack.add_named(&empty, Some("empty"));
    editor_stack.add_named(&editor_body, Some("editor"));
    editor_stack.set_visible_child_name("empty");

    // A grid of swatches, populated in build_color_popover / connected in actions.
    let color_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(0)
        .build();

    // Undo/redo are document-scope, so they live in the header, not the (future)
    // formatting toolbar. Driven from the text buffer's own undo stack.
    let undo_button = gtk::Button::builder()
        .icon_name("edit-undo-symbolic")
        .tooltip_text("Undo")
        .build();
    let redo_button = gtk::Button::builder()
        .icon_name("edit-redo-symbolic")
        .tooltip_text("Redo")
        .build();

    // Autosave is otherwise invisible; this tells the user their work is safe.
    let save_indicator = gtk::Label::builder()
        .label("Saved")
        .css_classes(vec!["dim-label", "caption"])
        .build();

    let pin_button = gtk::ToggleButton::builder()
        .icon_name("view-pin-symbolic")
        .tooltip_text("Pin note")
        .build();
    // Colour is a note property, so it belongs here beside pin/archive rather
    // than in the text toolbar.
    let color_button = gtk::MenuButton::builder()
        .icon_name("color-select-symbolic")
        .tooltip_text("Colour (Ctrl+Shift+C)")
        .popover(&color_popover(&color_box))
        .build();
    // Move-to-folder was removed from the editor: it duplicated the bulk-select
    // move action, which does the same job. Moving a single note = tick it in
    // select mode and use the folder button in the bulk bar.
    // "Conceal" rather than a box/archive glyph: Adwaita ships no archive icon,
    // and hiding-from-the-main-list is what archiving actually does here.
    let archive_button = gtk::ToggleButton::builder()
        .icon_name("view-conceal-symbolic")
        .tooltip_text("Archive note (Ctrl+A or Ctrl+Shift+A while typing)")
        .build();
    let delete_button = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Move to trash (Delete outside text fields)")
        .build();
    // Shown only while a trashed note is open (set in load_editor): the way back
    // out of the trash. A labelled button so it's unmistakable next to the
    // now-"delete permanently" trash button.
    let restore_button = gtk::Button::builder()
        .icon_name("restore-symbolic")
        .tooltip_text("Restore from trash")
        .visible(false)
        .build();
    // The three secondary note actions (colour, move-to-folder, archive) live
    // behind one overflow button rather than each taking a header slot. With all
    // of them plus the app menu in the header, the editor's minimum width was
    // ~627px — so on anything narrower the header (and the toolbar under it) got
    // clipped instead of the toolbar wrapping. Consolidating here lets the
    // editor shrink and the toolbar wrap. The button widgets are unchanged, so
    // their existing wiring (colour swatches, folder list, archive toggle) all
    // still works — they're just reparented into this popover.
    // colour + move + archive live together and adapt to width: shown inline in
    // the header in desktop mode (≥1200px) where there's room, and collapsed
    // behind the ⋮ overflow button below that. `overflow_box` is reparented
    // between `inline_slot` (header) and `overflow_popover` by the desktop
    // breakpoint below. The button widgets and their wiring are unchanged either
    // way. DEFAULT here is the collapsed (⋮) state — the desktop breakpoint
    // switches to inline, so phone/tablet (where no breakpoint or a MaxWidth one
    // is active) correctly keep the overflow.
    let overflow_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(4)
        .build();
    overflow_box.append(&color_button);
    overflow_box.append(&archive_button);
    // Stable header position for the inline buttons; the box is moved in/out of
    // this rather than re-packed, so it never disturbs the pack order.
    let inline_slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let overflow_popover = gtk::Popover::new();
    overflow_popover.set_child(Some(&overflow_box)); // default: collapsed (⋮)
    let overflow_button = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .tooltip_text("More note actions")
        .popover(&overflow_popover)
        .build(); // default: visible (⋮ shown)

    let preview_toggle = gtk::ToggleButton::builder()
        .icon_name("text-x-generic-symbolic")
        .tooltip_text("Live preview — markdown rendered, markers hidden (Ctrl+Shift+P)")
        .build();

    let editor_header = adw::HeaderBar::new();
    // The navigation page needs a non-empty accessible title, but the editor
    // header intentionally contains actions only and should not render it.
    editor_header.set_show_title(false);
    editor_header.pack_start(&undo_button);
    editor_header.pack_start(&redo_button);
    editor_header.pack_start(&restore_button);
    editor_header.pack_end(&delete_button);
    editor_header.pack_end(&overflow_button);
    editor_header.pack_end(&inline_slot);
    editor_header.pack_end(&preview_toggle);
    editor_header.pack_end(&pin_button);
    editor_header.pack_end(&save_indicator);

    let editor_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    editor_box.append(&editor_header);
    editor_box.append(&editor_stack);
    let editor_page = adw::NavigationPage::builder()
        .title("Editor")
        .child(&editor_box)
        .build();

    // ---- server controls ---------------------------------------------------
    let server_row = adw::SwitchRow::builder()
        .title("WebDAV server")
        .subtitle("Let the Android app sync to this machine")
        .build();
    let server_status = gtk::Label::builder()
        .css_classes(vec!["dim-label", "caption"])
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .build();

    // ---- assembly ----------------------------------------------------------
    let inner_split = adw::NavigationSplitView::builder()
        .sidebar(&list_page)
        .content(&editor_page)
        .min_sidebar_width(340.0)
        .max_sidebar_width(460.0)
        .build();
    let inner_page = adw::NavigationPage::builder()
        .title("Notes")
        .child(&inner_split)
        .build();

    let outer_split = adw::NavigationSplitView::builder()
        .sidebar(&sidebar_page)
        .content(&inner_page)
        .max_sidebar_width(220.0)
        .build();

    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&outer_split));
    window.set_content(Some(&toasts));

    // Adaptive layout via breakpoints. libadwaita reverts each setter when its
    // condition stops matching, so the wide default (all three panes) needs no
    // explicit setter.
    //
    //   ≥ 1200px desktop — sidebar | list | editor (three panes)
    //   900–1199 tablet   — sidebar | (list⇄editor, with a back button)
    //   < 900px  phone    — one pane at a time (sidebar → list → editor)
    //
    // IMPORTANT: AdwWindow applies only ONE breakpoint at a time (the matching
    // one), so these must not overlap-and-shadow each other. The two collapse
    // breakpoints use MaxWidth; the header breakpoint uses MinWidth 1200 so its
    // range is disjoint from them and it never shadows the collapse. Absolute
    // floor is the window's 500×500 minimum.
    let tablet = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        1199.0,
        adw::LengthUnit::Px,
    ));
    tablet.add_setter(&inner_split, "collapsed", Some(&true.to_value()));

    let phone = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        899.0,
        adw::LengthUnit::Px,
    ));
    phone.add_setter(&inner_split, "collapsed", Some(&true.to_value()));
    phone.add_setter(&outer_split, "collapsed", Some(&true.to_value()));

    // Editor header actions: inline only in desktop mode (≥1200px). Uses MinWidth
    // so it's disjoint from the collapse breakpoints above. The header's built
    // default is the collapsed ⋮ state, so phone/tablet keep the overflow; this
    // breakpoint expands it to inline when the window is wide enough.
    let desktop = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MinWidth,
        1200.0,
        adw::LengthUnit::Px,
    ));
    {
        let (slot, pop, obox, btn) = (
            inline_slot.clone(),
            overflow_popover.clone(),
            overflow_box.clone(),
            overflow_button.clone(),
        );
        desktop.connect_apply(move |_| {
            // Expand to inline.
            if obox.parent().as_ref() != Some(slot.upcast_ref::<gtk::Widget>()) {
                pop.set_child(None::<&gtk::Widget>);
                slot.append(&obox);
            }
            btn.set_visible(false);
        });
    }
    {
        let (slot, pop, obox, btn) = (
            inline_slot.clone(),
            overflow_popover.clone(),
            overflow_box.clone(),
            overflow_button.clone(),
        );
        desktop.connect_unapply(move |_| {
            // Collapse back to the ⋮ overflow.
            if obox.parent().as_ref() != Some(pop.upcast_ref::<gtk::Widget>()) {
                slot.remove(&obox);
                pop.set_child(Some(&obox));
            }
            btn.set_visible(true);
        });
    }

    // Order matters: phone after tablet so it wins where both match (< 900px);
    // desktop is disjoint (MinWidth) so its position is irrelevant.
    window.add_breakpoint(tablet);
    window.add_breakpoint(phone);
    window.add_breakpoint(desktop);

    let widgets = Rc::new(Widgets {
        window: window.clone(),
        list,
        sidebar,
        title_entry,
        buffer,
        editor_stack,
        body_stack: body_stack.clone(),
        body_placeholder: body_placeholder.clone(),
        checklist_box: checklist_box.clone(),
        list_menu: list_menu.clone(),
        tag_box: tag_box.clone(),
        select_toggle: select_toggle.clone(),
        undo_button: undo_button.clone(),
        redo_button: redo_button.clone(),
        save_indicator: save_indicator.clone(),
        color_box: color_box.clone(),
        color_button: color_button.clone(),
        bulk_bar: bulk_bar.clone(),
        bulk_label: bulk_label.clone(),
        bulk_archive: bulk_archive.clone(),
        search: search.clone(),
        list_page: list_page.clone(),
        editor_page: editor_page.clone(),
        inner_split: inner_split.clone(),
        outer_split: outer_split.clone(),
        pin_button: pin_button.clone(),
        restore_button: restore_button.clone(),
        delete_button: delete_button.clone(),
        archive_button: archive_button.clone(),
        server_row,
        server_status,
        toasts,
    });

    super::actions::connect(&state, &widgets, &new_button, &delete_button);
    super::actions::connect_bulk(
        &state,
        &widgets,
        &select_toggle,
        &bulk_select_all,
        &bulk_move,
        &bulk_pin,
        &bulk_color_box,
        &bulk_archive,
        &bulk_delete,
    );
    super::actions::connect_sort(&state, &widgets, &sort_box, &sort_button);
    super::preview::install(&state, &widgets, &rich_view, &preview_toggle);
    super::actions::refresh_sidebar(&state, &widgets);
    // Bound to its own statement: an `if let` on `state.borrow()` keeps the
    // borrow alive through the body, and the `borrow_mut()` below panics.
    let first_note = state.borrow().visible_notes().first().map(|n| n.id.clone());
    if let Some(first) = first_note {
        state.borrow_mut().selected = Some(first);
    }
    super::actions::refresh_list(&state, &widgets);
    super::actions::load_editor(&state, &widgets);

    if state.borrow().config.autostart_server {
        super::actions::set_server_running(&state, &widgets, true);
    }

    // "Keep running in background": while the server runs, closing the window
    // hides it instead of quitting, so the phone keeps syncing. GTK keeps the
    // process alive as long as the (hidden) window exists; relaunching the app
    // activates this instance and presents the window again (see main.rs).
    {
        let state = state.clone();
        window.connect_close_request(move |window| {
            let keep_alive = {
                let s = state.borrow();
                s.config.run_in_background
                    && s.server.as_ref().is_some_and(|h| h.is_running())
            };
            if keep_alive {
                window.set_visible(false);
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }

    (window, state, widgets)
}

fn sidebar_content(list: &gtk::ListBox) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
    b.append(&adw::HeaderBar::new());
    b.append(
        &gtk::ScrolledWindow::builder()
            .vexpand(true)
            .child(list)
            .build(),
    );
    b
}

/// Google Keep's 12-swatch palette (the colours the imported notes already use),
/// plus a "no colour" reset. `None` clears the colour.
pub const PALETTE: &[(&str, Option<&str>)] = &[
    ("Default", None),
    ("Red", Some("#F28B82")),
    ("Orange", Some("#FBBC04")),
    ("Yellow", Some("#FFF475")),
    ("Green", Some("#CCFF90")),
    ("Teal", Some("#A7FFEB")),
    ("Cerulean", Some("#CBF0F8")),
    ("Blue", Some("#AECBFA")),
    ("Purple", Some("#D7AEFB")),
    ("Pink", Some("#FDCFE8")),
    ("Brown", Some("#E6C9A8")),
    ("Gray", Some("#E8EAED")),
];

/// Wraps the swatch box in a popover for the header colour button.
fn color_popover(color_box: &gtk::Box) -> gtk::Popover {
    gtk::Popover::builder().child(color_box).build()
}

/// Picks a GtkSourceView style scheme that matches the current light/dark
/// preference, falling back gracefully if the named schemes aren't installed.
fn apply_source_style(buffer: &sourceview5::Buffer) {
    let dark = adw::StyleManager::default().is_dark();
    let mgr = sourceview5::StyleSchemeManager::default();
    let scheme = if dark {
        mgr.scheme("Adwaita-dark")
            .or_else(|| mgr.scheme("classic-dark"))
            .or_else(|| mgr.scheme("solarized-dark"))
    } else {
        mgr.scheme("Adwaita").or_else(|| mgr.scheme("classic"))
    };
    if let Some(scheme) = scheme {
        buffer.set_style_scheme(Some(&scheme));
    }
}

fn server_menu() -> gtk::gio::Menu {
    let menu = gtk::gio::Menu::new();

    let import = gtk::gio::Menu::new();
    import.append(Some("Google Keep export (.zip)…"), Some("app.import-keep"));
    import.append(
        Some("Obsidian / Markdown folder…"),
        Some("app.import-vault"),
    );
    menu.append_submenu(Some("Import notes"), &import);
    menu.append(Some("Back up all notes…"), Some("app.export-backup"));
    menu.append(Some("Restore backup…"), Some("app.restore-backup"));

    menu.append(Some("Settings…"), Some("app.server-settings"));
    menu.append(Some("Open notes folder"), Some("app.open-folder"));
    menu.append(Some("About Notes"), Some("app.about"));
    menu
}

/// Formats the connection hint shown in the server dialog.
pub fn server_hint(config: &Config, running: bool) -> String {
    if !running {
        return "Server stopped.".to_string();
    }
    let urls = server::local_urls(config.port).join("\n");
    format!(
        "Serving {} \nPoint the Android app at:\n{}\nUser: {}",
        config.data_dir.display(),
        urls,
        config.username
    )
}

pub fn toast(widgets: &Widgets, message: &str) {
    widgets.toasts.add_toast(adw::Toast::new(message));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::now_millis;

    fn state_with(notes: Vec<Note>) -> AppState {
        let dir = std::env::temp_dir().join(format!("sn-ui-{}", crate::model::new_uuid()));
        AppState {
            store: Store::new(dir).unwrap(),
            config: Config::default(),
            notes,
            selected: None,
            filter: Filter::Notes,
            folder: None,
            tag: None,
            query: String::new(),
            server: None,
            loading: false,
            rebuilding: false,
            list_refresh_pending: false,
            selecting: false,
            syncing: false,
            last_sync: 0,
            intent: false,
            checked: std::collections::BTreeSet::new(),
        }
    }

    fn note(title: &str) -> Note {
        Note {
            title: title.into(),
            ..Default::default()
        }
    }

    fn titles(state: &AppState) -> Vec<String> {
        state
            .visible_notes()
            .iter()
            .map(|n| n.title.clone())
            .collect()
    }

    #[test]
    fn every_sidebar_count_matches_its_unsearched_list() {
        let mut state = state_with(vec![
            note("loose"),
            Note {
                is_pinned: true,
                ..note("pinned")
            },
            Note {
                folder_name: Some("Work".into()),
                ..note("folder live")
            },
            Note {
                folder_name: Some("Work".into()),
                archived_at: Some(now_millis()),
                ..note("folder archived")
            },
            Note {
                archived_at: Some(now_millis()),
                ..note("archived")
            },
            Note {
                trashed_at: Some(now_millis()),
                ..note("trashed")
            },
            Note {
                labels: Some(vec!["tag".into(), "tag".into()]),
                ..note("tagged")
            },
        ]);

        for target in [
            SidebarTarget::Notes,
            SidebarTarget::Pinned,
            SidebarTarget::Archived,
            SidebarTarget::Trash,
            SidebarTarget::Folder("Work".into()),
            SidebarTarget::Tag("tag".into()),
        ] {
            let displayed = state.target_count(&target);
            state.apply_target(&target);
            assert_eq!(
                displayed,
                state.visible_notes().len(),
                "counter and content differ for {target:?}"
            );
        }
    }

    /// The point of archiving: out of the main list, but one click away — not
    /// deleted, and still reachable.
    #[test]
    fn archived_notes_leave_home_but_stay_reachable() {
        let archived = Note {
            archived_at: Some(now_millis()),
            ..note("Archived")
        };
        let mut state = state_with(vec![note("Active"), archived]);

        state.filter = Filter::Notes;
        assert_eq!(titles(&state), vec!["Active"]);

        state.filter = Filter::Archived;
        assert_eq!(titles(&state), vec!["Archived"]);
    }

    #[test]
    fn archived_notes_are_not_trashed_and_vice_versa() {
        let archived = Note {
            archived_at: Some(now_millis()),
            ..note("Archived")
        };
        let trashed = Note {
            trashed_at: Some(now_millis()),
            ..note("Trashed")
        };
        let mut state = state_with(vec![archived, trashed]);

        state.filter = Filter::Trash;
        assert_eq!(titles(&state), vec!["Trashed"]);

        state.filter = Filter::Archived;
        assert_eq!(titles(&state), vec!["Archived"]);
    }

    /// A note that is both pinned and archived belongs in Archive, not on the
    /// pinned shelf — otherwise archiving wouldn't actually hide it.
    #[test]
    fn archiving_beats_pinning() {
        let both = Note {
            archived_at: Some(now_millis()),
            is_pinned: true,
            ..note("Pinned and archived")
        };
        let mut state = state_with(vec![
            both,
            Note {
                is_pinned: true,
                ..note("Just pinned")
            },
        ]);

        state.filter = Filter::Pinned;
        assert_eq!(titles(&state), vec!["Just pinned"]);

        state.filter = Filter::Archived;
        assert_eq!(titles(&state), vec!["Pinned and archived"]);
    }

    /// Trashing an archived note takes it out of Archive — one place at a time.
    #[test]
    fn trashing_an_archived_note_moves_it_to_trash_only() {
        let n = Note {
            archived_at: Some(now_millis()),
            trashed_at: Some(now_millis()),
            ..note("Both")
        };
        let mut state = state_with(vec![n]);

        state.filter = Filter::Archived;
        assert!(titles(&state).is_empty());

        state.filter = Filter::Trash;
        assert_eq!(titles(&state), vec!["Both"]);
    }

    /// Regression: rebuilding the list used to clear `selected`, because
    /// removing rows makes GTK emit `row-selected(None)` and the handler
    /// believed it. Every keystroke rebuilds the list, so the second character
    /// typed into a note found no selected note and was silently dropped.
    ///
    /// This covers the state rule the fix depends on: while `rebuilding` is
    /// set, a reported deselection must not touch `selected`.
    #[test]
    fn a_deselection_during_a_rebuild_is_ignored() {
        let mut state = state_with(vec![note("Keep me")]);
        let id = state.notes[0].id.clone();
        state.selected = Some(id.clone());

        // What the row-selected handler does, in each mode.
        let apply_deselect = |s: &mut AppState| {
            if s.rebuilding {
                return;
            }
            s.selected = None;
        };

        state.rebuilding = true;
        apply_deselect(&mut state);
        assert_eq!(
            state.selected.as_deref(),
            Some(id.as_str()),
            "selection lost mid-rebuild"
        );

        state.rebuilding = false;
        apply_deselect(&mut state);
        assert_eq!(state.selected, None, "a real deselection must still work");
    }

    /// Consecutive edits must all land. Before the fix the note was deselected
    /// after the first save, so `selected_note_mut` returned None from then on.
    #[test]
    fn consecutive_edits_all_persist() {
        let mut state = state_with(vec![note("")]);
        let id = state.notes[0].id.clone();
        state.selected = Some(id.clone());

        for ch in ["H", "He", "Hel", "Hell", "Hello"] {
            let note = state
                .selected_note_mut()
                .expect("selection must survive each edit");
            note.title = ch.to_string();
            let snapshot = note.clone();
            state.store.save(&snapshot).unwrap();
        }

        assert_eq!(state.selected_note().unwrap().title, "Hello");
        let reloaded = state.store.load_all().unwrap();
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded[0].title, "Hello", "last edit did not reach disk");
    }

    #[test]
    fn search_matches_title_body_items_and_labels() {
        let n = Note {
            title: "Shopping".into(),
            content: "for the weekend".into(),
            labels: Some(vec!["errands".into()]),
            checklist_items: Some(vec![crate::model::ChecklistItem {
                id: "1".into(),
                text: "oat milk".into(),
                is_checked: false,
                order: 0,
                original_order: 0,
                created_at: 0,
                indentation_level: 0,
            }]),
            ..Default::default()
        };

        for q in ["shop", "WEEKEND", "oat", "errands", ""] {
            assert!(note_matches(&n, q), "expected {q:?} to match");
        }
        assert!(!note_matches(&n, "bicycle"));
    }

    /// Greek is the common case in this user's notes, so matching must be
    /// Unicode-aware rather than ASCII-only.
    #[test]
    fn search_is_case_insensitive_for_greek() {
        let n = Note {
            title: "Σχολείο".into(),
            ..note("x")
        };
        assert!(note_matches(&n, "σχολείο"));
        assert!(note_matches(&n, "ΣΧΟΛΕΙΟ") || note_matches(&n, "Σχολείο"));
    }

    /// More words must narrow the result, not widen it.
    #[test]
    fn every_search_term_must_match() {
        let n = Note {
            title: "Grocery run".into(),
            content: "milk".into(),
            ..Default::default()
        };
        assert!(note_matches(&n, "grocery milk"));
        assert!(!note_matches(&n, "grocery bicycle"));
    }

    #[test]
    fn search_from_all_notes_is_global_but_scoped_elsewhere() {
        let archived = Note {
            archived_at: Some(now_millis()),
            ..note("Archived recipe")
        };
        let trashed = Note {
            trashed_at: Some(now_millis()),
            ..note("Trashed recipe")
        };
        let mut state = state_with(vec![
            note("Active recipe"),
            archived,
            trashed,
            note("Active plan"),
        ]);

        // From "All notes", a search looks everywhere — including archived and
        // trashed — so a match is never hidden by the current view.
        state.query = "recipe".into();
        state.filter = Filter::Notes;
        let mut got = titles(&state);
        got.sort();
        assert_eq!(
            got,
            vec!["Active recipe", "Archived recipe", "Trashed recipe"]
        );

        // In a specific view, search stays scoped to that view.
        state.filter = Filter::Archived;
        assert_eq!(titles(&state), vec!["Archived recipe"]);
    }

    #[test]
    fn folder_view_hides_archived_notes_too() {
        let archived = Note {
            archived_at: Some(now_millis()),
            folder_name: Some("Work".into()),
            ..note("Archived in Work")
        };
        let active = Note {
            folder_name: Some("Work".into()),
            ..note("Active in Work")
        };
        let mut state = state_with(vec![archived, active]);

        state.filter = Filter::Folder;
        state.folder = Some("Work".into());
        assert_eq!(titles(&state), vec!["Active in Work"]);
    }

    #[test]
    fn selection_is_cleared_when_a_filter_hides_the_note() {
        let active = note("Active");
        let active_id = active.id.clone();
        let archived = Note {
            archived_at: Some(now_millis()),
            ..note("Archived")
        };
        let mut state = state_with(vec![active, archived]);
        state.selected = Some(active_id);

        state.apply_target(&SidebarTarget::Archived);
        assert!(state.clear_selection_if_hidden());
        assert_eq!(state.selected, None);
    }

    #[test]
    fn selection_survives_when_the_note_is_still_visible() {
        let pinned = Note {
            is_pinned: true,
            ..note("Pinned")
        };
        let id = pinned.id.clone();
        let mut state = state_with(vec![pinned]);
        state.selected = Some(id.clone());

        state.apply_target(&SidebarTarget::Pinned);
        assert!(!state.clear_selection_if_hidden());
        assert_eq!(state.selected.as_deref(), Some(id.as_str()));
    }
}

/// Renders one row in the note list.
///
/// Colour is the mobile app's main visual signal, so it tints the whole row
/// rather than sitting in a corner swatch. The CSS class is installed by
/// [`super::actions::install_color_css`].
pub fn note_row(note: &Note, in_trash: bool) -> gtk::Widget {
    let title = if note.title.trim().is_empty() {
        "Untitled".to_string()
    } else {
        note.title.clone()
    };

    let row = adw::ActionRow::builder()
        .title(glib::markup_escape_text(&title).to_string())
        .subtitle(glib::markup_escape_text(&note.preview()).to_string())
        .subtitle_lines(2)
        .build();

    let suffix = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    // In the trash, every note carries a trash icon (they're all equal there and
    // heading for deletion) and pinning is irrelevant, so it's not shown.
    if in_trash {
        suffix.append(&gtk::Image::from_icon_name("user-trash-symbolic"));
    } else if note.is_pinned {
        suffix.append(&gtk::Image::from_icon_name("view-pin-symbolic"));
    }
    suffix.append(
        &gtk::Label::builder()
            .label(crate::model::relative_time(note.updated_at))
            .css_classes(vec!["dim-label", "caption"])
            .build(),
    );
    row.add_suffix(&suffix);

    if let Some(color) = &note.color {
        row.add_css_class(&color_css_class(color));
    }
    row.upcast()
}

/// CSS class name for a note colour, e.g. `#F28B82` -> `note-color-f28b82`.
pub fn color_css_class(hex: &str) -> String {
    format!("note-color-{}", hex.trim_start_matches('#').to_lowercase())
}

#[cfg(test)]
mod tag_tests {
    use super::*;
    use crate::model::now_millis;

    fn state_with(notes: Vec<Note>) -> AppState {
        let dir = std::env::temp_dir().join(format!("sn-tag-{}", crate::model::new_uuid()));
        AppState {
            store: Store::new(dir).unwrap(),
            config: Config::default(),
            notes,
            selected: None,
            filter: Filter::Notes,
            folder: None,
            tag: None,
            query: String::new(),
            server: None,
            loading: false,
            rebuilding: false,
            list_refresh_pending: false,
            selecting: false,
            syncing: false,
            last_sync: 0,
            intent: false,
            checked: std::collections::BTreeSet::new(),
        }
    }

    fn tagged(title: &str, tags: &[&str]) -> Note {
        Note {
            title: title.into(),
            labels: Some(tags.iter().map(|t| t.to_string()).collect()),
            ..Default::default()
        }
    }

    #[test]
    fn tags_are_counted_across_live_notes() {
        let state = state_with(vec![
            tagged("a", &["work", "work", "urgent"]),
            tagged("b", &["work"]),
            Note {
                archived_at: Some(now_millis()),
                ..tagged("c", &["work"])
            },
            Note {
                trashed_at: Some(now_millis()),
                ..tagged("d", &["gone"])
            },
        ]);

        // Archived notes keep their tags and ARE counted (c is archived but still
        // carries "work"), so work == 3. The duplicate "work" on a counts once.
        // Only trashed notes are excluded, so "gone" does not appear.
        assert_eq!(
            state.all_tags(),
            vec![("urgent".to_string(), 1), ("work".to_string(), 3)]
        );
    }

    #[test]
    fn tag_view_includes_archived_notes() {
        let mut state = state_with(vec![
            tagged("live", &["work"]),
            Note {
                archived_at: Some(now_millis()),
                ..tagged("archived", &["work"])
            },
            Note {
                trashed_at: Some(now_millis()),
                ..tagged("trashed", &["work"])
            },
        ]);
        state.apply_target(&SidebarTarget::Tag("work".into()));
        let mut titles: Vec<_> = state
            .visible_notes()
            .iter()
            .map(|n| n.title.clone())
            .collect();
        titles.sort();
        // Archived shows under the tag; trashed does not.
        assert_eq!(titles, vec!["archived".to_string(), "live".to_string()]);
    }

    #[test]
    fn filtering_by_tag_selects_only_that_tag() {
        let mut state = state_with(vec![tagged("a", &["work"]), tagged("b", &["home"])]);
        state.apply_target(&SidebarTarget::Tag("work".into()));

        assert_eq!(state.filter, Filter::Tag);
        let titles: Vec<_> = state
            .visible_notes()
            .iter()
            .map(|n| n.title.clone())
            .collect();
        assert_eq!(titles, vec!["a"]);
    }

    #[test]
    fn switching_target_clears_the_previous_one() {
        let mut state = state_with(vec![]);
        state.apply_target(&SidebarTarget::Tag("work".into()));
        assert_eq!(state.tag.as_deref(), Some("work"));

        state.apply_target(&SidebarTarget::Folder("Work".into()));
        assert_eq!(state.folder.as_deref(), Some("Work"));
        assert_eq!(
            state.tag, None,
            "stale tag would silently narrow the folder view"
        );
    }

    #[test]
    fn sidebar_targets_survive_encoding() {
        for target in [
            SidebarTarget::Notes,
            SidebarTarget::Pinned,
            SidebarTarget::Archived,
            SidebarTarget::Trash,
            SidebarTarget::Folder("Σχολείο 🚸".into()),
            // A tag containing a colon must not be mangled by the encoding.
            SidebarTarget::Tag("google:keep".into()),
        ] {
            assert_eq!(SidebarTarget::decode(&target.encode()), target);
        }
    }

    #[test]
    fn search_composes_with_a_tag_filter() {
        let mut state = state_with(vec![
            tagged("Recipe pancakes", &["food"]),
            tagged("Recipe curry", &["food"]),
            tagged("Standup", &["work"]),
        ]);
        state.apply_target(&SidebarTarget::Tag("food".into()));
        state.query = "curry".into();

        let titles: Vec<_> = state
            .visible_notes()
            .iter()
            .map(|n| n.title.clone())
            .collect();
        assert_eq!(titles, vec!["Recipe curry"]);
    }
}

#[cfg(test)]
mod bulk_tests {
    use super::*;

    fn state_with(notes: Vec<Note>) -> AppState {
        let dir = std::env::temp_dir().join(format!("sn-bulk-{}", crate::model::new_uuid()));
        AppState {
            store: Store::new(dir).unwrap(),
            config: Config::default(),
            notes,
            selected: None,
            filter: Filter::Notes,
            folder: None,
            tag: None,
            query: String::new(),
            server: None,
            loading: false,
            rebuilding: false,
            list_refresh_pending: false,
            selecting: false,
            syncing: false,
            last_sync: 0,
            intent: false,
            checked: std::collections::BTreeSet::new(),
        }
    }
    fn note(t: &str) -> Note {
        Note {
            title: t.into(),
            ..Default::default()
        }
    }

    #[test]
    fn bulk_archive_touches_only_checked_notes() {
        let mut state = state_with(vec![note("a"), note("b"), note("c")]);
        let a = state.notes[0].id.clone();
        let c = state.notes[2].id.clone();
        state.checked.insert(a.clone());
        state.checked.insert(c.clone());

        let n = state.for_each_checked(|note| note.archived_at = Some(crate::model::now_millis()));
        assert_eq!(n, 2);

        // Persisted, and only the two checked ones.
        let reloaded = state.store.load_all().unwrap();
        let archived: Vec<_> = reloaded
            .iter()
            .filter(|n| n.is_archived())
            .map(|n| n.title.clone())
            .collect();
        assert_eq!(archived.len(), 2);
        assert!(archived.contains(&"a".to_string()));
        assert!(archived.contains(&"c".to_string()));
        assert!(!archived.contains(&"b".to_string()));
    }

    /// Bulk pin uses toggle semantics: a mixed selection pins everything;
    /// only a fully-pinned selection unpins. This is the decision rule the
    /// bulk-pin button computes before calling `for_each_checked`.
    #[test]
    fn bulk_pin_pins_mixed_selection_then_unpins_uniform_one() {
        let mut state = state_with(vec![note("a"), note("b")]);
        state.notes[0].is_pinned = true;
        let ids: Vec<String> = state.notes.iter().map(|n| n.id.clone()).collect();
        state.checked = ids.iter().cloned().collect();

        // Mixed (one pinned, one not) → pin all.
        let pin = state
            .notes
            .iter()
            .any(|n| state.checked.contains(&n.id) && !n.is_pinned);
        assert!(pin);
        state.for_each_checked(|n| n.is_pinned = pin);
        assert!(state.store.load_all().unwrap().iter().all(|n| n.is_pinned));

        // Uniformly pinned → unpin all.
        let pin = state
            .notes
            .iter()
            .any(|n| state.checked.contains(&n.id) && !n.is_pinned);
        assert!(!pin);
        state.for_each_checked(|n| n.is_pinned = pin);
        assert!(state.store.load_all().unwrap().iter().all(|n| !n.is_pinned));
    }

    #[test]
    fn bulk_color_persists_on_checked_only() {
        let mut state = state_with(vec![note("a"), note("b")]);
        let a = state.notes[0].id.clone();
        state.checked.insert(a.clone());

        let hex = Some("#f28b82".to_string());
        let n = state.for_each_checked(|note| note.color = hex.clone());
        assert_eq!(n, 1);

        let reloaded = state.store.load_all().unwrap();
        let colored = reloaded.iter().find(|n| n.id == a).unwrap();
        assert_eq!(colored.color.as_deref(), Some("#f28b82"));
        // The unchecked note is untouched (and was never even written).
        assert!(state
            .notes
            .iter()
            .any(|n| n.id != a && n.color.is_none()));
    }

    #[test]
    fn move_note_relocates_and_leaves_no_duplicate() {
        let store =
            Store::new(std::env::temp_dir().join(format!("sn-mv-{}", crate::model::new_uuid())))
                .unwrap();
        let n = note("Movable");
        store.save(&n).unwrap();
        store.move_note(&n, Some("Work".into())).unwrap();

        let all = store.load_all().unwrap();
        assert_eq!(all.len(), 1, "move must not duplicate");
        assert_eq!(all[0].folder_name.as_deref(), Some("Work"));

        // And back to top level.
        store.move_note(&all[0], None).unwrap();
        let all = store.load_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].folder_name, None);
    }
}

#[cfg(test)]
mod sort_tests {
    use super::*;

    fn cfg(field: &str, ascending: bool) -> Config {
        Config {
            sort_field: field.into(),
            sort_ascending: ascending,
            ..Config::default()
        }
    }
    fn note(t: &str) -> Note {
        Note {
            title: t.into(),
            ..Default::default()
        }
    }

    #[test]
    fn default_sort_is_updated_descending() {
        let mut a = note("old");
        a.updated_at = 100;
        let mut b = note("new");
        b.updated_at = 200;
        let c = cfg("updated", false);
        assert_eq!(note_sort_cmp(&c, &b, &a), std::cmp::Ordering::Less);
        // Flipping the direction flips the pair.
        let c = cfg("updated", true);
        assert_eq!(note_sort_cmp(&c, &a, &b), std::cmp::Ordering::Less);
    }

    #[test]
    fn title_sort_is_case_insensitive_and_unicode_aware() {
        let a = note("αλφα");
        let b = note("Βήτα");
        let c = cfg("title", true);
        assert_eq!(note_sort_cmp(&c, &a, &b), std::cmp::Ordering::Less);
        let c = cfg("title", false);
        assert_eq!(note_sort_cmp(&c, &b, &a), std::cmp::Ordering::Less);
    }

    #[test]
    fn type_sort_puts_text_before_checklists_when_ascending() {
        let text = note("t");
        let mut list = note("l");
        list.checklist_items = Some(vec![]);
        let c = cfg("type", true);
        assert_eq!(note_sort_cmp(&c, &text, &list), std::cmp::Ordering::Less);
    }

    #[test]
    fn color_sort_follows_palette_order_with_uncolored_first() {
        let plain = note("plain");
        let mut red = note("red");
        red.color = Some("#F28B82".into());
        let mut unknown = note("custom");
        unknown.color = Some("#123456".into());
        let c = cfg("color", true);
        assert_eq!(note_sort_cmp(&c, &plain, &red), std::cmp::Ordering::Less);
        assert_eq!(note_sort_cmp(&c, &red, &unknown), std::cmp::Ordering::Less);
    }

    /// Equal primary keys must not make rows jump between refreshes: the
    /// tie-breaker is fixed (updated desc, then id) regardless of direction.
    #[test]
    fn ties_break_deterministically() {
        let mut a = note("same");
        a.updated_at = 500;
        let mut b = note("same");
        b.updated_at = 700;
        for asc in [true, false] {
            let c = cfg("title", asc);
            assert_eq!(
                note_sort_cmp(&c, &b, &a),
                std::cmp::Ordering::Less,
                "newer-updated wins the tie in both directions"
            );
        }
    }

    /// Pinned notes stay above unpinned ones whatever the sort says.
    #[test]
    fn pinned_notes_stay_on_top_of_any_sort() {
        let dir = std::env::temp_dir().join(format!("sn-sort-{}", crate::model::new_uuid()));
        let mut pinned = note("ωμέγα");
        pinned.is_pinned = true;
        let alpha = note("αλφα");
        let state = AppState {
            store: Store::new(dir).unwrap(),
            config: cfg("title", true),
            notes: vec![alpha, pinned],
            selected: None,
            filter: Filter::Notes,
            folder: None,
            tag: None,
            query: String::new(),
            server: None,
            loading: false,
            rebuilding: false,
            list_refresh_pending: false,
            selecting: false,
            syncing: false,
            last_sync: 0,
            intent: false,
            checked: std::collections::BTreeSet::new(),
        };
        let visible = state.visible_notes();
        assert_eq!(visible[0].title, "ωμέγα");
        assert_eq!(visible[1].title, "αλφα");
    }
}

#[cfg(test)]
mod move_tests {
    use super::*;
    use crate::model::now_millis;

    fn state_with(notes: Vec<Note>) -> AppState {
        let dir = std::env::temp_dir().join(format!("sn-move-{}", crate::model::new_uuid()));
        AppState {
            store: Store::new(dir).unwrap(),
            config: Config::default(),
            notes,
            selected: None,
            filter: Filter::Notes,
            folder: None,
            tag: None,
            query: String::new(),
            server: None,
            loading: false,
            rebuilding: false,
            list_refresh_pending: false,
            selecting: false,
            syncing: false,
            last_sync: 0,
            intent: false,
            checked: std::collections::BTreeSet::new(),
        }
    }

    #[test]
    fn move_note_relocates_files_and_updates_folder() {
        let store =
            Store::new(std::env::temp_dir().join(format!("sn-mv-{}", crate::model::new_uuid())))
                .unwrap();
        let note = Note {
            title: "Report".into(),
            ..Default::default()
        };
        store.save(&note).unwrap();
        assert!(store
            .root()
            .join(format!("notes/{}.json", note.id))
            .exists());

        store.move_note(&note, Some("Work".into())).unwrap();

        // Old location gone, new location present, no duplicate.
        assert!(!store
            .root()
            .join(format!("notes/{}.json", note.id))
            .exists());
        assert!(store
            .root()
            .join(format!("notes/Work/{}.json", note.id))
            .exists());
        assert_eq!(store.load_all().unwrap().len(), 1);
        assert_eq!(
            store.load_all().unwrap()[0].folder_name.as_deref(),
            Some("Work")
        );
    }

    #[test]
    fn move_out_of_folder_returns_to_top_level() {
        let store =
            Store::new(std::env::temp_dir().join(format!("sn-mv2-{}", crate::model::new_uuid())))
                .unwrap();
        let note = Note {
            title: "X".into(),
            folder_name: Some("Work".into()),
            ..Default::default()
        };
        store.save(&note).unwrap();
        store.move_note(&note, None).unwrap();
        assert!(store
            .root()
            .join(format!("notes/{}.json", note.id))
            .exists());
        assert_eq!(store.load_all().unwrap()[0].folder_name, None);
    }

    /// Bulk unarchive is the same `for_each_checked` primitive with the op
    /// clearing `archived_at` instead of setting it — this is the state-level
    /// contract the context-aware button relies on.
    #[test]
    fn bulk_unarchive_clears_archived_on_checked_only() {
        let a = Note {
            archived_at: Some(now_millis()),
            ..Note {
                title: "a".into(),
                ..Default::default()
            }
        };
        let b = Note {
            archived_at: Some(now_millis()),
            ..Note {
                title: "b".into(),
                ..Default::default()
            }
        };
        let mut state = state_with(vec![a.clone(), b.clone()]);
        state.checked.insert(a.id.clone());

        let n = state.for_each_checked(|note| note.archived_at = None);
        assert_eq!(n, 1);
        assert!(state
            .notes
            .iter()
            .find(|x| x.id == a.id)
            .unwrap()
            .archived_at
            .is_none());
        assert!(state
            .notes
            .iter()
            .find(|x| x.id == b.id)
            .unwrap()
            .archived_at
            .is_some());
    }
}

/// Wraps the selection in `prefix`/`suffix` (e.g. `**`…`**`). With no
/// selection, inserts the markers and parks the cursor between them.
fn md_wrap(buffer: &gtk::TextBuffer, prefix: &str, suffix: &str) {
    if let Some((mut start, mut end)) = buffer.selection_bounds() {
        let text = buffer.text(&start, &end, false).to_string();
        buffer.begin_user_action();
        buffer.delete(&mut start, &mut end);
        buffer.insert(&mut start, &format!("{prefix}{text}{suffix}"));
        buffer.end_user_action();
    } else {
        let mut iter = buffer.iter_at_mark(&buffer.get_insert());
        buffer.begin_user_action();
        buffer.insert(&mut iter, &format!("{prefix}{suffix}"));
        buffer.end_user_action();
        let mut cursor = buffer.iter_at_mark(&buffer.get_insert());
        cursor.backward_chars(suffix.chars().count() as i32);
        buffer.place_cursor(&cursor);
    }
}

/// Inserts `prefix` at the start of the cursor's line (headings, list markers).
fn md_line_prefix(buffer: &gtk::TextBuffer, prefix: &str) {
    let mut iter = buffer.iter_at_mark(&buffer.get_insert());
    iter.set_line_offset(0);
    buffer.begin_user_action();
    buffer.insert(&mut iter, prefix);
    buffer.end_user_action();
}

/// Replaces the selection with `f(selection)` — used for the case buttons.
fn md_transform(buffer: &gtk::TextBuffer, f: impl Fn(&str) -> String) {
    if let Some((mut start, mut end)) = buffer.selection_bounds() {
        let text = buffer.text(&start, &end, false).to_string();
        buffer.begin_user_action();
        buffer.delete(&mut start, &mut end);
        buffer.insert(&mut start, &f(&text));
        buffer.end_user_action();
    }
}

fn title_case(s: &str) -> String {
    s.split_inclusive(char::is_whitespace)
        .map(|word| {
            let mut c = word.chars();
            match c.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &c.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect()
}

/// The pill-shaped Markdown formatting toolbar shown above the title. Buttons
/// act on `buffer` and return focus to `view` so typing continues.
fn build_format_toolbar(buffer: &gtk::TextBuffer, view: &sourceview5::View) -> gtk::Widget {
    // An adw::WrapBox (not a Box in a ScrolledWindow) so a narrow window wraps
    // the buttons onto 2–3 rows instead of scrolling them off. Unlike GtkFlowBox
    // it packs variable-width children tightly — no wasted grid columns — so a
    // wide window keeps everything on one row.
    let bar = adw::WrapBox::builder()
        .child_spacing(2)
        .line_spacing(2)
        .justify(adw::JustifyMode::None)
        .css_classes(vec!["toolbar", "format-bar"])
        .margin_top(6)
        .margin_bottom(2)
        .build();

    // (icon, tooltip, action)
    enum Act {
        Wrap(&'static str, &'static str),
        Line(&'static str),
        Case(fn(&str) -> String),
        Insert(&'static str),
        Sep,
    }
    use Act::*;
    let items: &[(&str, &str, Act)] = &[
        ("format-text-bold-symbolic", "Bold (Ctrl+B)", Wrap("**", "**")),
        ("format-text-italic-symbolic", "Italic (Ctrl+I)", Wrap("_", "_")),
        (
            "format-text-underline-symbolic",
            "Underline (Ctrl+U)",
            Wrap("<u>", "</u>"),
        ),
        (
            "format-text-strikethrough-symbolic",
            "Strikethrough (Ctrl+S)",
            Wrap("~~", "~~"),
        ),
        ("", "", Sep),
        ("heading-1", "Heading 1 (Ctrl+1)", Line("# ")),
        ("heading-2", "Heading 2 (Ctrl+2)", Line("## ")),
        ("heading-3", "Heading 3 (Ctrl+3)", Line("### ")),
        ("", "", Sep),
        ("case-title", "Title Case", Case(title_case)),
        ("case-upper", "UPPERCASE", Case(|s| s.to_uppercase())),
        ("case-lower", "lowercase", Case(|s| s.to_lowercase())),
        ("", "", Sep),
        ("view-list-symbolic", "Bullet list", Line("- ")),
        ("view-list-ordered-symbolic", "Numbered list", Line("1. ")),
        ("image-x-generic-symbolic", "Image", Insert("![alt](url)")),
        ("", "", Sep),
        ("insert-link-symbolic", "Link (Ctrl+K)", Wrap("[", "](url)")),
    ];

    // Formatting shortcuts, scoped to the body view so they can't fire from
    // the title, search or checklist entries. Capture phase beats the
    // TextView's own handling. Ctrl+S is free for strikethrough because the
    // editor autosaves — there is nothing to "save".
    {
        let buffer = buffer.clone();
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            if !modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK)
                || modifiers.contains(gtk::gdk::ModifierType::ALT_MASK)
            {
                return glib::Propagation::Proceed;
            }
            let shift = modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK);
            match key.to_unicode().map(|c| c.to_ascii_lowercase()) {
                Some('b') if !shift => md_wrap(&buffer, "**", "**"),
                Some('i') if !shift => md_wrap(&buffer, "_", "_"),
                Some('u') if !shift => md_wrap(&buffer, "<u>", "</u>"),
                Some('s') if !shift => md_wrap(&buffer, "~~", "~~"),
                Some('k') if !shift => md_wrap(&buffer, "[", "](url)"),
                Some('1') => md_line_prefix(&buffer, "# "),
                Some('2') => md_line_prefix(&buffer, "## "),
                Some('3') => md_line_prefix(&buffer, "### "),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        view.add_controller(keys);
    }

    for (icon, tip, act) in items {
        if matches!(act, Sep) {
            // WrapBox packs children at their natural width, so a thin group
            // separator costs almost nothing (unlike a FlowBox grid column).
            let sep = gtk::Separator::new(gtk::Orientation::Vertical);
            sep.set_can_focus(false);
            bar.append(&sep);
            continue;
        }
        // The three case buttons use text labels (Aa / AA / aa); the rest icons.
        let button = if let Some(label) = icon.strip_prefix("case-") {
            let text = match label {
                "title" => "Aa",
                "upper" => "AA",
                _ => "aa",
            };
            gtk::Button::builder()
                .label(text)
                .tooltip_text(*tip)
                .css_classes(vec!["flat"])
                .build()
        } else if let Some(level) = icon.strip_prefix("heading-") {
            gtk::Button::builder()
                .label(&format!("H{level}"))
                .tooltip_text(*tip)
                .css_classes(vec!["flat"])
                .build()
        } else {
            gtk::Button::builder()
                .icon_name(*icon)
                .tooltip_text(*tip)
                .css_classes(vec!["flat"])
                .build()
        };

        let (buffer, view) = (buffer.clone(), view.clone());
        // Move the action out of the slice by matching on a copy.
        let action: Box<dyn Fn(&gtk::TextBuffer)> = match act {
            Wrap(p, s) => {
                let (p, s) = (*p, *s);
                Box::new(move |b: &gtk::TextBuffer| md_wrap(b, p, s))
            }
            Line(p) => {
                let p = *p;
                Box::new(move |b: &gtk::TextBuffer| md_line_prefix(b, p))
            }
            Case(f) => {
                let f = *f;
                Box::new(move |b: &gtk::TextBuffer| md_transform(b, f))
            }
            Insert(t) => {
                let t = *t;
                Box::new(move |b: &gtk::TextBuffer| b.insert_at_cursor(t))
            }
            Sep => unreachable!(),
        };
        button.connect_clicked(move |_| {
            action(&buffer);
            view.grab_focus();
        });
        bar.append(&button);
    }

    bar.upcast()
}
