# packages/

TypeScript and TSX packages for the Steward monorepo.

- `extension-api`: `@steward/extension-api`, typed helpers around Steward's
  NSH host calls (`clipboardHistory`, `closeWindow`, `onCommand`, `onTheme`).
- `plugins/`: official NSH applications. Each plugin is checked, tested,
  built, signed, and packaged with the pinned NSH CLI.

Run `pnpm typecheck` and `pnpm build` from the repository root after installing
the pinned NSH CLI described in the top-level README. Package output stays
under each plugin's ignored `dist/` directory.
