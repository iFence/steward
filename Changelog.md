# Changelog

This file tracks the user-facing release notes for each version.

Conventions:
- Each version uses `## vX.Y.Z` as the heading, e.g. `## v0.1.0`
- The content below the heading is what appears in the GitHub Release body and the in-app update dialog
- End each version section with `---` or by starting the next version heading

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
