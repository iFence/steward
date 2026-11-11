# Steward Plugin Manifest v2

> Status: implemented by the NSH migration.

`plugin.json` describes only Steward launcher routing. Executable code,
capabilities, assets and UI come from the signed `plugin.nshpkg` referenced by
the manifest.

```json
{
  "schemaVersion": 2,
  "id": "it.aspirin.calendar",
  "name": "Calendar",
  "version": "0.1.4",
  "package": "plugin.nshpkg",
  "commands": [
    {
      "name": "calendar",
      "title": "Calendar",
      "trigger": { "type": "command" }
    }
  ]
}
```

Fields:

| Field | Type | Required | Meaning |
|---|---|---|---|
| `schemaVersion` | number | yes | Must be `2`. |
| `id` | string | yes | Reverse-domain id; must match `nsh.json`. |
| `name` | string | yes | Display name. |
| `version` | string | yes | Must match the package app version. |
| `icon` | string | no | Inline SVG launcher icon. |
| `package` | string | yes | Must be `plugin.nshpkg`. |
| `commands` | array | yes | At least one command. |

Commands contain `name`, `title`, optional `keywords`, and `trigger`.
Supported triggers are `command`, `prefix`, and `regex`.

Installation layout:

```text
<plugin dir>/
  plugin.json
  plugin.nshpkg
```

Old `permissions`, `fs_roots`, `http`, `isolation`, `detachable`, and JSON View
fields are rejected.
