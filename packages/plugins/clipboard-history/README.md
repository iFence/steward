# Clipboard History

Official NSH TSX plugin for browsing Steward's clipboard history. It reads the
host-owned snapshot through `@steward/extension-api`, copies an entry back to
the clipboard, and persists pinned entry IDs in NSH plugin storage.

Development:

```powershell
nsh check App.tsx
nsh test .
nsh build App.tsx --release -o dist
```

The host grants only `steward.clipboard.history` and clipboard access. Plugins
never open Steward's SQLite database directly.
