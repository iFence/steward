# Clipboard History

Official NSH TSX plugin for browsing Steward's clipboard history. It opens as
an embedded launcher page instead of a separate window, reads the host-owned
snapshot through `@steward/extension-api`, copies an entry back to the
clipboard, and persists pinned entry IDs in plugin storage.

The page supports incremental search, pinned-first ordering, keyboard
navigation, visibility refreshes, the current Steward theme, localized copy,
and Back/Escape navigation. The plugin remains in its isolated host process.

Development:

```powershell
nsh check App.tsx
nsh test .
nsh build App.tsx --release -o dist
```

The host grants only clipboard history and write access, page navigation, and
plugin-private storage. Plugins never open Steward's SQLite database directly.
