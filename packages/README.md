# packages/

TypeScript side of the Steward monorepo (pnpm workspace).

- `extension-api` — `@steward/extension-api`，面向插件开发者的 TS 类型声明与运行时 polyfill（M2 起实现运行时）。
- `plugins/` — 官方示例插件，M2 起用 esbuild 打成单文件 JS 供 QuickJS 宿主加载。

脚手架 `create-plugin-cli` 计划在 M5 提供，暂不建包。
