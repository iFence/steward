# Steward Plugin Manifest v2

> Status: implemented by the NSH migration.

`plugin.json` describes Steward launcher routing and optional host-rendered
preference metadata. Executable code, capabilities, assets, and UI come from
the signed `plugin.nshpkg` referenced by the manifest.

```json
{
  "schemaVersion": 2,
  "id": "it.aspirin.calendar",
  "name": "Calendar",
  "version": "0.1.4",
  "minHostVersion": "0.3.0",
  "package": "plugin.nshpkg",
  "preferences": {
    "schemaVersion": 1,
    "fields": [
      {
        "key": "theme",
        "title": "Theme",
        "type": "select",
        "default": "system",
        "options": [
          { "value": "system", "label": "System" },
          { "value": "dark", "label": "Dark" }
        ]
      }
    ]
  },
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

| Field            | Type   | Required    | Meaning                                                                                                                |
| ---------------- | ------ | ----------- | ---------------------------------------------------------------------------------------------------------------------- |
| `schemaVersion`  | number | yes         | Must be `2`.                                                                                                           |
| `id`             | string | yes         | Reverse-domain id; must match `nsh.json`.                                                                              |
| `name`           | string | yes         | Display name.                                                                                                          |
| `version`        | string | yes         | Must match the package app version.                                                                                    |
| `minHostVersion` | string | third-party | Minimum semantic Steward plugin host API version. Required for new non-official packages.                              |
| `icon`           | string | recommended | Inline SVG launcher icon. Official plugins must provide it; older manifests receive Steward's generic plugin fallback. |
| `package`        | string | yes         | Must be `plugin.nshpkg`.                                                                                               |
| `preferences`    | object | no          | Host-rendered preference schema. It is read without loading the plugin VM.                                             |
| `commands`       | array  | yes         | At least one command.                                                                                                  |

Commands contain `name`, `title`, optional `keywords`, and `trigger`.
Supported triggers are `command`, `prefix`, and `regex`.

## Preferences

`preferences.schemaVersion` is currently `1`. Up to 64 unique fields may be
declared. Supported `type` values are:

| Type          | Value shape                    |
| ------------- | ------------------------------ |
| `text`        | string                         |
| `boolean`     | boolean                        |
| `select`      | one declared option value      |
| `folder`      | absolute local directory path  |
| `application` | absolute local executable path |

Each field requires a stable lowercase `key` and a `title`. Optional properties
are `description`, `default`, `required`, `maxLength`, and `options`.
`maxLength` applies to text and path fields and is capped at 4096 bytes.
Relative paths, UNC paths, NUL characters, and `..` components are rejected.
Folder and application host access remains governed by the filesystem and
system-open capabilities; storing the preference alone does not grant file
access or an allowed launch target.

Stored values are keyed by plugin ID and survive updates. If a schema renames a
key, declare an idempotent migration:

```json
{
  "migrations": [{ "from": "oldTheme", "to": "theme" }]
}
```

The migration source must be retired and the destination must be one of the
declared fields. Stored values that no longer pass the current schema are
replaced by the current default or omitted when a required field has no valid
value.

Installation layout:

```text
<plugin dir>/
  plugin.json
  plugin.nshpkg
```

Old `permissions`, `fs_roots`, `http`, `isolation`, `detachable`, and JSON View
fields are rejected.

The current host API is `0.3.0`. Existing official packages may omit
`minHostVersion` during migration. See
[plugin-host-api-compatibility.md](plugin-host-api-compatibility.md).
