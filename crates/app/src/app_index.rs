//! Live maintenance of the application index.
//!
//! `platform_scanner().scan()` is a snapshot, so something has to decide *when*
//! to take a new one while Steward is running. This module owns that decision:
//!
//! - On Windows a `ReadDirectoryChangesW` watch covers the per-user and
//!   all-users Start Menu `Programs` trees, where installers create and remove
//!   `.lnk` shortcuts. Any change there queues a rescan, and a trailing debounce
//!   coalesces the burst of shortcuts a single installer writes.
//! - A reconcile timer covers what the shortcut walk cannot see: UWP / Microsoft
//!   Store apps live in `shell:AppsFolder`, not as `.lnk` files, so their install
//!   and uninstall only surface on a full rescan.
//!
//! Both paths funnel into [`ScanTimer`], a pure state machine so the timing rules
//! are unit-testable without a real directory watcher.

use std::time::{Duration, Instant};

use steward_core_engine::AppEntry;

#[cfg(target_os = "windows")]
use steward_core_engine::file_index::DirectoryWatcher;

/// Quiet period after the last Start Menu change before a scan is queued.
///
/// One install commonly writes several shortcuts; waiting for the burst to
/// settle keeps that to a single scan.
pub(crate) const START_MENU_DEBOUNCE: Duration = Duration::from_secs(1);

/// How often a full rescan runs regardless of watcher activity.
///
/// This is the safety net for UWP/Store apps (which never touch the Start Menu
/// tree) and for Start Menu roots the watcher could not open.
pub(crate) const APP_RECONCILE_INTERVAL: Duration = Duration::from_secs(30);

/// Whether a fresh scan produced a different app set than `current`.
///
/// Compared the way the cache keys apps (path, case-insensitive on Windows, plus
/// the display name) so an unchanged scan does not rebuild the search index or
/// rewrite the cache.
pub(crate) fn entries_differ(current: &[AppEntry], scanned: &[AppEntry]) -> bool {
    if current.len() != scanned.len() {
        return true;
    }
    let key = |app: &AppEntry| (app.path.to_string_lossy().to_lowercase(), app.name.clone());
    let mut left: Vec<_> = current.iter().map(key).collect();
    let mut right: Vec<_> = scanned.iter().map(key).collect();
    left.sort();
    right.sort();
    left != right
}

/// Debounce + reconcile timer for the application index.
struct ScanTimer {
    /// When a scan becomes due after a Start Menu change (trailing debounce).
    due: Option<Instant>,
    /// When the last scan was started, for [`APP_RECONCILE_INTERVAL`].
    last_started: Instant,
}

impl ScanTimer {
    fn new(now: Instant) -> Self {
        Self {
            due: None,
            last_started: now,
        }
    }

    /// Note a Start Menu change, pushing the debounce deadline out.
    fn note_change(&mut self, now: Instant) {
        self.due = Some(now + START_MENU_DEBOUNCE);
    }

    /// Whether a scan should start now. A scan already in flight defers both
    /// triggers, so the request survives until it finishes.
    fn take(&mut self, now: Instant, scanning: bool) -> bool {
        if scanning {
            return false;
        }
        let debounced = self.due.is_some_and(|due| now >= due);
        let reconciled = now.duration_since(self.last_started) >= APP_RECONCILE_INTERVAL;
        if !debounced && !reconciled {
            return false;
        }
        self.due = None;
        self.last_started = now;
        true
    }
}

/// Decides when the launcher should re-run the application scanner.
pub(crate) struct AppIndexWatcher {
    /// `None` when no Start Menu root could be opened; the reconcile timer then
    /// carries the index on its own.
    #[cfg(target_os = "windows")]
    watcher: Option<DirectoryWatcher>,
    timer: ScanTimer,
}

impl AppIndexWatcher {
    /// Start watching the Start Menu roots. Roots that cannot be opened are
    /// skipped, so a missing or unreadable tree degrades to the reconcile timer
    /// instead of failing startup.
    pub(crate) fn start(now: Instant) -> Self {
        #[cfg(target_os = "windows")]
        let watcher = {
            let roots = steward_core_engine::start_menu_roots();
            if roots.is_empty() {
                None
            } else {
                Some(DirectoryWatcher::watch(&roots))
            }
        };
        Self {
            #[cfg(target_os = "windows")]
            watcher,
            timer: ScanTimer::new(now),
        }
    }

    /// Fold pending Start Menu notifications into the timer and report whether a
    /// scan should start now.
    pub(crate) fn take_scan_request(&mut self, scanning: bool, now: Instant) -> bool {
        #[cfg(target_os = "windows")]
        if let Some(watcher) = &self.watcher {
            // Any change under the Start Menu counts: an uninstall can remove a
            // whole program folder, so filtering for `.lnk` names would miss it.
            // An overflow notice is also just a reason to rescan.
            while watcher.try_recv().is_some() {
                self.timer.note_change(now);
            }
        }
        self.timer.take(now, scanning)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(paths: &[(&str, &str)]) -> Vec<AppEntry> {
        paths
            .iter()
            .map(|(name, path)| AppEntry {
                name: (*name).into(),
                path: path.into(),
            })
            .collect()
    }

    #[test]
    fn an_unchanged_scan_does_not_rebuild_the_index() {
        let current = entries(&[("Editor", "C:/editor.exe"), ("Terminal", "C:/term.exe")]);
        assert!(!entries_differ(&current, &current.clone()));
        // Path case is not a change on Windows, but the display name is.
        assert!(!entries_differ(
            &current,
            &entries(&[("Editor", "c:/EDITOR.EXE"), ("Terminal", "C:/term.exe")])
        ));
        assert!(entries_differ(
            &current,
            &entries(&[("Editor Pro", "C:/editor.exe"), ("Terminal", "C:/term.exe")])
        ));
        assert!(entries_differ(
            &current,
            &entries(&[("Editor", "C:/editor.exe")])
        ));
    }

    #[test]
    fn debounce_coalesces_a_burst_into_one_scan() {
        let start = Instant::now();
        let mut timer = ScanTimer::new(start);
        timer.note_change(start);
        timer.note_change(start + Duration::from_millis(200));
        // Still inside the quiet period.
        assert!(!timer.take(start + Duration::from_millis(900), false));
        // The last change was at +200ms, so the scan is due at +1.2s.
        assert!(timer.take(start + Duration::from_millis(1_200), false));
        // The queued request is consumed: nothing is due right after.
        assert!(!timer.take(start + Duration::from_millis(1_300), false));
    }

    #[test]
    fn a_change_during_a_scan_is_not_lost() {
        let start = Instant::now();
        let mut timer = ScanTimer::new(start);
        timer.note_change(start);
        assert!(timer.take(start + Duration::from_secs(1), false));
        // A shortcut appears while the scan runs; the request waits for it.
        timer.note_change(start + Duration::from_secs(1));
        assert!(!timer.take(start + Duration::from_secs(2), true));
        assert!(timer.take(start + Duration::from_secs(2), false));
    }

    #[test]
    fn the_reconcile_timer_fires_without_any_watcher_activity() {
        let start = Instant::now();
        let mut timer = ScanTimer::new(start);
        assert!(!timer.take(start + Duration::from_secs(29), false));
        assert!(timer.take(start + Duration::from_secs(30), false));
        assert!(!timer.take(start + Duration::from_secs(59), false));
        assert!(timer.take(start + Duration::from_secs(60), false));
    }
}
