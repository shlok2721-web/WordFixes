//! End-to-end client-mode sync, run against this app's own WebDAV server.
//!
//! The server is the same code the phone talks to, so a passing run here means
//! the client half speaks the protocol the phone already uses — which is the
//! whole point of client mode.

use notes::model::Note;
use notes::store::Store;
use notes::{server, sync, webdav};

struct Fixture {
    _handle: server::ServerHandle,
    local: Store,
    remote: Store,
    client: webdav::Client,
    _dir: tempdir::TempDir,
}

/// Ports are per-test so the suite can run in parallel.
fn start(port: u16) -> Fixture {
    let dir = tempdir::TempDir::new();
    let local_root = dir.path().join("local");
    let remote_root = dir.path().join("remote");
    let local = Store::new(local_root).unwrap();
    let remote = Store::new(remote_root.clone()).unwrap();

    let handle = server::start(server::ServerConfig {
        bind: "127.0.0.1".into(),
        port,
        username: "u".into(),
        password: "p".into(),
        root: remote_root,
    })
    .expect("server bound");

    let client = webdav::Client::new(&format!("http://127.0.0.1:{port}"), "u", "p");
    Fixture {
        _handle: handle,
        local,
        remote,
        client,
        _dir: dir,
    }
}

fn note(id: &str, title: &str, updated: i64) -> Note {
    Note {
        id: id.into(),
        title: title.into(),
        updated_at: updated,
        ..Note::default()
    }
}

fn titles(store: &Store) -> Vec<String> {
    let mut t: Vec<String> = store.load_all().unwrap().into_iter().map(|n| n.title).collect();
    t.sort();
    t
}

#[test]
fn each_side_gets_what_the_other_had() {
    let f = start(18131);
    f.local.save(&note("a", "Mine", 100)).unwrap();
    f.remote.save(&note("b", "Θέμα", 100)).unwrap();

    let s = sync::run_once(&f.local, &f.client).unwrap();
    assert_eq!(s.failed, Vec::<String>::new());
    assert_eq!(s.pushed, 1);
    assert_eq!(s.pulled, 1);
    assert_eq!(titles(&f.local), vec!["Mine", "Θέμα"]);
    assert_eq!(titles(&f.remote), vec!["Mine", "Θέμα"]);
}

#[test]
fn the_newer_edit_wins_and_the_older_one_is_overwritten() {
    let f = start(18132);
    f.local.save(&note("a", "old local", 100)).unwrap();
    f.remote.save(&note("a", "new remote", 200)).unwrap();

    sync::run_once(&f.local, &f.client).unwrap();
    assert_eq!(titles(&f.local), vec!["new remote"]);

    // And the other direction, from the state we just reached.
    f.local.save(&note("a", "newest local", 300)).unwrap();
    sync::run_once(&f.local, &f.client).unwrap();
    assert_eq!(titles(&f.remote), vec!["newest local"]);
}

#[test]
fn a_deleted_note_stays_deleted_instead_of_coming_back() {
    let f = start(18133);
    f.local.save(&note("a", "doomed", 100)).unwrap();
    sync::run_once(&f.local, &f.client).unwrap();
    assert_eq!(titles(&f.remote), vec!["doomed"]);

    // Hard-delete locally: Store::delete writes the tombstone the phone reads.
    f.local.delete("a").unwrap();
    let s = sync::run_once(&f.local, &f.client).unwrap();
    assert_eq!(s.deleted_remote, 1);
    assert!(titles(&f.remote).is_empty());

    // The zombie check: a second run must not resurrect it from either side.
    let s = sync::run_once(&f.local, &f.client).unwrap();
    assert_eq!(s.pulled, 0);
    assert!(titles(&f.local).is_empty());
    assert!(titles(&f.remote).is_empty());
}

#[test]
fn notes_in_folders_keep_their_folder_across_the_wire() {
    let f = start(18134);
    let mut n = note("a", "Ψώνια", 100);
    n.folder_name = Some("Αγορές".into());
    f.local.save(&n).unwrap();

    sync::run_once(&f.local, &f.client).unwrap();

    let pulled = f.remote.load_all().unwrap();
    assert_eq!(pulled.len(), 1);
    assert_eq!(pulled[0].folder_name.as_deref(), Some("Αγορές"));
}

#[test]
fn a_field_this_build_does_not_model_survives_a_round_trip() {
    let f = start(18135);
    let mut n = note("a", "Kept", 100);
    n.extra
        .insert("futureField".into(), serde_json::json!({ "x": 1 }));
    f.local.save(&n).unwrap();

    sync::run_once(&f.local, &f.client).unwrap();

    let there = &f.remote.load_all().unwrap()[0];
    assert_eq!(there.extra.get("futureField"), Some(&serde_json::json!({"x":1})));
}

/// Minimal stand-in for the `tempfile` crate — one directory, removed on drop.
mod tempdir {
    use std::path::{Path, PathBuf};

    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new() -> Self {
            let p = std::env::temp_dir().join(format!("notes-sync-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
