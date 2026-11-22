#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const packageDir = join(root, "packages", "extension-api");
const outputDir = join(root, "target", "sdk");
const contractPath = join(root, "contracts", "plugin-api-v1.json");
const compatibilityPath = join(root, "contracts", "plugin-compatibility.json");
const cargoPath = join(root, "Cargo.toml");

function run(command, args, options = {}) {
  return execFileSync(command, args, {
    cwd: root,
    encoding: "utf8",
    stdio: options.capture ? "pipe" : "inherit",
    ...options,
  });
}

function pnpm(args, options = {}) {
  if (process.env.npm_execpath) {
    return run(process.execPath, [process.env.npm_execpath, ...args], options);
  }
  if (process.platform === "win32") {
    return run("corepack.cmd", ["pnpm", ...args], options);
  }
  return run("corepack", ["pnpm", ...args], options);
}

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function assertInside(rootPath, target, label) {
  const resolvedRoot = resolve(rootPath);
  const resolvedTarget = resolve(target);
  const child = relative(resolvedRoot, resolvedTarget);
  if (!child || child.startsWith("..") || isAbsolute(child)) {
    throw new Error(`${label} is outside ${resolvedRoot}: ${resolvedTarget}`);
  }
}

function validateVersions(packageJson) {
  const contract = readJson(contractPath);
  const compatibility = readJson(compatibilityPath);
  const current = compatibility.current;
  const cargo = readFileSync(cargoPath, "utf8");
  const nshVersion = /^nsh-core\s*=\s*"=([^"]+)"/m.exec(cargo)?.[1];

  const checks = [
    [contract.hostApiVersion, current.hostApiVersion, "host API version"],
    [packageJson.version, current.sdkVersion, "SDK version"],
    [packageJson.version, contract.hostApiVersion, "SDK and current host API version"],
    [current.minimumHostVersion, current.hostApiVersion, "minimum host version"],
    [nshVersion, current.nshVersion, "NSH version"],
  ];
  for (const [actual, expected, label] of checks) {
    if (actual !== expected) {
      throw new Error(
        `${label} mismatch: metadata says ${actual}, compatibility matrix says ${expected}`,
      );
    }
  }
  if (!compatibility.supported.some((line) => line.status === "current")) {
    throw new Error("compatibility matrix has no current supported line");
  }
}

function validateTarball(tarball) {
  const entries = run("tar", ["-tzf", tarball], { capture: true })
    .split(/\r?\n/)
    .filter(Boolean)
    .map((entry) => entry.replaceAll("\\", "/"));
  const required = [
    "package/package.json",
    "package/README.md",
    "package/types/nsh.d.ts",
    "package/dist/host.js",
    "package/dist/host.d.ts",
    "package/dist/contract.generated.js",
    "package/dist/contract.generated.d.ts",
  ];
  for (const entry of required) {
    if (!entries.includes(entry)) {
      throw new Error(`SDK tarball is missing ${entry}`);
    }
  }
  if (entries.some((entry) => entry.startsWith("package/src/"))) {
    throw new Error("SDK tarball must not contain raw TypeScript sources");
  }

  const packedPackageJson = JSON.parse(
    run("tar", ["-xOzf", tarball, "package/package.json"], { capture: true }),
  );
  const rootExport = packedPackageJson.exports?.["."];
  if (
    packedPackageJson.main !== "./dist/host.js" ||
    packedPackageJson.types !== "./dist/host.d.ts" ||
    rootExport?.import !== "./dist/host.js" ||
    rootExport?.types !== "./dist/host.d.ts"
  ) {
    throw new Error("SDK package has incomplete JavaScript or declaration exports");
  }
  if (packedPackageJson.files?.includes("src")) {
    throw new Error("SDK package files list still exposes source");
  }
}

mkdirSync(outputDir, { recursive: true });
assertInside(join(root, "target"), outputDir, "SDK output directory");
for (const entry of readdirSync(outputDir)) {
  if (entry.startsWith("steward-extension-api-") || entry === "SHA256SUMS-sdk.txt") {
    rmSync(join(outputDir, entry), { force: true });
  }
}

const packageJson = readJson(join(packageDir, "package.json"));
validateVersions(packageJson);
pnpm(["--filter", "@steward/extension-api", "pack", "--pack-destination", outputDir]);

const tarballs = readdirSync(outputDir)
  .filter((entry) => /^steward-extension-api-.+\.tgz$/.test(entry))
  .map((entry) => join(outputDir, entry));
if (tarballs.length !== 1) {
  throw new Error(`expected one SDK tarball under ${outputDir}, found ${tarballs.length}`);
}
const tarball = tarballs[0];
if (!existsSync(tarball)) {
  throw new Error(`SDK tarball was not created: ${tarball}`);
}
validateTarball(tarball);

const digest = createHash("sha256").update(readFileSync(tarball)).digest("hex");
writeFileSync(
  join(outputDir, "SHA256SUMS-sdk.txt"),
  `${digest}  ${tarball.split(/[\\/]/).at(-1)}\n`,
  "utf8",
);
console.log(`Packaged ${relative(root, tarball)} (${digest}).`);
