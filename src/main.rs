mod backup;
mod config;
mod convert;
mod import_keep;
mod import_md;
mod model;
mod naming;
mod richtext;
mod server;
mod store;
mod sync;
mod webdav;
mod ui;
mod watcher;

use adw::prelude::*;
use gtk::gio;

// Distribution packages can provide their registered application ID at build
// time. Local/source and Debian builds retain the original ID.
const APP_ID: &str = match option_env!("NOTES_APP_ID") {
    Some(id) => id,
    None => "dev.local.Notes",
};

fn main() -> gtk::glib::ExitCode {
    configure_input_method();
    let app = adw::Application::builder().application_id(APP_ID).build();

    app.connect_activate(|app| {
        // A remote activation (second launch, or reopening while running in
        // the background) must raise the existing window, not build a second
        // UI over the same vault.
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        install_icons();
        gtk::Window::set_default_icon_name(APP_ID);
        let (window, state, widgets) = ui::window::build(app);
        window.set_icon_name(Some(APP_ID));
        ui::settings::install_actions(app, &state, &widgets);
        install_accelerators(app);

        // Dropping the watcher stops the watch, so it has to outlive this
        // closure; the window owns it for the rest of the process.
        let watcher = ui::actions::start_watching(&state, &widgets);
        ui::actions::start_network_watch(&state, &widgets);
        // Client mode, if a remote is configured: one quiet pass at launch so
        // the vault is current before any editing, then on the chosen interval.
        ui::actions::sync_now(&state, &widgets, true);
        ui::actions::start_auto_sync(&state, &widgets);
        unsafe { window.set_data("notes-watcher", watcher) };

        window.present();
    });

    app.run_with_args::<&str>(&[])
}

fn install_accelerators(app: &adw::Application) {
    app.set_accels_for_action("app.new-note", &["<Primary>n"]);
    app.set_accels_for_action("app.new-checklist", &["<Primary><Shift>n"]);
    // Ctrl+S now means strikethrough in the editor; Settings keeps the
    // conventional Ctrl+comma.
    app.set_accels_for_action("app.server-settings", &["<Primary>comma"]);
}

/// KDE/Wayland can route GTK's single-line entries through the native Wayland
/// input context without completing dead-key Compose sequences. The concrete
/// symptom is that ordinary Greek letters work but tonos disappears. GTK's
/// built-in `simple` context uses the system Compose table and correctly
/// produces ά/έ/ή/ί/ό/ύ/ώ.
///
/// Respect an explicitly configured IME (for example IBus); this compatibility
/// fallback applies only when the environment did not choose one already.
fn configure_input_method() {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    let wayland = std::env::var("WAYLAND_DISPLAY").unwrap_or_default();
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let configured = std::env::var("GTK_IM_MODULE").unwrap_or_default();

    if should_force_simple_im(&session, &wayland, &desktop, &configured) {
        std::env::set_var("GTK_IM_MODULE", "simple");
    }
}

fn should_force_simple_im(session: &str, wayland: &str, desktop: &str, configured: &str) -> bool {
    configured.trim().is_empty()
        && (session.eq_ignore_ascii_case("wayland") || !wayland.trim().is_empty())
        && desktop
            .split(':')
            .any(|name| name.trim().eq_ignore_ascii_case("kde"))
}

#[cfg(test)]
mod input_method_tests {
    use super::should_force_simple_im;

    #[test]
    fn kde_wayland_without_an_explicit_ime_uses_compose_fallback() {
        assert!(should_force_simple_im("wayland", "wayland-0", "KDE", ""));
        assert!(should_force_simple_im("", "wayland-0", "KDE:PLASMA", ""));
    }

    #[test]
    fn an_explicit_input_method_is_never_overridden() {
        assert!(!should_force_simple_im(
            "wayland",
            "wayland-0",
            "KDE",
            "ibus"
        ));
    }

    #[test]
    fn x11_and_other_desktops_keep_their_default_input_context() {
        assert!(!should_force_simple_im("x11", "", "KDE", ""));
        assert!(!should_force_simple_im("wayland", "wayland-0", "GNOME", ""));
    }
}

/// Puts our bundled icons ahead of the desktop's icon theme.
///
/// Two reasons this is not optional:
///
/// 1. Icon names cannot be assumed to exist. Breeze (KDE) ships none of
///    `view-pin-symbolic`, `view-conceal-symbolic` or `view-list-symbolic`;
///    under it those rendered as broken squares.
/// 2. Falling back per-icon gives a mixed set — some Breeze, some ours —
///    which reads as sloppy. A designed app should look the same everywhere.
///
/// The cost is that we ignore the user's icon theme for these names. That is
/// the deliberate trade: consistency over local convention.
fn install_icons() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let theme = gtk::IconTheme::for_display(&display);

    // Installed locations first, then the source tree, so Debian, Flatpak and
    // `cargo run` all use the same bundled icon set.
    let mut candidates = vec![
        std::path::PathBuf::from("/usr/share/notes/icons"),
        std::path::PathBuf::from("/app/share/notes/icons"),
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/icons"),
    ];
    if let Some(appdir) = std::env::var_os("APPDIR") {
        candidates.push(std::path::PathBuf::from(appdir).join("usr/share/notes/icons"));
    }

    let ours: Vec<std::path::PathBuf> = candidates.into_iter().filter(|p| p.is_dir()).collect();

    if ours.is_empty() {
        return;
    }

    let mut paths = ours;
    paths.extend(theme.search_path());
    let refs: Vec<&std::path::Path> = paths.iter().map(|p| p.as_path()).collect();
    theme.set_search_path(&refs);
}

/// Used by the "Open notes folder" action.
pub fn open_path(path: &std::path::Path) {
    // Let Gio encode spaces, Greek characters and other URI-sensitive bytes.
    // String-concatenating `file://` produced invalid URIs for ordinary vault
    // paths such as this project's own parent directories.
    let uri = gio::File::for_path(path).uri();
    gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>).ok();
}
