# Git Repositories

Official PC-2 reference workload for generic local-data and system-open APIs.
Steward contains no Git-specific host service: this plugin selects a root,
finds `.git` entries with `walk`, reads `.git/config` with `readText`, filters
and favorites repository paths, and uses host-owned open/reveal/URL actions.

Clone, status, unpushed-commit checks, and safe removal are intentionally
deferred to PC-3 controlled process execution and host-owned destructive
operations.
