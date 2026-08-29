#!/usr/bin/env node
// Shared plugin bundler.
//
// Bundles a plugin's `src/index.ts` into a single-file IIFE assigned to
// `globalThis.__stewardPlugin`, exactly like the old inline `esbuild` command,
// but Node built-in modules (`fs`, `path`, `buffer`, ...) are marked external.
// The plugin runtime injects a `require`/module registry before evaluating the
// bundle (see `plugin-runtime/src/node_polyfill.js`), so those requires resolve
// to the runtime's polyfills instead of being inlined into the bundle. The
// extension-api is still bundled inline as before; only Node builtins are kept
// external.
//
// Usage: node scripts/build-plugin.mjs <entry> <outfile>

import { build } from "esbuild";

const nodeBuiltins = [
  "assert",
  "buffer",
  "child_process",
  "crypto",
  "dns",
  "events",
  "fs",
  "http",
  "https",
  "net",
  "os",
  "path",
  "process",
  "querystring",
  "stream",
  "string_decoder",
  "url",
  "util",
  "zlib",
];

const [, , entry, outfile] = process.argv;
if (!entry || !outfile) {
  console.error("usage: build-plugin.mjs <entry> <outfile>");
  process.exit(1);
}

await build({
  entryPoints: [entry],
  outfile,
  bundle: true,
  format: "iife",
  globalName: "__stewardPlugin",
  external: nodeBuiltins,
  logLevel: "info",
});
