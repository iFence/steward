# Calendar (official example plugin)

The M2 end-to-end example: TypeScript -> esbuild single-file IIFE -> QuickJS ->
launcher UI. Typing `calendar` (or a partial/fuzzy form like `cal`, `cldr` or
`CAL`) opens a real month calendar view; the manifest also declares the
localized keyword `日历`, so Chinese input (or its pinyin `rili` / `rl`)
matches too. The month can be shifted with `calendar +3` / `calendar -1` or
set absolutely with `calendar 2026-09`. Arrow keys move the selected day,
Enter or a click copies the ISO date to the clipboard and shows a toast.

```text
packages/plugins/calendar/
├── plugin.json          # manifest scanned by steward-plugin-registry
├── src/index.ts         # plugin source (imports @steward/extension-api)
└── dist/index.js        # esbuild IIFE bundle, loaded by the QuickJS runtime
```

Build: `pnpm --filter @steward/plugin-calendar build`.

To run the app against this repo's plugin instead of the installed copy, point
`STEWARD_PLUGINS_DIR` at `packages/plugins` (one plugin directory per entry).
