#!/usr/bin/env node

const {
  chmodSync,
  copyFileSync,
  createWriteStream,
  existsSync,
  mkdirSync,
  rmSync
} = require("node:fs");
const { mkdtemp } = require("node:fs/promises");
const https = require("node:https");
const os = require("node:os");
const path = require("node:path");
const { pipeline } = require("node:stream/promises");
const { spawnSync } = require("node:child_process");

const version = require("../package.json").version;
const repo = "faithleysath/remote-control-for-windows";
const checkOnly = process.argv.includes("--check");

function targetInfo() {
  const platform = process.platform;
  const arch = process.arch;

  if (platform === "linux" && arch === "x64") {
    return { triple: "x86_64-unknown-linux-gnu", archive: "tar.gz", type: "tar" };
  }
  if (platform === "linux" && arch === "arm64") {
    return { triple: "aarch64-unknown-linux-gnu", archive: "tar.gz", type: "tar" };
  }
  if (platform === "darwin" && arch === "x64") {
    return { triple: "x86_64-apple-darwin", archive: "tar.gz", type: "tar" };
  }
  if (platform === "darwin" && arch === "arm64") {
    return { triple: "aarch64-apple-darwin", archive: "tar.gz", type: "tar" };
  }
  if (platform === "win32" && arch === "x64") {
    return { triple: "x86_64-pc-windows-msvc", archive: "zip", type: "zip" };
  }
  if (platform === "win32" && arch === "arm64") {
    return { triple: "aarch64-pc-windows-msvc", archive: "zip", type: "zip" };
  }

  throw new Error(
    `Unsupported platform: ${platform} ${arch}. ` +
      "@faithleysath/rcwctl prebuilt npm installs support Linux glibc, macOS, and Windows on x64/arm64."
  );
}

function download(url, destination) {
  return new Promise((resolve, reject) => {
    const request = https.get(
      url,
      {
        headers: {
          "User-Agent": "@faithleysath/rcwctl installer"
        }
      },
      (response) => {
        if ([301, 302, 303, 307, 308].includes(response.statusCode)) {
          response.resume();
          download(response.headers.location, destination).then(resolve, reject);
          return;
        }
        if (response.statusCode !== 200) {
          response.resume();
          reject(new Error(`Download failed: ${response.statusCode} ${response.statusMessage}`));
          return;
        }
        pipeline(response, createWriteStream(destination)).then(resolve, reject);
      }
    );
    request.on("error", reject);
  });
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    stdio: "inherit",
    ...options
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(" ")} failed with exit code ${result.status}`);
  }
}

function psLiteral(value) {
  return `'${value.replace(/'/g, "''")}'`;
}

function extractArchive(archivePath, destination, type) {
  if (type === "tar") {
    run("tar", ["-xzf", archivePath, "-C", destination]);
    return;
  }

  if (process.platform === "win32") {
    run("powershell.exe", [
      "-NoProfile",
      "-ExecutionPolicy",
      "Bypass",
      "-Command",
      `Expand-Archive -LiteralPath ${psLiteral(archivePath)} -DestinationPath ${psLiteral(destination)} -Force`
    ]);
    return;
  }

  run("python3", ["-m", "zipfile", "-e", archivePath, destination]);
}

async function main() {
  const target = targetInfo();
  const packageName = `rcw-tools-${target.triple}`;
  const executable = process.platform === "win32" ? "rcwctl.exe" : "rcwctl";
  const vendor = path.join(__dirname, "..", "vendor");
  const binary = path.join(vendor, executable);

  if (checkOnly) {
    if (!existsSync(binary)) {
      throw new Error(`Missing installed binary at ${binary}`);
    }
    run(binary, ["--version"]);
    return;
  }

  mkdirSync(vendor, { recursive: true });
  rmSync(binary, { force: true });

  const archive = `${packageName}.${target.archive}`;
  const url = `https://github.com/${repo}/releases/download/v${version}/${archive}`;
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "rcwctl-npm-"));
  const archivePath = path.join(tempDir, archive);

  try {
    await download(url, archivePath);
    extractArchive(archivePath, tempDir, target.type);

    const extractedBinary = path.join(tempDir, packageName, executable);
    if (!existsSync(extractedBinary)) {
      throw new Error(`Release archive did not contain ${packageName}/${executable}`);
    }

    copyFileSync(extractedBinary, binary);
    chmodSync(binary, 0o755);
  } finally {
    rmSync(tempDir, { recursive: true, force: true });
  }
}

main().catch((error) => {
  console.error(`@faithleysath/rcwctl install failed: ${error.message}`);
  process.exit(1);
});
