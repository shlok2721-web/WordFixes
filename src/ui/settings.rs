//! "Server & Sync" preferences dialog and the app-level actions behind the
//! main menu.

use super::window::{server_hint, Shared, Widgets};
use crate::config::Config;
use adw::prelude::*;
use gtk::gio;
use std::rc::Rc;

pub fn install_actions(app: &adw::Application, state: &Shared, widgets: &Rc<Widgets>) {
    let open_settings = gio::SimpleAction::new("server-settings", None);
    {
        let (state, widgets) = (state.clone(), widgets.clone());
        open_settings.connect_activate(move |_, _| show_dialog(&state, &widgets));
    }
    app.add_action(&open_settings);

    let open_folder = gio::SimpleAction::new("open-folder", None);
    {
        let state = state.clone();
        open_folder.connect_activate(move |_, _| {
            let path = state.borrow().config.data_dir.clone();
            crate::open_path(&path);
        });
    }
    app.add_action(&open_folder);

    // About: names the exact build, so "am I running the new version?" is
    // answerable without guesswork.
    {
        let about = gio::SimpleAction::new("about", None);
        let widgets = widgets.clone();
        about.connect_activate(move |_, _| {
            let dialog = adw::AboutDialog::builder()
                .application_name("Notes")
                .application_icon("dev.local.Notes")
                .version(format!(
                    "{} — built {}",
                    env!("NOTES_BUILD_LABEL"),
                    env!("NOTES_BUILD_STAMP")
                ))
                .comments(
                    "A GTK4 notes app that is also the sync server for the \
                     Simple Notes Sync Android app — same data format, no Docker.",
                )
                .license_type(gtk::License::Agpl30)
                .website("https://github.com/kalotrapezis/Notes")
                .issue_url("https://github.com/kalotrapezis/Notes/issues")
                .build();
            dialog.present(Some(&widgets.window));
        });
        app.add_action(&about);
    }

    {
        let empty = gio::SimpleAction::new("empty-trash", None);
        let (state, widgets) = (state.clone(), widgets.clone());
        empty.connect_activate(move |_, _| super::actions::confirm_empty_trash(&state, &widgets));
        app.add_action(&empty);
    }

    {
        let export = gio::SimpleAction::new("export-backup", None);
        let (state, widgets) = (state.clone(), widgets.clone());
        export.connect_activate(move |_, _| pick_and_export_backup(&state, &widgets));
        app.add_action(&export);
    }
    {
        let restore = gio::SimpleAction::new("restore-backup", None);
        let (state, widgets) = (state.clone(), widgets.clone());
        restore.connect_activate(move |_, _| pick_and_restore_backup(&state, &widgets));
        app.add_action(&restore);
    }

    // --- import actions ---
    {
        let keep = gio::SimpleAction::new("import-keep", None);
        let (state, widgets) = (state.clone(), widgets.clone());
        keep.connect_activate(move |_, _| pick_and_import_keep(&state, &widgets));
        app.add_action(&keep);
    }
    {
        let vault = gio::SimpleAction::new("import-vault", None);
        let (state, widgets) = (state.clone(), widgets.clone());
        vault.connect_activate(move |_, _| pick_and_import_vault(&state, &widgets));
        app.add_action(&vault);
    }

    for (name, kind) in [
        ("new-note", Some(crate::model::NoteType::TEXT)),
        ("new-checklist", Some(crate::model::NoteType::CHECKLIST)),
        ("new-folder", None),
    ] {
        let action = gio::SimpleAction::new(name, None);
        let (state, widgets) = (state.clone(), widgets.clone());
        action.connect_activate(move |_, _| match kind {
            Some(kind) => super::actions::create_note(&state, &widgets, kind),
            None => super::actions::prompt_new_folder(&state, &widgets),
        });
        app.add_action(&action);
    }
}

/// "Last synced 3 mins ago", or an invitation when it never has been.
fn last_sync_subtitle(state: &Shared) -> String {
    match state.borrow().last_sync {
        0 => "Not synced yet".to_string(),
        at => format!("Last synced {}", crate::model::relative_time(at)),
    }
}

fn show_dialog(state: &Shared, widgets: &Rc<Widgets>) {
    let config = state.borrow().config.clone();

    let dialog = adw::PreferencesDialog::builder().title("Settings").build();

    let page = adw::PreferencesPage::builder()
        .title("Server")
        .icon_name("emblem-system-symbolic")
        .build();

    // --- server group ---
    let server_group = adw::PreferencesGroup::builder()
        .title("WebDAV server")
        .description(
            "Replaces the Docker container from the upstream project — \
             the Android app syncs directly to this machine.",
        )
        .build();

    let bind_row = adw::ComboRow::builder()
        .title("Listen on")
        .model(&gtk::StringList::new(&[
            "This computer only (127.0.0.1)",
            "All interfaces — reachable on your LAN (0.0.0.0)",
        ]))
        .selected(u32::from(config.bind == "0.0.0.0"))
        .build();

    let port_row = adw::SpinRow::with_range(1024.0, 65535.0, 1.0);
    port_row.set_title("Port");
    port_row.set_value(f64::from(config.port));

    let user_row = adw::EntryRow::builder()
        .title("Username")
        .text(&config.username)
        .build();
    let pass_row = adw::PasswordEntryRow::builder().title("Password").build();
    pass_row.set_text(&config.password);

    let autostart_row = adw::SwitchRow::builder()
        .title("Start server on launch")
        .active(config.autostart_server)
        .build();

    // No tray icon: the window simply hides on close while the server runs,
    // and relaunching Notes (or activating it again) raises it.
    let background_row = adw::SwitchRow::builder()
        .title("Keep running in background")
        .subtitle("Closing the window keeps the sync server alive; open Notes again to return")
        .active(config.run_in_background)
        .build();
    {
        let state = state.clone();
        background_row.connect_active_notify(move |row| {
            let mut s = state.borrow_mut();
            s.config.run_in_background = row.is_active();
            let _ = s.config.save();
        });
    }

    // Deliberately forget the generated credentials. The Automatic tab reuses
    // them forever once set, so this is the only way to make it mint a fresh
    // name / password / port next time — the "delete it" half of "only Advanced
    // changes or deletes the server config".
    let reset_row = adw::ActionRow::builder()
        .title("Reset server credentials")
        .subtitle("Stops the server and clears the saved name, password and port")
        .build();
    let reset_btn = gtk::Button::builder()
        .label("Reset")
        .valign(gtk::Align::Center)
        .css_classes(vec!["destructive-action"])
        .build();
    reset_row.add_suffix(&reset_btn);

    server_group.add(&widgets.server_row);
    server_group.add(&bind_row);
    server_group.add(&port_row);
    server_group.add(&user_row);
    server_group.add(&pass_row);
    server_group.add(&autostart_row);
    server_group.add(&background_row);
    server_group.add(&reset_row);

    {
        let (state, widgets) = (state.clone(), widgets.clone());
        let (bind_row, port_row, user_row, pass_row) = (
            bind_row.clone(),
            port_row.clone(),
            user_row.clone(),
            pass_row.clone(),
        );
        reset_btn.connect_clicked(move |_| {
            super::actions::set_server_running(&state, &widgets, false);
            {
                let mut s = state.borrow_mut();
                s.config.username = "noteuser".to_string();
                s.config.password = String::new();
                s.config.bind = "127.0.0.1".to_string();
                s.config.port = 8080;
                let _ = s.config.save();
            }
            // Reflect the cleared values in the open dialog.
            widgets.server_row.set_active(false);
            user_row.set_text("noteuser");
            pass_row.set_text("");
            bind_row.set_selected(0);
            port_row.set_value(8080.0);
            super::window::toast(&widgets, "Server credentials reset");
        });
    }

    // --- per-network server profiles ---
    // Pill buttons: one per saved profile ("Home", "Work", "On road"…), plus a
    // "+" pill that saves the current server setup under the current network.
    // The network watcher re-applies a matching profile when the network
    // changes (see actions::start_network_watch).
    let profiles_group = adw::PreferencesGroup::builder()
        .title("Network profiles")
        .description(
            "Save this server setup for the network you're on. When the active \
             network changes, its saved profile is applied automatically. \
             Right-click a profile to delete it.",
        )
        .build();
    let pills = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .margin_top(6)
        .build();
    profiles_group.add(&pills);

    {
        type RebuildFn = std::cell::RefCell<Box<dyn Fn()>>;
        let rebuild: Rc<RebuildFn> = Rc::new(std::cell::RefCell::new(Box::new(|| {})));
        let rebuild_handle = rebuild.clone();
        let (state, widgets) = (state.clone(), widgets.clone());
        let pills = pills.clone();
        let (bind_row, port_row, user_row, pass_row) = (
            bind_row.clone(),
            port_row.clone(),
            user_row.clone(),
            pass_row.clone(),
        );
        *rebuild.borrow_mut() = Box::new(move || {
            while let Some(child) = pills.first_child() {
                pills.remove(&child);
            }
            let profiles = state.borrow().config.server_profiles.clone();
            for profile in &profiles {
                let pill = gtk::Button::builder()
                    .label(&profile.name)
                    .css_classes(vec!["pill"])
                    .tooltip_text(format!("Network: {}", profile.network))
                    .build();
                // Left-click applies the profile and reflects it in the rows.
                {
                    let (state, widgets, profile) =
                        (state.clone(), widgets.clone(), profile.clone());
                    let (bind_row, port_row, user_row, pass_row) = (
                        bind_row.clone(),
                        port_row.clone(),
                        user_row.clone(),
                        pass_row.clone(),
                    );
                    pill.connect_clicked(move |_| {
                        super::actions::apply_server_profile(&state, &widgets, &profile);
                        bind_row.set_selected(u32::from(profile.bind == "0.0.0.0"));
                        port_row.set_value(f64::from(profile.port));
                        user_row.set_text(&profile.username);
                        pass_row.set_text(&profile.password);
                    });
                }
                // Right-click deletes (with confirmation).
                {
                    let (state, widgets) = (state.clone(), widgets.clone());
                    let name = profile.name.clone();
                    let rebuild = rebuild_handle.clone();
                    let gesture = gtk::GestureClick::builder()
                        .button(gtk::gdk::BUTTON_SECONDARY)
                        .build();
                    gesture.connect_released(move |_, _, _, _| {
                        let confirm = adw::AlertDialog::builder()
                            .heading(format!("Delete profile “{name}”?"))
                            .body("Only the saved profile is removed; the current server settings stay as they are.")
                            .build();
                        confirm.add_response("cancel", "Cancel");
                        confirm.add_response("delete", "Delete");
                        confirm
                            .set_response_appearance("delete", adw::ResponseAppearance::Destructive);
                        confirm.set_close_response("cancel");
                        let (state, name, rebuild) =
                            (state.clone(), name.clone(), rebuild.clone());
                        confirm.connect_response(None, move |_, r| {
                            if r != "delete" {
                                return;
                            }
                            {
                                let mut s = state.borrow_mut();
                                s.config.server_profiles.retain(|p| p.name != name);
                                let _ = s.config.save();
                            }
                            (rebuild.borrow())();
                        });
                        confirm.present(Some(&widgets.window));
                    });
                    pill.add_controller(gesture);
                }
                pills.append(&pill);
            }

            // The "+" pill: name the current setup and remember the network.
            let add = gtk::Button::builder()
                .icon_name("list-add-symbolic")
                .css_classes(vec!["pill"])
                .tooltip_text("Save the current server settings as a profile for this network")
                .build();
            {
                let (state, widgets) = (state.clone(), widgets.clone());
                let rebuild = rebuild_handle.clone();
                add.connect_clicked(move |_| {
                    let network = crate::naming::current_network().unwrap_or_default();
                    let dialog = adw::AlertDialog::builder()
                        .heading("Save server profile")
                        .body(if network.is_empty() {
                            "No network detected — the profile can still be applied manually."
                                .to_string()
                        } else {
                            format!("Current network: {network}")
                        })
                        .build();
                    let name_entry = adw::EntryRow::builder().title("Profile name").build();
                    // Suggest something useful: first unsaved of the classic trio.
                    let taken: Vec<String> = state
                        .borrow()
                        .config
                        .server_profiles
                        .iter()
                        .map(|p| p.name.clone())
                        .collect();
                    let suggestion = ["Home", "Work", "On road"]
                        .iter()
                        .find(|n| !taken.contains(&n.to_string()))
                        .unwrap_or(&"New place");
                    name_entry.set_text(suggestion);
                    let group = adw::PreferencesGroup::new();
                    group.add(&name_entry);
                    dialog.set_extra_child(Some(&group));
                    dialog.add_response("cancel", "Cancel");
                    dialog.add_response("save", "Save");
                    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
                    dialog.set_default_response(Some("save"));
                    dialog.set_close_response("cancel");
                    let (state, widgets, rebuild, network) = (
                        state.clone(),
                        widgets.clone(),
                        rebuild.clone(),
                        network.clone(),
                    );
                    let entry = name_entry.clone();
                    dialog.connect_response(None, move |_, r| {
                        if r != "save" {
                            return;
                        }
                        let name = entry.text().trim().to_string();
                        if name.is_empty() {
                            return;
                        }
                        {
                            let mut s = state.borrow_mut();
                            let profile = s.config.profile_from_current(&name, &network);
                            // Same name = overwrite; same network moves to the
                            // new profile (one profile per network).
                            s.config
                                .server_profiles
                                .retain(|p| p.name != name && (network.is_empty() || p.network != network));
                            s.config.server_profiles.push(profile);
                            let _ = s.config.save();
                        }
                        (rebuild.borrow())();
                    });
                    dialog.present(Some(&widgets.window));
                });
            }
            pills.append(&add);
        });
        (rebuild.borrow())();
    }

    // Commit the credential fields to config the moment they change, not only
    // on dialog close. The server switch lives inside this dialog, so if the
    // password were only saved on close, flipping the switch here would read an
    // empty password and refuse to start. (This was the "can't toggle server
    // on" bug.)
    {
        let state = state.clone();
        let (br, pr, ur, pw) = (
            bind_row.clone(),
            port_row.clone(),
            user_row.clone(),
            pass_row.clone(),
        );
        let commit = move || {
            let mut s = state.borrow_mut();
            s.config.bind = if br.selected() == 1 {
                "0.0.0.0"
            } else {
                "127.0.0.1"
            }
            .to_string();
            s.config.port = pr.value() as u16;
            s.config.username = ur.text().to_string();
            s.config.password = pw.text().to_string();
            if let Err(e) = s.config.save() {
                eprintln!("failed to save server settings: {e}");
            }
        };
        let c1 = commit.clone();
        pass_row.connect_changed(move |_| c1());
        let c2 = commit.clone();
        user_row.connect_changed(move |_| c2());
        let c3 = commit.clone();
        bind_row.connect_selected_notify(move |_| c3());
        port_row.connect_changed(move |_| commit());
    }

    let status_group = adw::PreferencesGroup::new();
    widgets.server_status.set_text(&server_hint(
        &config,
        state
            .borrow()
            .server
            .as_ref()
            .is_some_and(|h| h.is_running()),
    ));
    status_group.add(&widgets.server_status);

    // --- storage group ---
    let storage_group = adw::PreferencesGroup::builder().title("Storage").build();
    let dir_row = adw::ActionRow::builder()
        .title("Notes folder")
        .subtitle(config.data_dir.display().to_string())
        .build();
    let default_view_row = adw::ComboRow::builder()
        .title("Open on")
        .subtitle("Section shown when the app starts")
        .model(&gtk::StringList::new(&["Home", "Pinned", "Archived"]))
        .selected(config.default_view_index() as u32)
        .build();

    let mirror_row = adw::SwitchRow::builder()
        .title("Markdown mirror")
        .subtitle("Also write /notes-md/ for external editors")
        .active(config.markdown_mirror)
        .build();

    let retention_labels: Vec<&str> = crate::config::Config::TRASH_RETENTION_OPTIONS
        .iter()
        .map(|(l, _)| *l)
        .collect();
    let retention_row = adw::ComboRow::builder()
        .title("Empty trash")
        .subtitle("Permanently delete trashed notes after this long")
        .model(&gtk::StringList::new(&retention_labels))
        .selected(config.trash_retention_index())
        .build();

    storage_group.add(&default_view_row);
    storage_group.add(&dir_row);
    storage_group.add(&mirror_row);
    storage_group.add(&retention_row);

    // --- library group (one-time reorganisations) ---
    let library_group = adw::PreferencesGroup::builder()
        .title("Library")
        .description("One-time reorganisations of the whole collection.")
        .build();
    let convert_row = adw::ActionRow::builder()
        .title("Convert tags to folders")
        .subtitle(
            "Creates a folder for every tag and moves each tagged note into its \
             first tag's folder, so the structure shows on the phone. Tags are kept.",
        )
        .build();
    let convert_btn = gtk::Button::builder()
        .label("Convert…")
        .valign(gtk::Align::Center)
        .build();
    convert_row.add_suffix(&convert_btn);
    library_group.add(&convert_row);
    {
        let (state, widgets) = (state.clone(), widgets.clone());
        convert_btn.connect_clicked(move |_| run_tag_conversion(&state, &widgets));
    }

    let storage_page = adw::PreferencesPage::builder()
        .title("Storage")
        .icon_name("folder-symbolic")
        .build();

    // --- client mode ---
    // The mirror image of the group above: instead of hosting a vault for the
    // phone, this machine syncs to a vault hosted somewhere else. Same protocol,
    // same last-write-wins rules — see `sync::plan`.
    let remote_group = adw::PreferencesGroup::builder()
        .title("Sync to another server")
        .description(
            "Point this machine at a WebDAV vault hosted elsewhere — another PC \
             running Notes, a NAS, or a hosted account. Leave empty to only host \
             your own.",
        )
        .build();

    let remote_url_row = adw::EntryRow::builder()
        .title("Server URL")
        .text(&config.remote_url)
        .build();
    let remote_user_row = adw::EntryRow::builder()
        .title("Username")
        .text(&config.remote_username)
        .build();
    let remote_pass_row = adw::PasswordEntryRow::builder().title("Password").build();
    remote_pass_row.set_text(&config.remote_password);

    let interval_row = adw::ComboRow::builder()
        .title("Sync automatically")
        .model(&gtk::StringList::new(
            &Config::SYNC_INTERVAL_OPTIONS
                .iter()
                .map(|(label, _)| *label)
                .collect::<Vec<_>>(),
        ))
        .selected(config.sync_interval_index())
        .build();

    let sync_row = adw::ActionRow::builder()
        .title("Sync now")
        .subtitle(&last_sync_subtitle(state))
        .build();
    let sync_btn = gtk::Button::builder()
        .label("Sync")
        .valign(gtk::Align::Center)
        .build();
    sync_row.add_suffix(&sync_btn);

    // Saving on every keystroke would be noise; save when the row loses focus,
    // the same moment the value stops changing.
    {
        let outer_state = state.clone();
        let (state, url, user, pass) = (
            state.clone(),
            remote_url_row.clone(),
            remote_user_row.clone(),
            remote_pass_row.clone(),
        );
        let save = move || {
            let mut s = state.borrow_mut();
            s.config.remote_url = url.text().trim().to_string();
            s.config.remote_username = user.text().to_string();
            s.config.remote_password = pass.text().to_string();
            let _ = s.config.save();
        };
        let save = Rc::new(save);
        for row in [
            remote_url_row.clone().upcast::<gtk::Widget>(),
            remote_user_row.clone().upcast(),
            remote_pass_row.clone().upcast(),
        ] {
            let save = save.clone();
            let controller = gtk::EventControllerFocus::new();
            controller.connect_leave(move |_| save());
            row.add_controller(controller);
        }
        let save_on_sync = save.clone();
        let (state2, widgets2, sync_row2) = (outer_state.clone(), widgets.clone(), sync_row.clone());
        sync_btn.connect_clicked(move |_| {
            save_on_sync(); // whatever is typed is what we should sync against
            super::actions::sync_now(&state2, &widgets2, false);
            sync_row2.set_subtitle(&last_sync_subtitle(&state2));
        });
    }
    {
        let state = state.clone();
        interval_row.connect_selected_notify(move |row| {
            let mut s = state.borrow_mut();
            s.config.sync_interval_minutes = Config::sync_interval_for(row.selected());
            let _ = s.config.save();
        });
    }

    remote_group.add(&remote_url_row);
    remote_group.add(&remote_user_row);
    remote_group.add(&remote_pass_row);
    remote_group.add(&interval_row);
    remote_group.add(&sync_row);

    page.add(&server_group);
    page.add(&profiles_group);
    page.add(&remote_group);
    page.add(&status_group);
    storage_page.add(&storage_group);
    storage_page.add(&library_group);
    // Build both pages before adding either one, so Automatic can keep the
    // Advanced fields in sync when it generates credentials. Automatic is
    // still added first and remains the default page.
    dialog.add(&auto_page(
        state, widgets, &bind_row, &port_row, &user_row, &pass_row,
    ));
    dialog.add(&page);
    dialog.add(&storage_page);

    // Settings apply on close. Changing bind/port/credentials restarts a
    // running server so the new values actually take effect.
    {
        let (state, widgets) = (state.clone(), widgets.clone());
        let (autostart_row, mirror_row) = (autostart_row.clone(), mirror_row.clone());
        let default_view_row = default_view_row.clone();
        let retention_row = retention_row.clone();
        let initial_server = (
            config.bind.clone(),
            config.port,
            config.username.clone(),
            config.password.clone(),
        );
        dialog.connect_closed(move |_| {
            let restart_needed = {
                let mut s = state.borrow_mut();
                s.config.autostart_server = autostart_row.is_active();
                s.config.markdown_mirror = mirror_row.is_active();
                s.config.default_view = match default_view_row.selected() {
                    1 => "pinned",
                    2 => "archived",
                    _ => "notes",
                }
                .to_string();
                s.config.trash_retention_days =
                    crate::config::Config::trash_retention_days_for(retention_row.selected());
                s.store.markdown_mirror = s.config.markdown_mirror;

                if let Err(e) = s.config.save() {
                    eprintln!("failed to save settings: {e}");
                }

                // Server fields are already committed by their own signals.
                // Reading the entry widgets again here used to overwrite
                // credentials generated on the Automatic page with the stale
                // values those entries had when the dialog opened.
                let changed = initial_server.0 != s.config.bind
                    || initial_server.1 != s.config.port
                    || initial_server.2 != s.config.username
                    || initial_server.3 != s.config.password;
                changed && s.server.as_ref().is_some_and(|h| h.is_running())
            };

            if restart_needed {
                super::actions::set_server_running(&state, &widgets, false);
                super::actions::set_server_running(&state, &widgets, true);
            }
        });
    }

    dialog.present(Some(&widgets.window));
}

/// Plans the tags→folders conversion, shows the dry-run summary, and applies
/// it only after confirmation — with an automatic backup zip written first.
fn run_tag_conversion(state: &Shared, widgets: &Rc<Widgets>) {
    let plan = {
        let s = state.borrow();
        let folders = s.store.folders().unwrap_or_default();
        crate::convert::plan(&s.notes, &folders)
    };
    if plan.is_empty() {
        super::window::toast(widgets, "Nothing to convert — everything is already organised");
        return;
    }

    let mut body = format!(
        "• {} folder(s) to create\n• {} note(s) to move\n• {} archived note(s) to restore",
        plan.create_folders.len(),
        plan.moves.len(),
        plan.unarchive.len(),
    );
    if plan.multi_tag > 0 {
        body.push_str(&format!(
            "\n• {} note(s) have several tags — the alphabetically first one becomes the folder; every tag is kept",
            plan.multi_tag
        ));
    }
    body.push_str("\n\nA backup zip of the whole library is saved next to the notes folder first.");

    let dialog = adw::AlertDialog::builder()
        .heading("Convert tags to folders?")
        .body(body)
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("convert", "Back up and convert");
    dialog.set_response_appearance("convert", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let (state, widgets) = (state.clone(), widgets.clone());
    let parent = widgets.window.clone();
    dialog.connect_response(None, move |_, response| {
        if response != "convert" {
            return;
        }
        // Re-plan at apply time: notes may have changed since the dialog opened.
        let plan = {
            let s = state.borrow();
            let folders = s.store.folders().unwrap_or_default();
            crate::convert::plan(&s.notes, &folders)
        };

        let data_dir = state.borrow().config.data_dir.clone();
        let today = &crate::model::iso8601(crate::model::now_millis())[..10];
        let backup_dest = data_dir
            .parent()
            .unwrap_or(&data_dir)
            .join(format!("notes-pre-conversion-{today}.zip"));
        if let Err(e) = crate::backup::export(&data_dir, &backup_dest) {
            super::window::toast(&widgets, &format!("Backup failed, nothing changed: {e}"));
            return;
        }

        let (moved, restored) = {
            let mut s = state.borrow_mut();
            for folder in &plan.create_folders {
                if let Err(e) = s.store.create_folder(folder) {
                    eprintln!("could not create folder {folder}: {e}");
                }
            }
            let mut restored = 0;
            for id in &plan.unarchive {
                let snapshot = s.notes.iter_mut().find_map(|n| {
                    (&n.id == id).then(|| {
                        n.archived_at = None;
                        n.touch();
                        n.clone()
                    })
                });
                if let Some(n) = snapshot {
                    if s.store.save(&n).is_ok() {
                        restored += 1;
                    }
                }
            }
            let mut moved = 0;
            for (id, folder) in &plan.moves {
                let note = s.notes.iter().find(|n| &n.id == id).cloned();
                if let Some(n) = note {
                    if s.store.move_note(&n, Some(folder.clone())).is_ok() {
                        moved += 1;
                    }
                }
            }
            s.reload();
            s.selected = None;
            (moved, restored)
        };
        super::actions::refresh_sidebar(&state, &widgets);
        super::actions::refresh_list(&state, &widgets);
        super::actions::load_editor(&state, &widgets);
        super::window::toast(
            &widgets,
            &format!(
                "Converted: {} folders, moved {moved}, restored {restored}. Backup: {}",
                plan.create_folders.len(),
                backup_dest.display()
            ),
        );
    });
    dialog.present(Some(&parent));
}

/// Prompts for a destination and writes a whole-vault `.zip` backup there.
fn pick_and_export_backup(state: &Shared, widgets: &Rc<Widgets>) {
    let data_dir = state.borrow().config.data_dir.clone();
    let today = &crate::model::iso8601(crate::model::now_millis())[..10];

    let filter = gtk::FileFilter::new();
    filter.add_pattern("*.zip");
    filter.set_name(Some("Zip archive (*.zip)"));
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);

    let dialog = gtk::FileDialog::builder()
        .title("Save a backup of all notes")
        .initial_name(crate::backup::suggested_filename(today))
        .filters(&filters)
        .build();

    let widgets = widgets.clone();
    let parent = widgets.window.clone();
    dialog.save(Some(&parent), gio::Cancellable::NONE, move |result| {
        let Ok(file) = result else { return }; // user cancelled
        let Some(dest) = file.path() else { return };
        match crate::backup::export(&data_dir, &dest) {
            Ok(n) => super::window::toast(
                &widgets,
                &format!("Backed up {n} files to {}", dest.display()),
            ),
            Err(e) => super::window::toast(&widgets, &format!("Backup failed: {e}")),
        }
    });
}

/// Chooses a Notes backup, confirms the overwrite, and refreshes every view.
/// The backup module validates the complete archive and creates a recovery
/// snapshot before writing the first live file.
fn pick_and_restore_backup(state: &Shared, widgets: &Rc<Widgets>) {
    let filter = gtk::FileFilter::new();
    filter.add_pattern("*.zip");
    filter.set_name(Some("Notes backup (*.zip)"));
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);

    let chooser = gtk::FileDialog::builder()
        .title("Choose a Notes backup")
        .filters(&filters)
        .build();
    let (state, widgets) = (state.clone(), widgets.clone());
    let parent = widgets.window.clone();
    chooser.open(Some(&parent), gio::Cancellable::NONE, move |result| {
        let Ok(file) = result else { return };
        let Some(path) = file.path() else { return };

        let confirm = adw::AlertDialog::builder()
            .heading("Restore this backup?")
            .body(
                "Files from the backup will replace matching files in the notes folder. \
                 Notes created after the backup are kept, and an automatic safety backup \
                 of the current vault is made first.",
            )
            .build();
        confirm.add_response("cancel", "Cancel");
        confirm.add_response("restore", "Restore");
        confirm.set_response_appearance("restore", adw::ResponseAppearance::Destructive);
        confirm.set_default_response(Some("cancel"));
        confirm.set_close_response("cancel");

        let (state, widgets) = (state.clone(), widgets.clone());
        let parent = widgets.window.clone();
        confirm.connect_response(None, move |_, response| {
            if response != "restore" {
                return;
            }
            let data_dir = state.borrow().config.data_dir.clone();
            match crate::backup::restore(&path, &data_dir) {
                Ok(report) => {
                    {
                        let mut s = state.borrow_mut();
                        s.reload();
                        s.selected = None;
                    }
                    super::actions::refresh_sidebar(&state, &widgets);
                    super::actions::refresh_list(&state, &widgets);
                    super::actions::load_editor(&state, &widgets);
                    super::window::toast(
                        &widgets,
                        &format!(
                            "Restored {} files. Safety backup: {}",
                            report.restored_files,
                            report.recovery_backup.display()
                        ),
                    );
                }
                Err(e) => super::window::toast(&widgets, &format!("Restore failed: {e}")),
            }
        });
        confirm.present(Some(&parent));
    });
}

/// Prompts for a Keep Takeout `.zip` and imports it.
fn pick_and_import_keep(state: &Shared, widgets: &Rc<Widgets>) {
    let filter = gtk::FileFilter::new();
    filter.add_pattern("*.zip");
    filter.set_name(Some("Takeout archive (*.zip)"));
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);

    let dialog = gtk::FileDialog::builder()
        .title("Choose a Google Keep Takeout .zip")
        .filters(&filters)
        .build();

    let (state, widgets) = (state.clone(), widgets.clone());
    let parent = widgets.window.clone();
    dialog.open(Some(&parent), gio::Cancellable::NONE, move |result| {
        let Ok(file) = result else { return };
        let Some(path) = file.path() else { return };
        match crate::import_keep::scan_zip(&path) {
            Ok(report) => confirm_import(&state, &widgets, report, "Google Keep"),
            Err(e) => super::window::toast(&widgets, &format!("Could not read archive: {e}")),
        }
    });
}

/// Prompts for an Obsidian / Markdown folder and imports it.
fn pick_and_import_vault(state: &Shared, widgets: &Rc<Widgets>) {
    let dialog = gtk::FileDialog::builder()
        .title("Choose a notes folder")
        .build();

    let (state, widgets) = (state.clone(), widgets.clone());
    let parent = widgets.window.clone();
    dialog.select_folder(Some(&parent), gio::Cancellable::NONE, move |result| {
        let Ok(folder) = result else { return };
        let Some(path) = folder.path() else { return };
        match crate::import_md::scan_vault(&path) {
            Ok(report) => confirm_import(&state, &widgets, report, "Markdown"),
            Err(e) => super::window::toast(&widgets, &format!("Could not read folder: {e}")),
        }
    });
}

/// Shows what was found and commits only on confirmation. Nothing is written
/// until the user says so — the scan is read-only up to this point.
fn confirm_import(
    state: &Shared,
    widgets: &Rc<Widgets>,
    report: crate::import_md::ImportReport,
    source: &str,
) {
    let found = report.imported.len();
    if found == 0 {
        super::window::toast(widgets, &format!("No {source} notes found"));
        return;
    }

    // How many are genuinely new vs. updates, computed against the current
    // store so the number isn't misleading on a re-import.
    let existing = state.borrow().store.load_all().unwrap_or_default();
    let known: std::collections::HashSet<&str> = existing.iter().map(|n| n.id.as_str()).collect();
    let new_count = report
        .imported
        .iter()
        .filter(|n| !known.contains(n.id.as_str()))
        .count();
    let update_count = found - new_count;

    let mut body = format!("Found {found} notes ({new_count} new");
    if update_count > 0 {
        body.push_str(&format!(", {update_count} already imported"));
    }
    body.push(')');
    if !report.skipped.is_empty() {
        body.push_str(&format!(
            "\n{} file(s) skipped (unreadable).",
            report.skipped.len()
        ));
    }

    let dialog = adw::AlertDialog::builder()
        .heading(format!("Import from {source}?"))
        .body(body)
        .build();

    // Let the user pick how to organise: Keep gives labels, Obsidian gives
    // folders, and some people want the opposite. Default is source-appropriate.
    let organize = adw::ComboRow::builder()
        .title("Organise by")
        .model(&gtk::StringList::new(&["As-is", "Folders", "Tags"]))
        .selected(0)
        .build();
    let group = adw::PreferencesGroup::new();
    group.add(&organize);
    dialog.set_extra_child(Some(&group));

    dialog.add_response("cancel", "Cancel");
    dialog.add_response("import", "Import");
    dialog.set_response_appearance("import", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("import"));
    dialog.set_close_response("cancel");

    let (state, widgets) = (state.clone(), widgets.clone());
    let parent = widgets.window.clone();
    dialog.connect_response(None, move |_, response| {
        if response != "import" {
            return;
        }
        // Clone per invocation: the closure is Fn, and reorganize mutates.
        let mut report = report.clone();
        let mode = match organize.selected() {
            1 => crate::import_md::OrganizeMode::Folders,
            2 => crate::import_md::OrganizeMode::Tags,
            _ => crate::import_md::OrganizeMode::AsIs,
        };
        report.reorganize(mode);
        let (added, updated, failed) = {
            let s = state.borrow();
            let existing = s.store.load_all().unwrap_or_default();
            // An explicit organisation choice must also affect notes from an
            // earlier import. Preserve newer local text and update only their
            // folder/tags when the import itself is older.
            report.commit_reorganized(
                &s.store,
                &existing,
                mode != crate::import_md::OrganizeMode::AsIs,
            )
        };
        // Reload so the imported notes, folders and tags all appear at once.
        {
            let mut s = state.borrow_mut();
            s.reload();
        }
        super::actions::refresh_sidebar(&state, &widgets);
        super::actions::refresh_list(&state, &widgets);

        let mut msg = format!("Imported {added} new");
        if updated > 0 {
            msg.push_str(&format!(", updated {updated}"));
        }
        if failed > 0 {
            msg.push_str(&format!(", {failed} failed"));
        }
        super::window::toast(&widgets, &msg);
    });
    dialog.present(Some(&parent));
}

/// The "Automatic" server tab: one switch generates a friendly name + password,
/// finds a free port, binds to the LAN and starts — then shows copy-pasteable
/// details for the phone. No networking knowledge needed.
fn auto_page(
    state: &Shared,
    widgets: &Rc<Widgets>,
    bind_row: &adw::ComboRow,
    port_row: &adw::SpinRow,
    user_row: &adw::EntryRow,
    pass_row: &adw::PasswordEntryRow,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Automatic")
        .icon_name("network-wireless-symbolic")
        .build();

    let group = adw::PreferencesGroup::builder()
        .title("Sync with your phone")
        .description("Turn this on, then copy the details into the Android app.")
        .build();

    let switch = adw::SwitchRow::builder().title("Server running").build();

    // Detail rows with a copy button each. Hidden until the server is running.
    let make = |title: &str| -> (adw::ActionRow, gtk::Button) {
        let btn = gtk::Button::builder()
            .icon_name("edit-copy-symbolic")
            .css_classes(vec!["flat"])
            .valign(gtk::Align::Center)
            .tooltip_text("Copy")
            .build();
        let row = adw::ActionRow::builder().title(title).build();
        row.add_suffix(&btn);
        (row, btn)
    };
    let (url_row, url_copy) = make("Address");
    let (auto_user_row, user_copy) = make("Username");
    let (auto_pass_row, pass_copy) = make("Password");

    group.add(&switch);
    group.add(&url_row);
    group.add(&auto_user_row);
    group.add(&auto_pass_row);
    page.add(&group);

    // Copy the row's subtitle (the value) to the clipboard.
    let copy = |row: &adw::ActionRow, btn: &gtk::Button, widgets: &Rc<Widgets>| {
        let (row, widgets) = (row.clone(), widgets.clone());
        btn.connect_clicked(move |_| {
            if let Some(text) = row.subtitle() {
                widgets.window.clipboard().set_text(&text);
                super::window::toast(&widgets, "Copied");
            }
        });
    };
    copy(&url_row, &url_copy, widgets);
    copy(&auto_user_row, &user_copy, widgets);
    copy(&auto_pass_row, &pass_copy, widgets);

    // Fills the detail rows from current config + running state.
    let refresh = {
        let (state, url_row, user_row, pass_row) = (
            state.clone(),
            url_row.clone(),
            auto_user_row.clone(),
            auto_pass_row.clone(),
        );
        move || {
            let s = state.borrow();
            let running = s.server.as_ref().is_some_and(|h| h.is_running());
            for r in [&url_row, &user_row, &pass_row] {
                r.set_visible(running);
            }
            if running {
                url_row.set_subtitle(&crate::server::local_urls(s.config.port).join("   "));
                user_row.set_subtitle(&s.config.username);
                pass_row.set_subtitle(&s.config.password);
            }
        }
    };
    refresh();
    let running = state
        .borrow()
        .server
        .as_ref()
        .is_some_and(|h| h.is_running());
    switch.set_active(running);
    widgets.server_row.set_active(running);

    // The Advanced and Automatic pages expose the same server state. Keep the
    // Automatic switch honest when the Advanced switch starts or stops it.
    {
        let (state, switch) = (state.clone(), switch.clone());
        widgets.server_row.connect_active_notify(move |_| {
            let running = state
                .borrow()
                .server
                .as_ref()
                .is_some_and(|h| h.is_running());
            if switch.is_active() != running {
                switch.set_active(running);
            }
        });
    }

    {
        let (state, widgets) = (state.clone(), widgets.clone());
        let (bind_row, port_row, user_row, pass_row) = (
            bind_row.clone(),
            port_row.clone(),
            user_row.clone(),
            pass_row.clone(),
        );
        switch.connect_active_notify(move |sw| {
            if sw.is_active() {
                let generated = {
                    let mut s = state.borrow_mut();
                    // The server config is generated and saved exactly ONCE — the
                    // first time the user turns this on. After that the same
                    // name / password / port are reused on every start: the phone
                    // has them saved, so a changing address would silently break
                    // its sync. Only the Advanced tab edits or clears them. A
                    // non-empty password is the "already set up" signal.
                    if s.config.password.is_empty() {
                        s.config.username = crate::naming::friendly_name();
                        s.config.password = crate::naming::random_password(12);
                        // Automatic means LAN-reachable, on a port that's free.
                        s.config.bind = "0.0.0.0".to_string();
                        if let Some(p) = crate::naming::find_available_port(s.config.port) {
                            s.config.port = p;
                        }
                        let _ = s.config.save();
                        Some((
                            s.config.bind.clone(),
                            s.config.port,
                            s.config.username.clone(),
                            s.config.password.clone(),
                        ))
                    } else {
                        None
                    }
                };
                if let Some((bind, port, username, password)) = generated {
                    bind_row.set_selected(u32::from(bind == "0.0.0.0"));
                    port_row.set_value(f64::from(port));
                    user_row.set_text(&username);
                    pass_row.set_text(&password);
                }
                super::actions::set_server_running(&state, &widgets, true);
            } else {
                super::actions::set_server_running(&state, &widgets, false);
            }
            let running = state
                .borrow()
                .server
                .as_ref()
                .is_some_and(|h| h.is_running());
            widgets.server_row.set_active(running);
            if sw.is_active() != running {
                sw.set_active(running);
            }
            refresh();
        });
    }

    page
}
