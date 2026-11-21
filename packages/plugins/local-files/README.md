# Local File Inspector

Official PC-2 validation plugin for Steward's generic filesystem and system-open
APIs.

- `pickFolder(false)` selects a read-only scan root.
- `walk` returns bounded metadata without descending through reparse points.
- `pickFile` plus `readText` previews bounded UTF-8 text.
- `installedApplications` plus `openWith` opens the first result with a catalog
  application.
- `openPath` and `revealPath` use the same selected-root grant.

Run `nsh test .` to exercise the host calls without launching the desktop app.
