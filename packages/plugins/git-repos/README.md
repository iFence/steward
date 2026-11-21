# Git Repositories

Official PC-2 reference workload for generic local-data and system-open APIs.
Steward contains no Git-specific host service: this plugin selects a root,
finds `.git` entries with `walk`, reads `.git/config` with `readText`, filters
and favorites repository paths, and uses host-owned open/reveal/URL actions.

Clone, status, remote inspection, and unpushed-commit checks use Steward's
generic controlled execution API. The plugin declares the exact `git.exe`
target and passes arguments as an array; commands run in a user-selected write
root through the launcher-owned broker. Safe removal remains deferred to later
host-owned destructive filesystem work.
