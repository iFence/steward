#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const pluginsRoot = join(root, "packages", "plugins");

const plugins = readdirSync(pluginsRoot, { withFileTypes: true })
  .filter(
    (entry) => entry.isDirectory() && existsSync(join(pluginsRoot, entry.name, "plugin.json")),
  )
  .map((entry) => {
    const directory = join(pluginsRoot, entry.name);
    const manifest = JSON.parse(readFileSync(join(directory, "package.json"), "utf8"));
    if (typeof manifest.scripts?.test !== "string") {
      throw new Error(`official plugin ${manifest.name} has no test script`);
    }
    return { directory, name: manifest.name };
  })
  .sort((left, right) => left.name.localeCompare(right.name));

if (!plugins.length) {
  throw new Error("no official plugins were discovered");
}

for (const plugin of plugins) {
  console.log(`Testing ${plugin.name}`);
  if (process.env.npm_execpath) {
    execFileSync(process.execPath, [process.env.npm_execpath, "--filter", plugin.name, "test"], {
      cwd: root,
      stdio: "inherit",
    });
  } else {
    execFileSync("pnpm", ["--filter", plugin.name, "test"], {
      cwd: root,
      stdio: "inherit",
      shell: process.platform === "win32",
    });
  }
}

console.log(`Tested ${plugins.length} official plugins.`);
