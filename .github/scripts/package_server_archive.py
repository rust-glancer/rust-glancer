#!/usr/bin/env python3
"""Package a Rust Glancer binary for editor-managed installation."""

import argparse
import gzip
import hashlib
import os
import re
import tarfile
import tempfile
from pathlib import Path
from typing import BinaryIO, Dict, Optional, Tuple


WORKSPACE_ROOT = Path(__file__).resolve().parents[2]
STABLE_SEMVER = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
SUPPORTED_TARGETS = {
    "aarch64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
}


def main() -> None:
    arguments = parse_arguments()
    if STABLE_SEMVER.fullmatch(arguments.version) is None:
        raise SystemExit(f"Unsupported release version: {arguments.version!r}")
    if arguments.target not in SUPPORTED_TARGETS:
        raise SystemExit(f"Unsupported release target: {arguments.target!r}")

    archive_name = f"rust-glancer-{arguments.version}-{arguments.target}.tar.gz"
    output = arguments.out or WORKSPACE_ROOT / "dist" / archive_name
    sources = {
        "rust-glancer": (
            WORKSPACE_ROOT
            / "target"
            / arguments.target
            / "release"
            / "rust-glancer",
            0o755,
        ),
        "LICENSE-MIT": (WORKSPACE_ROOT / "LICENSE-MIT", 0o644),
        "LICENSE-APACHE": (WORKSPACE_ROOT / "LICENSE-APACHE", 0o644),
    }

    for archive_path, (source, _) in sources.items():
        if not source.is_file():
            raise SystemExit(
                f"Expected {archive_path} source does not exist: {source}"
            )

    output.parent.mkdir(parents=True, exist_ok=True)

    # Write deterministic metadata and replace the output only after the complete archive
    # has been verified. This prevents an interrupted packaging run from leaving a release
    # artifact that merely looks complete by its file name.
    temporary_path: Optional[Path] = None
    try:
        with tempfile.NamedTemporaryFile(
            dir=output.parent,
            prefix=f".{archive_name}.",
            suffix=".tmp",
            delete=False,
        ) as temporary_file:
            temporary_path = Path(temporary_file.name)
            with gzip.GzipFile(
                filename="",
                mode="wb",
                fileobj=temporary_file,
                mtime=0,
            ) as compressed_file:
                with tarfile.open(fileobj=compressed_file, mode="w") as archive:
                    for archive_path, (source, mode) in sources.items():
                        add_file(archive, archive_path, source, mode)

        verify_archive(temporary_path, sources)
        os.replace(temporary_path, output)
        temporary_path = None
    finally:
        if temporary_path is not None and temporary_path.exists():
            temporary_path.unlink()

    print(f"Packaged {output}")


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Package a standalone Rust Glancer release archive."
    )
    parser.add_argument("--target", required=True, help="Rust target triple")
    parser.add_argument("--version", required=True, help="stable release version")
    parser.add_argument(
        "--out",
        type=Path,
        help="output path; defaults to dist/<canonical archive name>",
    )
    return parser.parse_args()


def add_file(
    archive: tarfile.TarFile,
    archive_path: str,
    source: Path,
    mode: int,
) -> None:
    info = archive.gettarinfo(str(source), arcname=archive_path)
    info.uid = 0
    info.gid = 0
    info.uname = ""
    info.gname = ""
    info.mtime = 0
    info.mode = mode
    info.pax_headers = {}
    with source.open("rb") as source_file:
        archive.addfile(info, source_file)


def verify_archive(
    archive_path: Path,
    sources: Dict[str, Tuple[Path, int]],
) -> None:
    with tarfile.open(archive_path, mode="r:gz") as archive:
        members = archive.getmembers()
        actual_paths = [member.name for member in members]
        expected_paths = list(sources)
        if actual_paths != expected_paths:
            raise SystemExit(
                "Packaged archive contains unexpected paths: "
                f"expected {expected_paths!r}, got {actual_paths!r}"
            )

        for member in members:
            source, expected_mode = sources[member.name]
            if not member.isfile():
                raise SystemExit(f"Archive member is not a file: {member.name}")
            if member.mode & 0o777 != expected_mode:
                raise SystemExit(
                    f"Archive member {member.name} has mode {member.mode & 0o777:o}, "
                    f"expected {expected_mode:o}"
                )

            packaged_file = archive.extractfile(member)
            if packaged_file is None:
                raise SystemExit(f"Could not read archive member: {member.name}")
            with packaged_file, source.open("rb") as source_file:
                if file_digest(packaged_file) != file_digest(source_file):
                    raise SystemExit(
                        f"Archive member differs from its source: {member.name}"
                    )


def file_digest(stream: BinaryIO) -> bytes:
    digest = hashlib.sha256()
    while True:
        chunk = stream.read(1024 * 1024)
        if not chunk:
            break
        digest.update(chunk)
    return digest.digest()


if __name__ == "__main__":
    main()
