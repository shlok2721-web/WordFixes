//! Filesystem watcher for the notes directory.
//!
//! Without this, a note the phone pushes over WebDAV is invisible until the
//! app restarts — the store is only read at startup.
//!
//! The hard part is not noticing changes, it's ignoring our *own* writes.
//! Every keystroke saves a note, which fires an event, which would reload the
//! store and rebuild the list underneath the user. So writes are stamped, and
//! events arriving within a short window after one of our own writes are
//! treated as an echo of it.

use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

/// How long after our own write an event is assumed to be that write's echo.
const SELF_WRITE_ECHO_MS: i64 = 1_200;

/// How long to let events settle before reloading, so a sync that drops 50
/// files causes one reload rather than 50.
pub const DEBOUNCE_MS: u64 = 400;

/// Shared clock of the last write this process made.
#[derive(Clone, Default)]
pub struct WriteStamp(Arc<AtomicI64>);

impl WriteStamp {
    pub fn touch(&self) {
        self.0.store(crate::model::now_millis(), Ordering::SeqCst);
    }

    pub fn last_millis(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// Set by the watcher thread, drained by the UI tick.
#[derive(Clone, Default)]
pub struct DirtyFlag(Arc<std::sync::atomic::AtomicBool>);

impl DirtyFlag {
    pub fn set(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn take(&self) -> bool {
        self.0.swap(false, Ordering::SeqCst)
    }

    /// Reads the pending state without consuming it. A change seen during the
    /// self-write echo window must remain pending until that window expires;
    /// consuming it immediately can drop a simultaneous phone sync forever.
    pub fn is_set(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Decides whether a pending change should cause a reload.
///
/// Split out from the watcher so the debounce rule can be tested without a
/// filesystem or a main loop.
pub fn should_reload(dirty: bool, now_ms: i64, last_self_write_ms: i64) -> bool {
    dirty && (now_ms - last_self_write_ms) > SELF_WRITE_ECHO_MS
}

/// Starts watching `root` recursively. The returned watcher must be kept
/// alive — dropping it stops the watch.
pub fn watch(root: &Path, dirty: DirtyFlag) -> anyhow::Result<notify::RecommendedWatcher> {
    use notify::{EventKind, RecursiveMode, Watcher};

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        // Access-only events (a WebDAV GET, an editor peeking) change nothing.
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        // Ignore our own temp files from `write_atomic`; the rename that
        // follows produces its own event.
        if event
            .paths
            .iter()
            .all(|p| p.extension().is_some_and(|e| e == "tmp"))
        {
            return;
        }
        dirty.set();
    })?;

    watcher.watch(root, RecursiveMode::Recursive)?;
    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_tree_never_reloads() {
        assert!(!should_reload(false, 10_000, 0));
    }

    #[test]
    fn our_own_write_does_not_trigger_a_reload() {
        // Change seen 200ms after we saved: that's our own echo.
        assert!(!should_reload(true, 10_200, 10_000));
    }

    #[test]
    fn a_foreign_write_triggers_a_reload() {
        // Nothing written by us for 5s, so this came from the phone.
        assert!(should_reload(true, 15_000, 10_000));
    }

    #[test]
    fn the_echo_window_is_exclusive_at_its_edge() {
        assert!(!should_reload(true, 10_000 + SELF_WRITE_ECHO_MS, 10_000));
        assert!(should_reload(true, 10_001 + SELF_WRITE_ECHO_MS, 10_000));
    }

    #[test]
    fn dirty_flag_drains_exactly_once() {
        let flag = DirtyFlag::default();
        assert!(!flag.take());
        flag.set();
        flag.set();
        assert!(flag.take(), "first drain sees the change");
        assert!(!flag.take(), "second drain is clean");
    }

    #[test]
    fn inspecting_a_dirty_flag_does_not_consume_it() {
        let flag = DirtyFlag::default();
        flag.set();
        assert!(flag.is_set());
        assert!(flag.is_set());
        assert!(flag.take());
        assert!(!flag.is_set());
    }

    #[test]
    fn write_stamp_advances() {
        let stamp = WriteStamp::default();
        assert_eq!(stamp.last_millis(), 0);
        stamp.touch();
        assert!(stamp.last_millis() > 0);
    }

    /// End to end against a real directory: a file appearing must be noticed.
    #[test]
    fn notices_a_file_appearing() {
        let dir = std::env::temp_dir().join(format!("sn-watch-{}", crate::model::new_uuid()));
        std::fs::create_dir_all(&dir).unwrap();

        let dirty = DirtyFlag::default();
        let _watcher = watch(&dir, dirty.clone()).unwrap();

        std::fs::write(dir.join("incoming.json"), b"{}").unwrap();

        // inotify delivery is asynchronous; poll rather than sleep a fixed time.
        let mut seen = false;
        for _ in 0..100 {
            if dirty.take() {
                seen = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(seen, "watcher missed a new file");
    }
}
