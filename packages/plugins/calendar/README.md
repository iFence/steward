# Calendar (official example plugin)

The M2 end-to-end example: TypeScript -> esbuild single-file IIFE -> QuickJS ->
launcher rows. Typing `calendar` (optionally with an offset, e.g.
`calendar +3`) shows the next seven days; selecting a row copies its ISO date
to the clipboard and shows a toast.

```text
packages/plugins/calendar/
├── plugin.json          # manifest scanned by steward-plugin-registry
├── src/index.ts         # plugin source (imports @steward/extension-api)
└── dist/index.js        # esbuild IIFE bundle, loaded by the QuickJS runtime
```

Build: `pnpm --filter @steward/plugin-calendar build`.

To run the app against this repo's plugin instead of the installed copy, point
`STEWARD_PLUGINS_DIR` at `packages/plugins` (one plugin directory per entry).
