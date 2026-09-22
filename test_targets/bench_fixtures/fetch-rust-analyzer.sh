#!/usr/bin/env bash
set -euo pipefail

repo_url="https://github.com/rust-lang/rust-analyzer.git"

# This revision is part of the benchmark definition. Keep it stable unless the
# benchmark target intentionally needs to move, and justify that change in review.
# rust-analyzer v0.3.3057, tag 2026-09-21.
revision="aaddfb73fd95f2c0bf001b474dca91ae28bcce3a"

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
fixture_dir="${script_dir}/rust-analyzer"

if [[ -e "${fixture_dir}" && ! -d "${fixture_dir}/.git" ]]; then
    echo "error: ${fixture_dir} exists but is not a git checkout" >&2
    exit 1
fi

if [[ ! -d "${fixture_dir}/.git" ]]; then
    git clone --filter=blob:none "${repo_url}" "${fixture_dir}"
fi

if [[ -n "$(git -C "${fixture_dir}" status --porcelain)" ]]; then
    echo "error: ${fixture_dir} has local changes; preserve them before updating the fixture" >&2
    exit 1
fi

# Resolve missing partial-clone objects from upstream even when origin is a local reference clone.
git -C "${fixture_dir}" -c remote.origin.url="${repo_url}" fetch --filter=blob:none origin "${revision}"
git -C "${fixture_dir}" -c remote.origin.url="${repo_url}" checkout --detach "${revision}"

(
    cd "${fixture_dir}"
    cargo fetch --locked --quiet
)
