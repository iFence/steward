#!/usr/bin/env bash
# Extract the release notes for a specific version from a changelog file.
#
# The changelog format is defined at the top of `Changelog.md`:
#   - Version sections start with `## vX.Y.Z` (leading `v`/`V` optional).
#   - A `---` separator or the next `## vX.Y.Z` header ends the section.
#
# The release workflow publishes the extracted section as the GitHub Release
# body, so a tag without a matching section is an error rather than an empty
# release.
#
# Usage: ./scripts/extract-release-notes.sh <tag> [changelog-path]
#   tag             e.g. "v0.1.0" or "0.1.0"
#   changelog-path  default "Changelog.md"
#   Prints the matched section body to stdout.
set -euo pipefail

TAG="${1:-}"
CHANGELOG="${2:-Changelog.md}"

if [ -z "$TAG" ]; then
  echo "Usage: $0 <tag> [changelog-path]" >&2
  exit 1
fi

if [ ! -f "$CHANGELOG" ]; then
  echo "Changelog not found: $CHANGELOG" >&2
  exit 1
fi

# Strip a leading 'v' so tags and changelog headers match regardless of prefix.
VERSION="${TAG#v}"

NOTES="$(awk -v ver="$VERSION" '
  BEGIN { found = 0 }
  /^## / {
    if (found) exit
    token = substr($0, 4)
    sub(/ .*$/, "", token)
    gsub(/^[vV]/, "", token)
    if (token == ver) { found = 1; next }
  }
  found && /^---$/ { exit }
  found { print }
' "$CHANGELOG")"

if [ -z "${NOTES//[[:space:]]/}" ]; then
  echo "No changelog section for version '$VERSION' in $CHANGELOG" >&2
  exit 1
fi

printf '%s\n' "$NOTES"
