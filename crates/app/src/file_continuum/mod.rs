//! Automatically attach a directory picker to the foreground open/save dialog.
//! Directory I/O and Explorer COM calls stay off the UI thread.

mod native;
mod search;

use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use gpui::{App, AsyncApp, Context, Window};
use steward_core_engine::MatchTier;
use steward_ui_components::ResultItem;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};

use crate::launcher::{RankedRow, RowRank};
use crate::{
    i18n::Localization,
    launcher::{LauncherState, StewardApp},
    platform,
    window::{hide_window, show_launcher, sync_directory_picker_bounds},
};
use native::DialogTarget;

pub(crate) const STATUS_HEIGHT: f32 = 24.0;
const HISTORY_LIMIT: usize = 50;
/// How long after a session first turns passive the picker keeps trying to take
/// the keyboard back, and how often it retries.
///
/// A freshly opened open/save dialog takes the foreground *after* the picker
/// attached to it, so the bar's first keyboard hand-off is stolen back by the
/// dialog itself and the query box never becomes usable. For this long the poll
/// re-activates the bar whenever the dialog (not the user) is holding the
/// keyboard; after it, a passive session stays passive until the user clicks
/// the bar, which is what "click the search box to type" promises.
const PASSIVE_RECLAIM_WINDOW: Duration = Duration::from_millis(4000);
const PASSIVE_RECLAIM_RETRY: Duration = Duration::from_millis(300);
/// Hard cap on reclaim attempts per passive episode.
///
/// The window alone is not enough to stop a tug of war: a dialog that keeps
/// re-activating itself would have the bar take the keyboard back every 300ms
/// for the whole window, and each round visibly re-places the bar. After a few
/// tries the picker gives up and stays passive (click to type), which is the
/// documented fallback.
const PASSIVE_RECLAIM_LIMIT: u32 = 3;
type SearchRequest = (u64, String, Vec<PathBuf>);

pub(crate) struct FileContinuum {
    pub(crate) target: Option<DialogTarget>,
    /// True while the bar is attached but does not hold the keyboard (a dialog
    /// the picker already attached to, or a refused focus hand-off); clicking
    /// the bar clears it.
    pub(crate) passive: bool,
    pub(crate) navigating: bool,
    /// True once this session has sent a path to the dialog.
    ///
    /// A jump is the one thing the user asked for that the picker must not undo:
    /// the dialog comes back to the foreground the moment the path is in its
    /// address bar, so without this the poll would attach a fresh picker to the
    /// dialog it had just navigated and the box would pop straight back up. It
    /// only clears when the *dialog* changes (the same window re-opened is a new
    /// session), because the picker is otherwise owned by the dialog for as long
    /// as the dialog exists.
    pub(crate) navigated: bool,
    pub(crate) status: String,
    /// The supported dialog that owns the foreground, refreshed every 100 ms.
    foreground_dialog: Option<DialogTarget>,
    /// The dialog the picker attached to last: the first attach to a dialog
    /// takes the keyboard, later attaches leave it with the dialog.
    last_target: Option<DialogTarget>,
    /// When the bar may next try to take the keyboard back after it turned
    /// passive (see [`PASSIVE_RECLAIM_WINDOW`]), and when that window closes.
    /// `None` means the picker is not passive, so there is nothing to reclaim.
    passive_until: Option<Instant>,
    /// Reclaim attempts made in the current passive episode, capped at
    /// [`PASSIVE_RECLAIM_LIMIT`] so a dialog that keeps re-activating itself
    /// cannot turn the recovery into a tug of war.
    reclaim_attempts: u32,
    reclaim_at: Instant,
    checked_at: Instant,
    history: Arc<Mutex<Vec<PathBuf>>>,
    stopped: Arc<AtomicBool>,
    search_request: Arc<Mutex<Option<SearchRequest>>>,
    search_wake: Sender<()>,
    search_results: Receiver<(u64, Vec<PathBuf>)>,
    generation: u64,
    navigation: Option<Receiver<Result<(), String>>>,
    cancelled: Arc<AtomicBool>,
}

impl FileContinuum {
    pub(crate) fn new() -> Self {
        let history = Arc::new(Mutex::new(Vec::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let (worker_history, worker_stop) = (history.clone(), stopped.clone());
        std::thread::spawn(move || {
            let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
            if !initialized {
                return;
            }
            while !worker_stop.load(Ordering::Relaxed) {
                if let Some(path) = native::explorer_directory() {
                    remember(&mut worker_history.lock().unwrap(), path);
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            unsafe { CoUninitialize() };
        });

        // One worker, one replaceable request: typing cannot spawn an unbounded
        // number of filesystem operations. Generations reject late replies.
        let search_request = Arc::new(Mutex::new(None::<SearchRequest>));
        let worker_request = search_request.clone();
        let (search_wake, wake) = bounded(1);
        let (results, search_results) = unbounded();
        std::thread::spawn(move || {
            while wake.recv().is_ok() {
                let request = worker_request.lock().unwrap().take();
                if let Some((generation, query, recent)) = request {
                    let paths = search::search_directories(&query, &recent);
                    if results.send((generation, paths)).is_err() {
                        break;
                    }
                }
            }
        });

        Self {
            target: None,
            passive: false,
            navigating: false,
            navigated: false,
            status: String::new(),
            foreground_dialog: None,
            last_target: None,
            passive_until: None,
            reclaim_attempts: 0,
            reclaim_at: Instant::now(),
            checked_at: Instant::now(),
            history,
            stopped,
            search_request,
            search_wake,
            search_results,
            generation: 0,
            navigation: None,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Refresh the foreground independently of the launcher's hotkey binding.
    /// Returns whether the session and geometry should be checked on this tick.
    fn update_foreground(&mut self) -> bool {
        if self.checked_at.elapsed() < Duration::from_millis(100) {
            return false;
        }
        self.checked_at = Instant::now();
        self.foreground_dialog = DialogTarget::foreground();
        true
    }

    fn request_search(&mut self, query: String) {
        self.generation += 1;
        // The status deliberately does *not* change here. The list from the
        // previous query stays on screen while the worker probes the filesystem,
        // and flipping the line to "Searching folders…" and back on every
        // keystroke made the hint blink on each character typed or deleted. The
        // status moves when the search *finishes* (see `poll`).
        let recent = self.history.lock().unwrap().clone();
        *self.search_request.lock().unwrap() = Some((self.generation, query, recent));
        let _ = self.search_wake.try_send(());
    }

    /// End the current session. A foreground dialog can attach a new one.
    ///
    /// `navigated` is cleared too: an ordinary cancel (the dialog closing) puts
    /// the picker back in the state where it follows the dialog.
    pub(crate) fn cancel(&mut self) -> Option<DialogTarget> {
        self.cancelled.store(true, Ordering::Release);
        self.generation += 1;
        *self.search_request.lock().unwrap() = None;
        self.navigation = None;
        self.navigating = false;
        self.set_passive(false);
        self.navigated = false;
        self.target.take()
    }

    /// End the session because a path was committed to the dialog.
    ///
    /// The dialog keeps the keyboard and the bar goes away, and — unlike
    /// [`FileContinuum::cancel`] — the session remembers that it navigated, so the
    /// dialog coming straight back to the foreground does not immediately
    /// re-attach a picker to it. Returns the dialog, for the caller to hand focus
    /// back to.
    pub(crate) fn end_after_navigation(&mut self) -> Option<DialogTarget> {
        let target = self.target.take();
        self.cancelled.store(true, Ordering::Release);
        self.generation += 1;
        *self.search_request.lock().unwrap() = None;
        self.navigation = None;
        self.navigating = false;
        self.set_passive(false);
        self.navigated = true;
        target
    }

    /// The bar gained or lost the keyboard.
    ///
    /// Losing it opens the reclaim window (see [`PASSIVE_RECLAIM_WINDOW`]) so a
    /// dialog that activates itself right after the picker attached cannot
    /// leave the query box unusable; gaining it closes the window for good.
    pub(crate) fn set_passive(&mut self, passive: bool) {
        if self.passive == passive {
            return;
        }
        self.passive = passive;
        let now = Instant::now();
        self.passive_until = passive.then(|| now + PASSIVE_RECLAIM_WINDOW);
        self.reclaim_attempts = 0;
        self.reclaim_at = now;
        debug_picker(if passive {
            "bar turned passive: the dialog holds the keyboard"
        } else {
            "bar holds the keyboard"
        });
    }

    /// Whether the poll should re-activate the bar now: the picker is passive
    /// only because a dialog took the keyboard moments ago, the bar is on
    /// screen without it, and a retry is due within the attempt cap.
    fn should_reclaim_keyboard(&self, launcher_visible: bool, launcher_foreground: bool) -> bool {
        let Some(until) = self.passive_until else {
            return false;
        };
        let now = Instant::now();
        self.passive
            && self.target.is_some()
            && !self.navigated
            && launcher_visible
            && !launcher_foreground
            && self.reclaim_attempts < PASSIVE_RECLAIM_LIMIT
            && now < until
            && now >= self.reclaim_at
    }

    /// Record a reclaim attempt. Logged with its count so a dialog that fights
    /// back is visible in the trace instead of looking like a normal recovery.
    pub(crate) fn note_reclaim(&mut self) {
        self.reclaim_attempts += 1;
        let attempt = self.reclaim_attempts;
        self.reclaim_at = Instant::now() + PASSIVE_RECLAIM_RETRY;
        debug_picker(&format!("reclaiming the keyboard (attempt {attempt})"));
    }
}

impl Drop for FileContinuum {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        self.cancelled.store(true, Ordering::Release);
    }
}

fn remember(history: &mut Vec<PathBuf>, path: PathBuf) {
    let key = path.to_string_lossy();
    history.retain(|old| !old.to_string_lossy().eq_ignore_ascii_case(&key));
    history.insert(0, path);
    history.truncate(HISTORY_LIMIT);
}

/// Attach the bar under the dialog that just took the foreground. A dialog
/// seen for the first time hands the keyboard to the bar, so the query goes
/// here instead of into the dialog's own path box; a dialog the picker
/// already attached to (re-shown after Esc, a navigation or a click into the
/// dialog) keeps its focus and the bar waits passively for a click.
fn attach_picker(
    state: &Rc<RefCell<LauncherState>>,
    i18n: Rc<Localization>,
    cx: &mut AsyncApp,
    target: DialogTarget,
) {
    let (prefill, take_keyboard) = {
        let state = state.borrow();
        // The bar is about to be shown, and `LauncherState::height()` is what
        // sizes it. It reads the *previous* session's result count, so without
        // this the bar is placed one full result list too tall (measured:
        // 428px applied, then corrected to 260px once the search answered) and
        // covers the dialog it is anchored to until that correction lands.
        //
        // Only on a *new* session, though: re-attaching to the dialog this
        // session is already on (a passive re-attach, the keyboard reclaim) must
        // not throw away the rows that are on screen, or the bar would collapse
        // to the empty-list height and then grow back as the search re-answered.
        let fresh_session = state.file_continuum.borrow().target != Some(target);
        if fresh_session {
            state.clear_picker_results();
        }
        let mut picker = state.file_continuum.borrow_mut();
        picker.cancel();
        picker.navigated = false;
        let take_keyboard = takes_keyboard(picker.last_target, target);
        picker.last_target = Some(target);
        picker.target = Some(target);
        picker.set_passive(!take_keyboard);
        // The newest folder the user was in, as a starting point. Only a new
        // session seeds the box with it: an existing one keeps the path the user
        // is in the middle of typing.
        let prefill = fresh_session
            .then(|| picker.history.lock().unwrap().first().cloned())
            .flatten()
            .unwrap_or_default();
        (prefill, take_keyboard)
    };
    show_launcher(state, i18n, cx, take_keyboard);
    debug_picker(if take_keyboard {
        "attached and taking the keyboard"
    } else {
        "attached passively"
    });
    // Windows can refuse the focus hand-off (a dialog owned by an elevated
    // process, for example): the bar then falls back to click-to-type.
    if take_keyboard && !launcher_is_foreground(state, cx) {
        state.borrow().file_continuum.borrow_mut().set_passive(true);
    }
    let handle = state
        .borrow()
        .window
        .and_then(|h| h.downcast::<StewardApp>());
    if let Some(handle) = handle {
        let _ = handle.update(cx, |app, window, cx| {
            app.begin_directory_picker(prefill, window, cx);
        });
    }
    sync_directory_picker_bounds(state, cx);
}

/// Whether a dialog that just took the foreground should hand its keyboard to
/// the picker: only the first time the picker sees it. A dialog the picker
/// already attached to is one the user returned to on purpose (Esc, a
/// completed navigation or a click), so it keeps the input.
fn takes_keyboard(last_target: Option<DialogTarget>, target: DialogTarget) -> bool {
    last_target != Some(target)
}

/// Whether the launcher window owns the foreground, i.e. the activation in
/// [`show_launcher`] actually succeeded.
fn launcher_is_foreground(state: &Rc<RefCell<LauncherState>>, cx: &mut AsyncApp) -> bool {
    let handle = state
        .borrow()
        .window
        .and_then(|h| h.downcast::<StewardApp>());
    let launcher_hwnd = handle.and_then(|handle| {
        handle
            .update(cx, |_, window, _| platform::hwnd(window))
            .ok()
            .flatten()
    });
    launcher_hwnd == Some(platform::foreground_hwnd())
}

/// What the poll does with the picker session on this tick.
#[derive(Debug, PartialEq, Eq)]
enum SessionAction {
    /// The dialog just took the foreground: attach the bar to it.
    Attach(DialogTarget),
    /// The dialog is gone, or the user moved on: end the session and hide.
    Detach,
    /// Leave the session alone.
    Keep,
}

/// Everything the session decision looks at, gathered into one value: the call
/// site reads better than seven positional booleans, and the tests can state
/// exactly which field they vary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SessionFacts {
    /// The dialog this session is attached to, if any.
    target: Option<DialogTarget>,
    /// The supported dialog that currently owns the foreground.
    foreground_dialog: Option<DialogTarget>,
    launcher_visible: bool,
    /// The attached dialog is still alive.
    target_valid: bool,
    launcher_foreground: bool,
    navigating: bool,
    /// This session has already put a path into the dialog.
    navigated: bool,
}

/// The picker belongs to the dialog: while a supported dialog is in the
/// foreground the bar is shown, and it goes away only when the dialog does
/// (closed, or no longer a directory picker) or when this session has just
/// navigated it.
///
/// There is deliberately no "the user dismissed it" state. Esc does not close the
/// box and the summon hotkey has nothing to re-open — the box simply stays for as
/// long as the dialog does, which is what the user asked for and is also less
/// state to get wrong. `navigated` is the single exception, and it exists because
/// a completed jump hands the foreground *back* to the dialog: without it the
/// next tick would attach a fresh picker to the dialog it had just navigated.
/// A different dialog is a new session and attaches as usual.
fn session_action(facts: SessionFacts) -> SessionAction {
    let SessionFacts {
        target,
        foreground_dialog,
        launcher_visible,
        target_valid,
        launcher_foreground,
        navigating,
        navigated,
    } = facts;
    match target {
        // The dialog this session was attached to is gone.
        Some(_) if !target_valid => SessionAction::Detach,
        Some(target) if foreground_dialog == Some(target) => {
            if navigated || launcher_visible || navigating {
                SessionAction::Keep
            } else {
                SessionAction::Attach(target)
            }
        }
        // Still attached, but the dialog is not in front and the bar is not
        // either: the user moved to another application, so let it go.
        Some(_) if launcher_visible && launcher_foreground => SessionAction::Keep,
        Some(_) => SessionAction::Detach,
        // No session: follow the dialog that is in front, unless this session
        // already navigated it (see the doc comment above).
        None if !launcher_visible => match foreground_dialog {
            Some(_) if navigated => SessionAction::Keep,
            Some(dialog) => SessionAction::Attach(dialog),
            None => SessionAction::Keep,
        },
        None => SessionAction::Keep,
    }
}

/// Called by the foreground event pump; all work here is nonblocking.
pub(crate) fn poll(state: &Rc<RefCell<LauncherState>>, i18n: Rc<Localization>, cx: &mut AsyncApp) {
    let (mut paths, mut navigation, foreground_checked) = {
        let state = state.borrow();
        let mut picker = state.file_continuum.borrow_mut();
        let foreground_checked = picker.update_foreground();
        let mut paths = None;
        while let Ok((generation, result)) = picker.search_results.try_recv() {
            if picker.target.is_some() && generation == picker.generation {
                // A non-empty result list has nothing to report: the line stays
                // hidden and the results speak for themselves. Only an empty list
                // says anything.
                picker.status = if result.is_empty() {
                    "file-continuum-no-results".to_owned()
                } else {
                    String::new()
                };
                paths = Some(result);
            } else {
                // Dropped: either the session ended, or a newer query was
                // already requested. Both leave the rows on screen stale, which
                // is exactly what the confirmation gate then reports.
                debug_picker(&format!(
                    "picker reply {generation} dropped (current={}, attached={})",
                    picker.generation,
                    picker.target.is_some()
                ));
            }
        }
        let navigation = picker.navigation.as_ref().and_then(|rx| rx.try_recv().ok());
        if navigation.is_some() {
            picker.navigation = None;
        }
        (paths, navigation, foreground_checked)
    };
    let handle = state
        .borrow()
        .window
        .and_then(|h| h.downcast::<StewardApp>());
    if foreground_checked {
        let launcher_hwnd = handle.and_then(|handle| {
            handle
                .update(cx, |_, window, _| platform::hwnd(window))
                .ok()
                .flatten()
        });
        let visible = launcher_hwnd.is_some_and(platform::is_hwnd_visible);
        let launcher_foreground = launcher_hwnd == Some(platform::foreground_hwnd());
        let (action, reclaim) = {
            let state = state.borrow();
            let picker = state.file_continuum.borrow();
            (
                session_action(SessionFacts {
                    target: picker.target,
                    foreground_dialog: picker.foreground_dialog,
                    launcher_visible: visible,
                    target_valid: picker.target.is_some_and(|target| target.valid()),
                    launcher_foreground,
                    navigating: picker.navigating,
                    navigated: picker.navigated,
                }),
                picker.should_reclaim_keyboard(visible, launcher_foreground),
            )
        };
        match action {
            SessionAction::Attach(target) => {
                paths = None;
                navigation = None;
                attach_picker(state, i18n.clone(), cx, target);
            }
            SessionAction::Detach => {
                paths = None;
                navigation = None;
                if let Some(handle) = handle {
                    let _ = handle.update(cx, |app, window, cx| {
                        app.cancel_directory_picker(window, cx, false);
                        hide_window(window, cx);
                    });
                } else {
                    state.borrow().file_continuum.borrow_mut().cancel();
                }
            }
            SessionAction::Keep => {
                if state.borrow().file_continuum.borrow().target.is_some() {
                    sync_directory_picker_bounds(state, cx);
                    // A dialog that activated itself right after the picker
                    // attached must not leave the bar keyboard-less: take the
                    // foreground back while the reclaim window is open. The
                    // input keeps its text — unlike `attach_picker` this does
                    // not restart the session, so nothing is re-searched.
                    if reclaim {
                        state.borrow().file_continuum.borrow_mut().note_reclaim();
                        show_launcher(state, i18n.clone(), cx, true);
                    }
                }
            }
        }
    }
    if let (Some(handle), Some(paths)) = (handle, paths) {
        let _ = handle.update(cx, |app, window, cx| {
            app.apply_directory_results(paths, window, cx)
        });
    }
    if let Some(result) = navigation {
        match result {
            Ok(()) => {
                // The jump landed: take the box off screen and hand the keyboard
                // back to the dialog. The session is marked as navigated, which
                // is what keeps the dialog's own foreground from attaching a
                // fresh picker a moment later — the box comes back when the
                // dialog *changes*, not when this one re-focuses.
                let target = state
                    .borrow()
                    .file_continuum
                    .borrow_mut()
                    .end_after_navigation();
                if let Some(handle) = handle {
                    let _ = handle.update(cx, |app, window, cx| {
                        // Leave the picker's own box empty rather than holding
                        // the path just committed: the next session starts
                        // clean. The ordinary launcher's box was never touched
                        // by this session, and re-running its query puts its
                        // rows back under it (the picker's rows replaced them
                        // while the session was attached).
                        app.picker_input.query.clear();
                        app.picker_input.marked = None;
                        app.picker_input.set_cursor(0);
                        app.search(window, cx);
                        hide_window(window, cx);
                    });
                }
                if let Some(target) = target {
                    target.restore_focus();
                }
            }
            Err(error) => {
                {
                    let state = state.borrow();
                    let mut picker = state.file_continuum.borrow_mut();
                    picker.navigating = false;
                    picker.status = error;
                }
                // Return the editable query with a visible, localized error.
                // Do not steal focus if the user has already switched away.
                let target = state.borrow().file_continuum.borrow().target;
                if target.is_some() && target == DialogTarget::foreground() {
                    show_launcher(state, i18n, cx, true);
                }
                if let Some(handle) = handle {
                    let _ = handle.update(cx, |_, _, cx| cx.notify());
                }
            }
        }
    }
}

/// Opt-in diagnostics for the picker's focus, session and placement decisions.
///
/// Off unless `STEWARD_FILE_CONTINUUM_DEBUG` is set, and written both to stderr
/// (visible from a `cargo run` session) and to
/// `%TEMP%\steward-file-continuum.log` (visible when the shipped
/// `steward-app.exe` was double-clicked and has no console). The picker's
/// behaviour depends on which window Windows hands the foreground to and when,
/// plus the dialog's live rectangle — none of it observable from the outside,
/// and all of it needed to tell "the bar never took the keyboard" from "the bar
/// took it and lost it again".
///
/// Start the picker trace, when tracing is on.
///
/// The log is **truncated** here and stamped with the process id, so a log
/// always belongs to exactly one run: `Get-Content` can no longer show a stale
/// session's lines side by side with the current one (which is what made an
/// already-fixed 428px placement look like it was still happening).
pub(crate) fn debug_start() {
    if !debug_enabled() {
        return;
    }
    let banner = format!(
        "--- steward file-continuum trace, pid {} ---",
        std::process::id()
    );
    eprintln!("[file-continuum] {banner}");
    if let Some(path) = debug_log_path() {
        let _ = std::fs::write(path, format!("{banner}\n"));
    }
}

fn debug_log_path() -> Option<std::path::PathBuf> {
    Some(std::env::temp_dir().join("steward-file-continuum.log"))
}

fn debug_picker(message: &str) {
    if !debug_enabled() {
        return;
    }
    eprintln!("[file-continuum] {message}");
    if let Some(path) = debug_log_path() {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            use std::io::Write as _;
            let _ = writeln!(file, "{message}");
        }
    }
}

/// Whether the picker's geometry tracing is on (see [`debug_picker`]).
pub(crate) fn debug_enabled() -> bool {
    std::env::var_os("STEWARD_FILE_CONTINUUM_DEBUG").is_some()
}

/// Trace one line, when [`debug_enabled`]. Used by the window/placement code,
/// which has no access to the picker's internals but does know the numbers the
/// bar was placed with.
pub(crate) fn debug_log(message: &str) {
    debug_picker(message);
}

/// Trace the bar's own rectangle next to the dialog's, right after a placement.
/// This is the pair that answers "the bar is in the wrong place": whether the
/// anchor was missing, stale, or simply never applied. No-op unless tracing is
/// on.
#[cfg(target_os = "windows")]
pub(crate) fn debug_placement(window: &gpui::Window, state: &Rc<RefCell<LauncherState>>) {
    if !debug_enabled() {
        return;
    }
    let bar = platform::hwnd(window).map(platform::window_rect);
    let dialog = state.borrow().dialog_anchor();
    debug_log(&format!("placed: bar={bar:?} dialog={dialog:?}"));
}

pub(crate) fn confirm_directory(state: &Rc<RefCell<LauncherState>>, path: PathBuf, cx: &mut App) {
    let (target, cancelled, tx) = {
        let state = state.borrow();
        let mut picker = state.file_continuum.borrow_mut();
        // A retired session has nothing to drive: the next result-row confirm
        // after a jump must not inject another Alt+D into the dialog.
        if picker.navigating || picker.navigated {
            return;
        }
        let Some(target) = picker.target else {
            return;
        };
        let (tx, rx) = bounded(1);
        picker.cancelled = Arc::new(AtomicBool::new(false));
        picker.navigation = Some(rx);
        picker.navigating = true;
        picker.status = "file-continuum-navigating".into();
        (target, picker.cancelled.clone(), tx)
    };
    // Leave the current result-list callback before focus/activation callbacks
    // re-enter the launcher. Keep native I/O out of GPUI's foreground task.
    cx.defer(move |_| {
        if cancelled.load(Ordering::Acquire) {
            return;
        }
        if let Err(error) = target.focus_address() {
            let _ = tx.send(Err(error.to_string()));
            return;
        }
        std::thread::spawn(move || {
            let result = target
                .navigate(&path, &cancelled)
                .map_err(|error| error.to_string());
            let _ = tx.send(result);
        });
    });
}

impl StewardApp {
    /// Clicking a bar that does not hold the keyboard hands it the input.
    /// Returns whether the picker needed promoting.
    pub(crate) fn promote_passive_picker(&mut self) -> bool {
        #[cfg(target_os = "windows")]
        {
            let state = self.state.borrow();
            let mut picker = state.file_continuum.borrow_mut();
            if !picker.passive {
                return false;
            }
            picker.set_passive(false);
            picker.status = String::new();
            true
        }
        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    }

    fn begin_directory_picker(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The prefill goes into the picker's own box: the ordinary launcher's
        // query is left exactly as the user typed it.
        self.picker_input.query = path.to_string_lossy().into_owned();
        self.picker_input.marked = None;
        self.picker_input.select_all();
        self.mouse_selecting = false;
        self.search(window, cx);
    }

    pub(crate) fn search_directory_picker(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        {
            let state = self.state.borrow();
            state.plugin_gen.set(state.plugin_gen.get() + 1);
            state.plugin_hits.borrow_mut().clear();
            state.plugin_views.borrow_mut().clear();
            state.plugin_pending.borrow_mut().clear();
            *state.plugin_calendar.borrow_mut() = None;
            state
                .file_continuum
                .borrow_mut()
                .request_search(self.picker_input.query.clone());
            debug_picker(&format!(
                "picker search requested for {:?}",
                self.picker_input.query
            ));
        }
        // The rows already on screen were not produced for this query, so they
        // must not be actionable until the reply lands and re-renders them. They
        // keep showing while the search runs — blanking them was what made the
        // drop-down flash on every keystroke — and `render_merged` re-enables
        // confirmation for the query it renders.
        let stale = self.results_query != self.picker_input.query;
        self.results.set_confirmable(!stale, cx);
        cx.notify();
    }

    fn apply_directory_results(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Directory candidates carry no match rank: the picker's own matcher
        // already ordered them by relevance, so they keep that order (every row
        // gets the same rank and the sort is stable). `render_merged` re-enables
        // confirmation for the query it renders.
        //
        // The row shows the folder's **whole path** and nothing else: the path is
        // what the picker navigates to, so showing a name and a parent that have
        // to be reassembled by eye only costs a column. A path too long for the
        // row is truncated at its end (the tail is also what `search.rs` matches
        // last), so the tail is what a long path loses first — not the drive and
        // top-level folders the user is navigating through.
        self.base_rows = paths
            .into_iter()
            .map(|path| RankedRow {
                item: ResultItem::Directory {
                    title: path.to_string_lossy().into_owned(),
                    subtitle: String::new(),
                    path,
                },
                icon: None,
                rank: RowRank {
                    tier: MatchTier::NameSubstring,
                    relevance: 0,
                    name_len: 0,
                    path: Vec::new(),
                },
            })
            .collect();
        self.builtin_count = 0;
        self.ranked_query = self.picker_input.query.clone();
        // The picker's own reply is what produced these rows, and it is the
        // answer to the picker box as it stands right now — the file index
        // plays no part in a picker session. Recording that here is what makes
        // the rows confirmable; `render_merged` only re-enables confirmation
        // for a query a producer vouched for.
        self.results_query = self.picker_input.query.clone();
        let generation = self.state.borrow().file_search_generation;
        debug_picker(&format!(
            "picker reply applied: rows={} query={:?}",
            self.base_rows.len(),
            self.picker_input.query
        ));
        self.render_merged(generation, window, cx);
    }

    /// End the picker session (the dialog is gone, or the user moved on).
    ///
    /// The picker's own box only ever belongs to the session that is ending,
    /// and the next session seeds its own prefill. The ordinary launcher's box
    /// was never touched, so its query is re-run here to put its rows back:
    /// the picker's folder rows replaced them while the session was attached.
    pub(crate) fn cancel_directory_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        restore: bool,
    ) {
        let target = self.state.borrow().file_continuum.borrow_mut().cancel();
        if target.is_some() {
            self.search(window, cx);
        }
        if restore {
            hide_window(window, cx);
            if let Some(target) = target {
                target.restore_focus();
            }
        }
        cx.notify();
    }

    /// Esc while the picker is attached does nothing.
    ///
    /// The picker belongs to the dialog: it appears with it and goes away with
    /// it (or when a path is committed). Closing it on Esc only produced a box
    /// that came straight back — the dialog is still in front, so the very next
    /// poll re-attached it — and then the user had to re-summon it by hotkey to
    /// get back to the box they had just closed. Removing both halves of that
    /// loop is the point of this change, so Esc is deliberately inert here; the
    /// dialog's own Cancel (or Esc into the dialog) ends the session by closing
    /// the dialog.
    pub(crate) fn picker_ignores_escape(&mut self) {
        debug_picker("Esc ignored: the picker stays for as long as the dialog does");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_is_recent_first_deduplicated_and_bounded() {
        let mut history = Vec::new();
        for i in 0..70 {
            remember(&mut history, PathBuf::from(format!(r"C:\folder{i}")));
        }
        assert_eq!(history.len(), HISTORY_LIMIT);
        remember(&mut history, PathBuf::from(r"c:\FOLDER65"));
        assert_eq!(history.len(), HISTORY_LIMIT);
        assert_eq!(history[0], PathBuf::from(r"c:\FOLDER65"));
        assert_eq!(history[1], PathBuf::from(r"C:\folder69"));
    }

    #[test]
    fn only_the_first_attach_to_a_dialog_takes_the_keyboard() {
        let dialog = DialogTarget::for_test(0x1000);
        let other = DialogTarget::for_test(0x2000);
        assert!(takes_keyboard(None, dialog));
        assert!(!takes_keyboard(Some(dialog), dialog));
        assert!(takes_keyboard(Some(other), dialog));
    }

    #[test]
    fn a_just_passive_session_reclaims_the_keyboard_but_a_settled_one_does_not() {
        let dialog = DialogTarget::for_test(0x1000);
        let mut picker = FileContinuum::new();
        picker.target = Some(dialog);
        picker.set_passive(true);
        // The bar is on screen, the dialog holds the keyboard: this is the
        // freshly-attached case the reclaim exists for.
        assert!(picker.should_reclaim_keyboard(true, false));
        // Nothing to reclaim while the bar already has the keyboard, while it is
        // hidden, or once the session has navigated the dialog and let it go.
        assert!(!picker.should_reclaim_keyboard(true, true));
        assert!(!picker.should_reclaim_keyboard(false, false));
        picker.navigated = true;
        assert!(!picker.should_reclaim_keyboard(true, false));
        picker.navigated = false;
        // Past the window the picker stays passive on purpose: the user is
        // expected in the dialog by then, and the bar waits for a click.
        picker.passive_until = Some(Instant::now() - Duration::from_millis(1));
        assert!(!picker.should_reclaim_keyboard(true, false));
        // A session that still holds the keyboard never reclaims anything.
        picker.set_passive(false);
        assert!(!picker.should_reclaim_keyboard(true, false));
    }

    /// Facts with everything neutral; each test varies only what it is about.
    fn facts() -> SessionFacts {
        SessionFacts {
            target: None,
            foreground_dialog: None,
            launcher_visible: false,
            target_valid: true,
            launcher_foreground: false,
            navigating: false,
            navigated: false,
        }
    }

    #[test]
    fn foreground_dialog_always_reattaches_after_cancellation_or_hidden_window() {
        let dialog = DialogTarget::for_test(0x1000);
        assert_eq!(
            session_action(SessionFacts {
                foreground_dialog: Some(dialog),
                ..facts()
            }),
            SessionAction::Attach(dialog)
        );
        assert_eq!(
            session_action(SessionFacts {
                target: Some(dialog),
                foreground_dialog: Some(dialog),
                ..facts()
            }),
            SessionAction::Attach(dialog)
        );
        // An ordinary visible launcher still belongs to its current session.
        assert_eq!(
            session_action(SessionFacts {
                foreground_dialog: Some(dialog),
                launcher_visible: true,
                ..facts()
            }),
            SessionAction::Keep
        );
    }

    #[test]
    fn attached_session_follows_focus_and_dialog_lifetime() {
        let dialog = DialogTarget::for_test(0x1000);
        // Clicking the picker and then its dialog keeps the same attachment.
        assert_eq!(
            session_action(SessionFacts {
                target: Some(dialog),
                launcher_visible: true,
                launcher_foreground: true,
                ..facts()
            }),
            SessionAction::Keep
        );
        assert_eq!(
            session_action(SessionFacts {
                target: Some(dialog),
                foreground_dialog: Some(dialog),
                launcher_visible: true,
                ..facts()
            }),
            SessionAction::Keep
        );
        // Navigation may hide the picker while it sends input to the dialog.
        assert_eq!(
            session_action(SessionFacts {
                target: Some(dialog),
                foreground_dialog: Some(dialog),
                navigating: true,
                ..facts()
            }),
            SessionAction::Keep
        );
        // Closing the dialog also ends a session whose picker owns focus.
        assert_eq!(
            session_action(SessionFacts {
                target: Some(dialog),
                launcher_visible: true,
                ..facts()
            }),
            SessionAction::Detach
        );
        // A dead target detaches even while the bar is up and focused.
        assert_eq!(
            session_action(SessionFacts {
                target: Some(dialog),
                target_valid: false,
                launcher_visible: true,
                launcher_foreground: true,
                ..facts()
            }),
            SessionAction::Detach
        );
    }

    #[test]
    fn a_navigated_session_carries_on_for_the_new_dialog() {
        let dialog = DialogTarget::for_test(0x1000);
        let other = DialogTarget::for_test(0x2000);
        // Just after a jump: the bar is gone and the dialog has the keyboard
        // back. The picker must stay away (this is the bug where committing a
        // path put the box straight back on screen).
        assert_eq!(
            session_action(SessionFacts {
                foreground_dialog: Some(dialog),
                navigated: true,
                ..facts()
            }),
            SessionAction::Keep
        );
        // The next time the user opens a dialog — a different window — it gets a
        // picker again, because a fresh session clears the flag.
        assert_eq!(
            session_action(SessionFacts {
                foreground_dialog: Some(other),
                navigated: false,
                ..facts()
            }),
            SessionAction::Attach(other)
        );
    }

    #[test]
    fn the_picker_is_shown_for_as_long_as_its_dialog_is() {
        let dialog = DialogTarget::for_test(0x1000);
        // Hidden bar, dialog in front, session still attached: show it again.
        // There is no "the user closed it" state to disagree with this — that is
        // the behaviour, not a fallback.
        assert_eq!(
            session_action(SessionFacts {
                target: Some(dialog),
                foreground_dialog: Some(dialog),
                ..facts()
            }),
            SessionAction::Attach(dialog)
        );
        // The same holds for a session that has not attached yet.
        assert_eq!(
            session_action(SessionFacts {
                foreground_dialog: Some(dialog),
                ..facts()
            }),
            SessionAction::Attach(dialog)
        );
    }
}
