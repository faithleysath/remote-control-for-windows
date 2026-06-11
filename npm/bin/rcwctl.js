#!/usr/bin/env node

const { spawnSync } = require("node:child_process");
const { existsSync } = require("node:fs");
const path = require("node:path");

const packages = {
  "darwin:arm64": "@faithleysath/rcwctl-darwin-arm64",
  "darwin:x64": "@faithleysath/rcwctl-darwin-x64",
  "linux:arm64": "@faithleysath/rcwctl-linux-arm64",
  "linux:x64": "@faithleysath/rcwctl-linux-x64",
  "win32:arm64": "@faithleysath/rcwctl-win32-arm64",
  "win32:x64": "@faithleysath/rcwctl-win32-x64"
};
const executable = process.platform === "win32" ? "rcwctl.exe" : "rcwctl";
const packageName = packages[`${process.platform}:${process.arch}`];

if (!packageName) {
  console.error(
    `Unsupported platform: ${process.platform} ${process.arch}. Reinstall @faithleysath/rcwctl on a supported platform.`
  );
  process.exit(1);
}

let binary;

try {
  const packageJson = require.resolve(`${packageName}/package.json`);
  binary = path.join(path.dirname(packageJson), "bin", executable);
} catch (error) {
  console.error(`rcwctl platform package is missing: ${packageName}. Reinstall @faithleysath/rcwctl.`);
  process.exit(1);
}

if (!existsSync(binary)) {
  console.error(`rcwctl binary is missing from ${packageName}. Reinstall @faithleysath/rcwctl.`);
  process.exit(1);
}

const result = spawnSync(binary, process.argv.slice(2), {
  stdio: "inherit"
});

if (result.error) {
  console.error(result.error.message);
  process.exit(1);
}

process.exit(result.status === null ? 1 : result.status);
