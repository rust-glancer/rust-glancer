#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, mkdirSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const extensionRoot = resolve(scriptDir, "..");
const workspaceRoot = resolve(extensionRoot, "../..");

const rustToVsCodeTarget = new Map([
  ["x86_64-apple-darwin", "darwin-x64"],
  ["aarch64-apple-darwin", "darwin-arm64"],
  ["x86_64-unknown-linux-gnu", "linux-x64"],
  ["aarch64-unknown-linux-gnu", "linux-arm64"],
  ["arm-unknown-linux-gnueabihf", "linux-armhf"],
  ["x86_64-unknown-linux-musl", "alpine-x64"],
  ["aarch64-unknown-linux-musl", "alpine-arm64"],
  ["x86_64-pc-windows-msvc", "win32-x64"],
  ["aarch64-pc-windows-msvc", "win32-arm64"],
]);

const options = parseArgs(process.argv.slice(2));
const rustTarget = options.rustTarget ?? detectHostTarget();
const vsCodeTarget = options.vsCodeTarget ?? rustToVsCodeTarget.get(rustTarget);

if (vsCodeTarget === undefined) {
  fail(
    `No VS Code target is known for Rust target '${rustTarget}'. ` +
      "Pass --vscode-target explicitly if this platform is supported.",
  );
}

const outPath = options.outPath ?? join(workspaceRoot, "dist", `rust-glancer-${vsCodeTarget}.vsix`);
const executableName = rustTarget.includes("windows") ? "rust-glancer.exe" : "rust-glancer";
const builtServer = join(workspaceRoot, "target", rustTarget, "release", executableName);
const bundledServerDir = join(extensionRoot, "server");
const bundledServer = join(bundledServerDir, executableName);
const workspaceLicense = join(workspaceRoot, "LICENSE");
const extensionLicense = join(extensionRoot, "LICENSE");

if (!options.skipBuild) {
  run("cargo", ["build", "--release", "-p", "rust-glancer", "--target", rustTarget], {
    cwd: workspaceRoot,
  });
}

if (!existsSync(builtServer)) {
  fail(`Expected server binary does not exist: ${builtServer}`);
}

// The server directory is a generated packaging staging area. Recreate it so the VSIX contains
// exactly one platform binary even after switching targets between local packaging runs.
rmSync(bundledServerDir, { recursive: true, force: true });
mkdirSync(bundledServerDir, { recursive: true });
copyFileSync(builtServer, bundledServer);
if (!rustTarget.includes("windows")) {
  chmodSync(bundledServer, 0o755);
}

mkdirSync(dirname(outPath), { recursive: true });
if (existsSync(workspaceLicense)) {
  copyFileSync(workspaceLicense, extensionLicense);
}

const vsce = vsceBin();
if (!existsSync(vsce)) {
  fail(`Expected local vsce binary does not exist: ${vsce}. Run npm install in editors/code.`);
}

const vsceArgs = ["package", "-o", outPath, "--target", vsCodeTarget];
if (options.preRelease) {
  vsceArgs.push("--pre-release");
}

run(vsce, vsceArgs, { cwd: extensionRoot });
console.log(`Packaged ${outPath}`);

function parseArgs(args) {
  const parsed = {
    outPath: undefined,
    preRelease: false,
    rustTarget: undefined,
    skipBuild: false,
    vsCodeTarget: undefined,
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    switch (arg) {
      case "--help":
      case "-h":
        printHelp();
        process.exit(0);
        break;
      case "--out":
        parsed.outPath = resolve(readValue(args, ++index, arg));
        break;
      case "--pre-release":
        parsed.preRelease = true;
        break;
      case "--skip-build":
        parsed.skipBuild = true;
        break;
      case "--target":
        parsed.rustTarget = readValue(args, ++index, arg);
        break;
      case "--vscode-target":
        parsed.vsCodeTarget = readValue(args, ++index, arg);
        break;
      default:
        fail(`Unknown argument: ${arg}`);
    }
  }

  return parsed;
}

function readValue(args, index, flag) {
  const value = args[index];
  if (value === undefined || value.startsWith("--")) {
    fail(`Missing value for ${flag}`);
  }
  return value;
}

function detectHostTarget() {
  const output = runCapture("rustc", ["-vV"], { cwd: workspaceRoot });
  const host = output.match(/^host: (.+)$/m)?.[1];
  if (host === undefined) {
    fail("Could not detect rustc host target from `rustc -vV`.");
  }
  return host;
}

function vsceBin() {
  const name = process.platform === "win32" ? "vsce.cmd" : "vsce";
  return join(extensionRoot, "node_modules", ".bin", name);
}

function run(command, args, options) {
  console.log(`> ${[command, ...args].join(" ")}`);
  const result = spawnSync(command, args, { ...options, stdio: "inherit" });
  if (result.error !== undefined) {
    fail(result.error.message);
  }
  if (result.status !== 0) {
    fail(`Command exited with status ${result.status}: ${command}`);
  }
}

function runCapture(command, args, options) {
  const result = spawnSync(command, args, { ...options, encoding: "utf8" });
  if (result.error !== undefined) {
    fail(result.error.message);
  }
  if (result.status !== 0) {
    fail(`Command exited with status ${result.status}: ${command}`);
  }
  return result.stdout;
}

function printHelp() {
  console.log(`Usage: npm run package:vsix -- [options]

Options:
  --target <triple>         Rust target triple. Defaults to rustc host.
  --vscode-target <target>  VS Code extension target. Inferred for common Rust targets.
  --out <path>              VSIX output path. Defaults to ../../dist/rust-glancer-<target>.vsix.
  --pre-release            Mark the packaged extension as a pre-release.
  --skip-build             Reuse an existing target/<triple>/release/rust-glancer binary.
`);
}

function fail(message) {
  console.error(message);
  process.exit(1);
}
