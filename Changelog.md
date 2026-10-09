# Changelog

This file tracks the user-facing release notes for each version.

Conventions:
- Each version uses `## vX.Y.Z` as the heading, e.g. `## v0.1.0`
- The content below the heading is what appears in the GitHub Release body and the in-app update dialog
- End each version section with `---` or by starting the next version heading

## v0.2.0

### ✨ New Features
- **Virtual UI Trees**: plugins can return a `{ "type": "ui" }` view — a serializable element tree built with a React-style builder (`div()`, `col()`, `text()`, `button()`, `input()`, …). The host validates the tree and renders it with the same gpui components as the launcher, so plugins get real layout and styling without running any UI code and without leaving the plugin process.
- **Interactive elements**: buttons and links deliver `click`, inputs deliver `change` and `submit`, and a handler that returns a new tree redraws the view. Input text is owned by the host, so typing stays responsive even when a plugin is slow.
- **UI Showcase plugin**: an official example plugin (`uishowcase`) demonstrating layout containers, styled text, a click handler and a host-owned input, with the view poppable into its own window.
- **Dockable plugin workspace**: every plugin view (list, detail, form, grid, search, calendar and `ui` trees) opens into a workspace window where panels are tabbed, dragged, split and closed; the arrangement is saved and restored on the next launch.
- **Toasts**: plugin `showToast` messages now appear as transient toasts instead of being written to the log.
- **File search on its own page**: the launcher now has two levels. The main list is applications and commands only, and typing `fs` (or the command's full name, `file search` / `文件搜索`) offers a built-in **File Search** command that opens a second level searching the full-disk index. On that page the box searches files only, `fs report` drills in already searching `report`, and Esc (or the back control) returns to the main list. The old `file:` / `f:` / `app:` prefixes are gone — they are plain query text now.

### 🐛 Bug Fixes

- **A clean box on every summon**: the launcher kept whatever was typed during the previous visit, so after clicking away - or clicking a result to launch it - the old query was still sitting in the box the next time the hotkey summoned the bar. Hiding the launcher now clears the query and returns to the main list, so every summon opens on an empty box, with the most-used applications below it.
- **Calendar polish**: the selected day is drawn as a rectangle instead of a stretched oval, and the dockable plugin workspace no longer titles itself `No localization for id: "app-name"` or labels its tab `steward.plugin` - the window is titled Steward and the tab shows the plugin's own title.
- **Calendar navigation icons**: the month and year steps in the calendar panel are Lucide chevrons instead of `<<` `<` `>` `>>` text - a single chevron moves the month and a double one moves the year, so the pair reads as month/year at a glance.
- **Tags sit on the right of every row**: an application row's "Application" tag and a command row's "Command" tag were left-aligned inside the 280px secondary column, so a short label floated in the middle of the row. The secondary and size columns now align their text to the right, which puts the tags (and a file row's size) flush with the row's right edge while the columns stay lined up down the list.
- **Calendar controls reworked**: the month and year buttons are rounded rectangles instead of pills, the calendar header no longer carries a pushpin (a detachable calendar now opens in its own window through the launcher's shared pop-out control), and the selected day is marked with a theme-colored border around its box.
- **One panel, one tab**: the workspace used to dock a second copy of a command it had already restored from the saved layout (a calendar came up as two "Calendar" tabs), and a lone panel drew its title above its own content as well. Restored panels are now registered like freshly opened ones, duplicate dockings are dropped from a layout that already held them, and a panel alone in its group draws no title row - the calendar fills the window from the top.
- **No more crash on a plugin's text box**: the first `ui` tree a plugin drew that contained an input panicked with "RefCell already borrowed" (the registration read the input map and then wrote to it while the read borrow was still alive), which closed the launcher the moment a query routed to a plugin like UI Showcase. The entry is now read out before the map is written, so a plugin's input renders and takes typing as it should.
- **No more file-dialog popups**: Steward no longer attaches a directory picker under other applications' open/save dialogs. Opening a folder from a third-party app keeps that app's dialog in front, and the launcher only appears when you summon it with the hotkey or the tray icon.
- **The binary reports its real version**: `steward-app.exe` carried a hard-coded `0.1.0` version resource, so Explorer's Details tab, Task Manager and the installer's file table disagreed with the workspace (0.2.0). The resource is now generated from the package version during the build, so every place that reads a version reports the same one.
- **A silent installer**: installing or upgrading no longer flashes black console windows. The installer still stops a running Steward before its files are replaced, but it now hands the command to WiX's quiet launcher instead of letting the custom action spawn `taskkill` with a visible console (three times, six on a major upgrade).

---

## v0.1.0

Steward's first release: an instant, low-memory launcher for Windows with a process-isolated plugin platform.

### ✨ New Features
- **Summon instantly, stay out of the way**: Steward starts silently in the system tray; `Ctrl+Alt+Space` (or a tray click) brings up the launcher bar and `Esc` hides it again. The process keeps running, so the next summon is immediate.
- **Application search**: apps from the Start menu are indexed with their real shell icons and matched with a fuzzy matcher, so partial or out-of-order queries still find the right entry.
- **Full-disk file search**: an NTFS index built from the `$MFT` and the USN journal searches files across every volume, and keeps tracking installs, removals and renames while Steward runs.
- **Built-in calculator**: type an arithmetic expression and the result appears inline, ready to copy.
- **Links open in your browser**: a pasted URL is recognized as a result and opened with the system default browser.
- **Configurable hotkeys**: record a new global summon hotkey and a settings hotkey from the settings window; if a binding is already taken, Steward falls back to the default instead of failing to start.
- **Plugin platform**: TypeScript plugins run in a QuickJS runtime inside a separate process, speak JSON-RPC over a Windows named pipe, and declare permissions (filesystem, clipboard, network) that the host enforces. Plugins contribute declarative views (list, detail, form, grid, action panel, search bar) and can use a Node-compatible polyfill layer for the common standard modules.
- **Official plugins**: Calendar (with lunar dates, solar terms and festivals, following the app accent color) and Clipboard History.
- **Settings**: language (English, 中文, 日本語, 한국어, Deutsch, Français, Русский), theme accent color, launch at startup, and hotkey recording.

### 🚀 Improvements
- **Frosted-glass surfaces**: the launcher draws a dark, blurred bar and adapts its scrim to whatever is behind it, so white text stays readable over bright windows; the selected-row highlight follows the same curve.
- **Consistent theming**: the settings window, plugin panels and built-in plugin views pick up the accent color you choose at runtime instead of hard-coded colors.
- **Native Windows polish**: per-monitor DPI awareness, dark native tray menu and title bar, a borderless draggable launcher bar, and no console window in debug or release builds.
- **Fast by design**: GPUI, the app index and the plugin metadata cache load at startup so summoning the bar never waits on a scan; heavier capabilities stay behind the plugin process boundary.

### 🐛 Bug Fixes
- **Stable memory while the file index runs**: the full-disk index no longer copies itself on every background refresh — catch-up updates it in place, the index-to-file-id map is only built while a change batch needs it, and snapshots stream straight into the database. A machine with a million files no longer sees large periodic memory spikes after startup.

---
