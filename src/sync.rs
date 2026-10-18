//! Client-mode sync: pulling from and pushing to a remote WebDAV server.
//!
//! There is no custom protocol. The phone syncs by copying `notes/*.json` over
//! WebDAV and resolving collisions with last-write-wins on `updatedAt`, plus a
//! shared `notes/deletions.json` tombstone ledger. This module does the same
//! thing from the desktop side, so a PC pointed at someone else's server behaves
//! exactly like a second phone.
//!
//! The decision half (`plan`) is pure and offline-testable; the transport half
//! lives in [`crate::webdav`]. That split is deliberate — `plan` is the piece an
//! Android port would reimplement, and it is the piece worth having tests for.

use crate::model::Note;
use std::collections::{BTreeMap, BTreeSet};

/// A note file seen on the server. `path` is relative to the sync root, e.g.
/// `notes/abc.json` or `notes/Σχολείο/abc.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteNote {
    pub id: String,
    pub path: String,
    pub updated_at: i64,
}

/// One step of a sync run. Ordering within a run does not matter; each action
/// touches exactly one note.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    /// Download `path` and save it over the local copy.
    Pull { id: String, path: String },
    /// Upload the local note to `path`.
    Push { id: String, path: String },
    /// Remove the local copy — the server says it was deleted elsewhere.
    DeleteLocal { id: String },
    /// Remove `path` on the server: either it was deleted here, or the note
    /// moved folders and this is the file it left behind.
    DeleteRemote { id: String, path: String },
}

/// Where a note's JSON lives, relative to the sync root. Mirrors the on-disk
/// layout `Store` writes, which is what the phone expects.
pub fn remote_path(note: &Note) -> String {
    match note.folder_name.as_deref().filter(|f| !f.is_empty()) {
        Some(folder) => format!("notes/{folder}/{}.json", note.id),
        None => format!("notes/{}.json", note.id),
    }
}

/// Decides what a sync run should do.
///
/// `tombstones` is the merged `deletions.json` ledger (id → deletedAt) from both
/// sides. Rules, in order:
///
/// 1. A tombstoned note is deleted on both sides, even if the local copy is
///    newer. This intentionally matches the Android app, which treats the ledger
///    as authoritative — diverging from it produces zombie notes that one device
///    keeps deleting and the other keeps re-uploading. The cost is that an edit
///    racing a delete loses; the alternative is worse.
/// 2. Otherwise the higher `updatedAt` wins, both directions. Equal means done.
/// 3. A note that moved folders is pushed to its new path and its old remote
///    file is removed, so the phone doesn't end up with two copies.
pub fn plan(
    local: &[Note],
    remote: &[RemoteNote],
    tombstones: &BTreeMap<String, i64>,
) -> Vec<Action> {
    let by_id: BTreeMap<&str, &RemoteNote> =
        remote.iter().map(|r| (r.id.as_str(), r)).collect();
    let local_ids: BTreeSet<&str> = local.iter().map(|n| n.id.as_str()).collect();
    let mut actions = Vec::new();

    for note in local {
        let want = remote_path(note);
        let there = by_id.get(note.id.as_str());

        if tombstones.contains_key(&note.id) {
            actions.push(Action::DeleteLocal {
                id: note.id.clone(),
            });
            if let Some(r) = there {
                actions.push(Action::DeleteRemote {
                    id: note.id.clone(),
                    path: r.path.clone(),
                });
            }
            continue;
        }

        match there {
            None => actions.push(Action::Push {
                id: note.id.clone(),
                path: want,
            }),
            Some(r) if note.updated_at > r.updated_at => {
                if r.path != want {
                    actions.push(Action::DeleteRemote {
                        id: note.id.clone(),
                        path: r.path.clone(),
                    });
                }
                actions.push(Action::Push {
                    id: note.id.clone(),
                    path: want,
                });
            }
            Some(r) if note.updated_at < r.updated_at => actions.push(Action::Pull {
                id: note.id.clone(),
                path: r.path.clone(),
            }),
            Some(_) => {}
        }
    }

    for r in remote {
        if local_ids.contains(r.id.as_str()) {
            continue;
        }
        if tombstones.contains_key(&r.id) {
            actions.push(Action::DeleteRemote {
                id: r.id.clone(),
                path: r.path.clone(),
            });
        } else {
            actions.push(Action::Pull {
                id: r.id.clone(),
                path: r.path.clone(),
            });
        }
    }

    actions
}

/// Merges two `deletions.json` ledgers into `id → deletedAt`, keeping the
/// earliest timestamp for an id present in both.
pub fn merge_tombstones(ledgers: &[serde_json::Value]) -> BTreeMap<String, i64> {
    let mut out = BTreeMap::new();
    for ledger in ledgers {
        let Some(records) = ledger.get("deletedNotes").and_then(|v| v.as_array()) else {
            continue;
        };
        for r in records {
            let (Some(id), Some(at)) = (
                r.get("id").and_then(|v| v.as_str()),
                r.get("deletedAt").and_then(|v| v.as_i64()),
            ) else {
                continue;
            };
            out.entry(id.to_string())
                .and_modify(|prev: &mut i64| *prev = (*prev).min(at))
                .or_insert(at);
        }
    }
    out
}

/// Rebuilds a ledger file body from merged tombstones, in the wire format the
/// Android app reads.
pub fn tombstone_ledger(tombstones: &BTreeMap<String, i64>, device_id: &str) -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "deletedNotes": tombstones.iter().map(|(id, at)| serde_json::json!({
            "id": id,
            "deletedAt": at,
            "deviceId": device_id,
        })).collect::<Vec<_>>(),
    })
}

/// What a completed run did, for the status line and the toast.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Summary {
    pub pulled: usize,
    pub pushed: usize,
    pub deleted_local: usize,
    pub deleted_remote: usize,
    /// Actions that failed. A run reports them rather than aborting: one
    /// unreadable note must not stop the other two hundred from syncing.
    pub failed: Vec<String>,
}

const LEDGER_PATH: &str = "notes/deletions.json";

/// Reads the server's note index. Each file is fetched because `updatedAt`
/// lives inside the JSON — a file's mtime is when it was *copied*, which is not
/// the same thing and would make the newer side lose.
///
/// ponytail: fetches every note every run. Fine for a personal vault (hundreds
/// of small files); add an mtime pre-filter from PROPFIND if a run gets slow.
fn remote_index(client: &crate::webdav::Client) -> anyhow::Result<Vec<RemoteNote>> {
    let mut out = Vec::new();
    for path in client.list_json("notes")? {
        if path.ends_with("deletions.json") {
            continue;
        }
        let Ok(bytes) = client.get(&path) else { continue };
        let Ok(note) = serde_json::from_slice::<Note>(&bytes) else {
            continue;
        };
        out.push(RemoteNote {
            id: note.id,
            path,
            updated_at: note.updated_at,
        });
    }
    Ok(out)
}

/// One full sync pass against `client`.
pub fn run_once(store: &crate::store::Store, client: &crate::webdav::Client) -> anyhow::Result<Summary> {
    let local = store.load_all()?;
    let remote = remote_index(client)?;

    let remote_ledger = client
        .get(LEDGER_PATH)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| serde_json::json!({ "version": 1, "deletedNotes": [] }));
    let tombstones = merge_tombstones(&[store.deletions(), remote_ledger]);

    let by_id: BTreeMap<&str, &Note> = local.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut summary = Summary::default();

    for action in plan(&local, &remote, &tombstones) {
        let result = match &action {
            Action::Pull { path, .. } => client
                .get(path)
                .and_then(|b| Ok(serde_json::from_slice::<Note>(&b)?))
                .and_then(|n| store.save(&n))
                .map(|_| summary.pulled += 1),
            Action::Push { id, path } => match by_id.get(id.as_str()) {
                Some(note) => serde_json::to_vec_pretty(note)
                    .map_err(Into::into)
                    .and_then(|b| client.put(path, &b))
                    .map(|_| summary.pushed += 1),
                None => Ok(()),
            },
            Action::DeleteLocal { id } => {
                store.delete_synced(id).map(|_| summary.deleted_local += 1)
            }
            Action::DeleteRemote { path, .. } => {
                client.delete(path).map(|_| summary.deleted_remote += 1)
            }
        };
        if let Err(e) = result {
            summary.failed.push(format!("{action:?}: {e}"));
        }
    }

    // Both sides end up holding the union, so a tombstone reaches every device
    // even when they never sync at the same time.
    let merged = tombstone_ledger(&tombstones, &crate::store::device_id());
    if !tombstones.is_empty() {
        let _ = store.write_deletions(&merged);
        if let Err(e) = client.put(LEDGER_PATH, serde_json::to_string_pretty(&merged)?.as_bytes()) {
            summary.failed.push(format!("ledger: {e}"));
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: &str, updated: i64) -> Note {
        Note {
            id: id.into(),
            updated_at: updated,
            ..Note::default()
        }
    }

    fn rnote(id: &str, updated: i64) -> RemoteNote {
        RemoteNote {
            id: id.into(),
            path: format!("notes/{id}.json"),
            updated_at: updated,
        }
    }

    fn no_tombstones() -> BTreeMap<String, i64> {
        BTreeMap::new()
    }

    #[test]
    fn local_only_is_pushed_and_remote_only_is_pulled() {
        let actions = plan(&[note("a", 10)], &[rnote("b", 10)], &no_tombstones());
        assert!(actions.contains(&Action::Push {
            id: "a".into(),
            path: "notes/a.json".into()
        }));
        assert!(actions.contains(&Action::Pull {
            id: "b".into(),
            path: "notes/b.json".into()
        }));
    }

    #[test]
    fn newer_side_wins_both_directions() {
        assert_eq!(
            plan(&[note("a", 20)], &[rnote("a", 10)], &no_tombstones()),
            vec![Action::Push {
                id: "a".into(),
                path: "notes/a.json".into()
            }]
        );
        assert_eq!(
            plan(&[note("a", 10)], &[rnote("a", 20)], &no_tombstones()),
            vec![Action::Pull {
                id: "a".into(),
                path: "notes/a.json".into()
            }]
        );
    }

    #[test]
    fn identical_timestamps_do_nothing() {
        assert!(plan(&[note("a", 10)], &[rnote("a", 10)], &no_tombstones()).is_empty());
    }

    /// The rule that keeps zombies out: the ledger beats a newer local edit.
    #[test]
    fn tombstone_wins_over_a_newer_local_edit() {
        let tombs = BTreeMap::from([("a".to_string(), 5)]);
        let actions = plan(&[note("a", 99)], &[rnote("a", 1)], &tombs);
        assert_eq!(
            actions,
            vec![
                Action::DeleteLocal { id: "a".into() },
                Action::DeleteRemote {
                    id: "a".into(),
                    path: "notes/a.json".into()
                }
            ]
        );
    }

    #[test]
    fn tombstoned_remote_only_note_is_never_pulled_back() {
        let tombs = BTreeMap::from([("a".to_string(), 5)]);
        let actions = plan(&[], &[rnote("a", 99)], &tombs);
        assert_eq!(
            actions,
            vec![Action::DeleteRemote {
                id: "a".into(),
                path: "notes/a.json".into()
            }]
        );
    }

    #[test]
    fn a_moved_note_removes_the_file_it_left_behind() {
        let mut moved = note("a", 20);
        moved.folder_name = Some("Σχολείο".into());
        let actions = plan(&[moved], &[rnote("a", 10)], &no_tombstones());
        assert_eq!(
            actions,
            vec![
                Action::DeleteRemote {
                    id: "a".into(),
                    path: "notes/a.json".into()
                },
                Action::Push {
                    id: "a".into(),
                    path: "notes/Σχολείο/a.json".into()
                }
            ]
        );
    }

    #[test]
    fn merging_ledgers_keeps_the_earliest_deletion() {
        let a = serde_json::json!({"deletedNotes":[{"id":"x","deletedAt":50,"deviceId":"p"}]});
        let b = serde_json::json!({"deletedNotes":[{"id":"x","deletedAt":10,"deviceId":"d"},
                                                   {"id":"y","deletedAt":7,"deviceId":"d"}]});
        let merged = merge_tombstones(&[a, b]);
        assert_eq!(merged.get("x"), Some(&10));
        assert_eq!(merged.get("y"), Some(&7));
    }

    #[test]
    fn a_malformed_ledger_is_skipped_not_fatal() {
        let junk = serde_json::json!({"deletedNotes": "not an array"});
        let good = serde_json::json!({"deletedNotes":[{"id":"x","deletedAt":1,"deviceId":"d"}]});
        assert_eq!(merge_tombstones(&[junk, good]).len(), 1);
    }
}
