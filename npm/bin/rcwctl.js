#!/usr/bin/env node

const { spawnSync } = require("node:child_process");
const { existsSync } = require("node:fs");
const path = require("node:path");

const executable = process.platform === "win32" ? "rcwctl.exe" : "rcwctl";
const binary = path.join(__dirname, "..", "vendor", executable);

if (!existsSync(binary)) {
  console.error("rcwctl binary is missing. Reinstall @faithleysath/rcwctl.");
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
