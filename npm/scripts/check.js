const fs = require("node:fs");
const path = require("node:path");

const root = path.join(__dirname, "..");
const rootPackage = require(path.join(root, "package.json"));

const variants = [
  {
    dir: "darwin-arm64",
    name: "rcwctl-darwin-arm64",
    os: ["darwin"],
    cpu: ["arm64"],
    binary: "bin/rcwctl"
  },
  {
    dir: "darwin-x64",
    name: "rcwctl-darwin-x64",
    os: ["darwin"],
    cpu: ["x64"],
    binary: "bin/rcwctl"
  },
  {
    dir: "linux-arm64",
    name: "rcwctl-linux-arm64",
    os: ["linux"],
    cpu: ["arm64"],
    binary: "bin/rcwctl"
  },
  {
    dir: "linux-x64",
    name: "rcwctl-linux-x64",
    os: ["linux"],
    cpu: ["x64"],
    binary: "bin/rcwctl"
  },
  {
    dir: "win32-arm64",
    name: "rcwctl-win32-arm64",
    os: ["win32"],
    cpu: ["arm64"],
    binary: "bin/rcwctl.exe"
  },
  {
    dir: "win32-x64",
    name: "rcwctl-win32-x64",
    os: ["win32"],
    cpu: ["x64"],
    binary: "bin/rcwctl.exe"
  }
];

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

function main() {
  assert(rootPackage.name === "rcwctl", "meta package name must be rcwctl");
  assert(!rootPackage.scripts.postinstall, "meta package must not use postinstall");
  assert(rootPackage.bin && rootPackage.bin.rcwctl === "bin/rcwctl.js", "meta package bin entry is invalid");

  for (const variant of variants) {
    const pkgPath = path.join(root, "packages", variant.dir, "package.json");
    const readmePath = path.join(root, "packages", variant.dir, "README.md");
    assert(fs.existsSync(pkgPath), `missing package manifest: ${variant.dir}`);
    assert(fs.existsSync(readmePath), `missing package README: ${variant.dir}`);

    const pkg = readJson(pkgPath);
    assert(pkg.name === variant.name, `${variant.dir} name must be ${variant.name}`);
    assert(pkg.version === rootPackage.version, `${variant.dir} version must match root package`);
    assert(Array.isArray(pkg.os) && pkg.os.length === 1 && pkg.os[0] === variant.os[0], `${variant.dir} os mismatch`);
    assert(Array.isArray(pkg.cpu) && pkg.cpu.length === 1 && pkg.cpu[0] === variant.cpu[0], `${variant.dir} cpu mismatch`);
    assert(pkg.optionalDependencies === undefined, `${variant.dir} must not declare optionalDependencies`);
    assert(pkg.bin && pkg.bin.rcwctl === variant.binary, `${variant.dir} bin entry is invalid`);
  }

  const deps = rootPackage.optionalDependencies || {};
  assert(Object.keys(deps).length === variants.length, "meta package optionalDependencies count mismatch");
  for (const variant of variants) {
    assert(deps[variant.name] === rootPackage.version, `${variant.name} version mismatch in optionalDependencies`);
  }

  console.log("npm package metadata ok");
}

main();
