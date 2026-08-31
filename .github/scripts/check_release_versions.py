#!/usr/bin/env python3
"""Ensure every shipped artifact uses the release version."""

import json
import re
from pathlib import Path
from typing import Any


WORKSPACE_ROOT = Path(__file__).resolve().parents[2]
RELEASE_PLEASE_ROOT = WORKSPACE_ROOT / ".github" / "release-please"
STABLE_SEMVER = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
SERVER_VERSION = re.compile(
    r'^pub const SERVER_VERSION: &str = "([^"]+)"; // x-release-please-version$',
    re.MULTILINE,
)
ZED_EXTENSION_VERSION = re.compile(
    r'^version = "([^"]+)" # x-release-please-version$',
    re.MULTILINE,
)
ZED_MANAGED_SERVER_VERSION = re.compile(
    r'^const MANAGED_SERVER_VERSION: &str = "([^"]+)"; // x-release-please-version$',
    re.MULTILINE,
)


def main() -> None:
    version = (RELEASE_PLEASE_ROOT / "version.txt").read_text(encoding="utf-8").strip()
    failures: list[str] = []
    if STABLE_SEMVER.fullmatch(version) is None:
        failures.append(
            ".github/release-please/version.txt contains unsupported "
            f"version {version!r}"
        )

    # The server is shipped inside the extension, so it reports the extension's release version
    # rather than the intentionally fixed Cargo package version.
    methods = (WORKSPACE_ROOT / "crates/lsp/server/src/methods/mod.rs").read_text(
        encoding="utf-8"
    )
    server_version = SERVER_VERSION.search(methods)
    if server_version is None:
        failures.append("LSP server release version marker is missing")
    elif server_version.group(1) != version:
        failures.append(
            f"LSP server has version {server_version.group(1)!r}, expected {version}"
        )

    zed_manifest = (WORKSPACE_ROOT / "editors/zed/extension.toml").read_text(
        encoding="utf-8"
    )
    zed_extension_version = ZED_EXTENSION_VERSION.search(zed_manifest)
    if zed_extension_version is None:
        failures.append("Zed extension release version marker is missing")
    elif zed_extension_version.group(1) != version:
        failures.append(
            "Zed extension has version "
            f"{zed_extension_version.group(1)!r}, expected {version}"
        )

    zed_server = (WORKSPACE_ROOT / "editors/zed/src/server/mod.rs").read_text(
        encoding="utf-8"
    )
    zed_managed_server_version = ZED_MANAGED_SERVER_VERSION.search(zed_server)
    if zed_managed_server_version is None:
        failures.append("Zed managed server release version marker is missing")
    elif zed_managed_server_version.group(1) != version:
        failures.append(
            "Zed managed server has version "
            f"{zed_managed_server_version.group(1)!r}, expected {version}"
        )

    package_json = read_json(WORKSPACE_ROOT / "editors/code/package.json")
    if package_json.get("version") != version:
        failures.append(
            "editors/code/package.json has version "
            f"{package_json.get('version')!r}, expected {version}"
        )

    package_lock = read_json(WORKSPACE_ROOT / "editors/code/package-lock.json")
    if package_lock.get("version") != version:
        failures.append(
            "editors/code/package-lock.json has version "
            f"{package_lock.get('version')!r}, expected {version}"
        )

    lock_root = package_lock.get("packages", {}).get("")
    lock_root_version = lock_root.get("version") if isinstance(lock_root, dict) else None
    if lock_root_version != version:
        failures.append(
            "editors/code/package-lock.json root package has version "
            f"{lock_root_version!r}, expected {version}"
        )

    # The manifest is empty before the first release. After that, its value is the last released
    # version and Release Please updates it in the same release PR as the product files.
    release_manifest = read_json(RELEASE_PLEASE_ROOT / "manifest.json")
    manifest_version = release_manifest.get(".")
    if manifest_version is not None and manifest_version != version:
        failures.append(
            ".github/release-please/manifest.json has version "
            f"{manifest_version!r}, expected {version}"
        )

    if failures:
        details = "\n".join(f"- {failure}" for failure in failures)
        raise SystemExit(f"Release version check failed:\n{details}")

    print(
        f"Release version {version} is synchronized across the LSP server "
        "and editor extensions."
    )


def read_json(path: Path) -> dict[str, Any]:
    with path.open(encoding="utf-8") as file:
        value = json.load(file)
    if not isinstance(value, dict):
        raise SystemExit(f"{path.relative_to(WORKSPACE_ROOT)} is not a JSON object")
    return value


if __name__ == "__main__":
    main()
