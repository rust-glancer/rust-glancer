//! Pinned source templates and the disposable checkouts used by individual trials.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::Context as _;
use gen_lsp_types::Position;
use serde::{Deserialize, Serialize};

pub(super) const TOOLCHAIN: &str = "1.100.0";

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Site {
    pub(super) file: String,
    pub(super) anchor: String,
}

impl Site {
    pub(super) fn position(&self, source: &str) -> anyhow::Result<Position> {
        anchor_position(source, &self.anchor)
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Completion {
    pub(super) anchor: String,
    pub(super) label: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Replacement {
    pub(super) before: String,
    pub(super) after: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Simulation {
    pub(super) crate_root: String,
    pub(super) project_input: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Target {
    pub(super) id: String,
    url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lockfile: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    standalone: bool,
    pub(super) revision: String,
    pub(super) entry: Site,
    pub(super) definition: Site,
    edits: Vec<Replacement>,
    pub(super) completion: Completion,
    pub(super) return_type: Replacement,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) simulation: Option<Simulation>,
}

impl Target {
    pub(super) fn catalog() -> anyhow::Result<Vec<Self>> {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../test_targets/bench_fixtures/lsp/targets.json"
        )))
        .context("parse LSP benchmark target definitions")
    }

    pub(super) fn repository_url(&self) -> Option<&str> {
        let url = self.url.trim_end_matches('/');
        let url = url.strip_suffix(".git").unwrap_or(url);
        let (owner, repository) = url.strip_prefix("https://github.com/")?.split_once('/')?;
        let valid_component = |part: &str| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        };
        (valid_component(owner) && valid_component(repository)).then_some(url)
    }

    pub(super) fn prepare(&self, directory: &Path, managed: bool) -> anyhow::Result<PathBuf> {
        let legacy = directory.join(&self.id);
        let mut path = legacy.clone();
        if managed {
            let pinned = directory.join(format!("{}-{}", self.id, self.revision));
            // Reuse an existing checkout for the current pin. A changed pin gets its
            // own managed directory and leaves the previous checkout available.
            if pinned.exists()
                || !legacy.exists()
                || !git(&legacy, &["rev-parse", "HEAD"])
                    .is_ok_and(|head| head.trim() == self.revision)
            {
                path = pinned;
            }
        }
        if !path.exists() {
            // Publish the template only once the requested commit is available. A failed fetch
            // leaves the partial checkout at a different path so the next run can retry.
            let staging = directory.join(format!(".{}-{}", self.id, std::process::id()));
            fs::create_dir_all(&staging)?;
            git(&staging, &["init", "--quiet"])?;
            git(
                &staging,
                &[
                    "fetch",
                    "--quiet",
                    "--depth",
                    "1",
                    &self.url,
                    &self.revision,
                ],
            )?;
            git(
                &staging,
                &[
                    "-c",
                    "core.autocrlf=false",
                    "checkout",
                    "--quiet",
                    "--detach",
                    "FETCH_HEAD",
                ],
            )?;
            if let Err(error) = fs::rename(&staging, &path) {
                if !path.exists() {
                    return Err(error).context("install fetched benchmark template");
                }
                // Another process already installed the same pin.
                fs::remove_dir_all(&staging).context("remove redundant benchmark template")?;
            }
        }
        let revision = git(&path, &["rev-parse", "HEAD"])?;
        anyhow::ensure!(
            revision.trim() == self.revision,
            "benchmark template {} is at {}, expected {}",
            path.display(),
            revision.trim(),
            self.revision
        );
        let status = git(&path, &["status", "--porcelain", "--untracked-files=no"])?;
        anyhow::ensure!(
            status.is_empty(),
            "benchmark template {} has tracked edits",
            path.display()
        );
        Ok(path)
    }

    pub(super) fn checkout(&self, directory: &Path, template: &Path) -> anyhow::Result<Checkout> {
        let checkout = Checkout::clone(directory, template, &self.revision)?;
        if let Some(lockfile) = &self.lockfile {
            let contents = match lockfile.as_str() {
                "bevy.lock" => {
                    include_str!("../../../../../test_targets/bench_fixtures/lsp/locks/bevy.lock")
                }
                _ => anyhow::bail!("unknown lockfile snapshot {lockfile}"),
            };
            fs::write(checkout.source.join("Cargo.lock"), contents)
                .context("restore pinned Cargo.lock")?;
        }
        // A single-package project nested under rust-glancer's target/ otherwise inherits
        // our workspace. Give its disposable checkout a workspace of its own.
        if self.standalone {
            use std::io::Write as _;
            fs::OpenOptions::new()
                .append(true)
                .open(checkout.source.join("Cargo.toml"))?
                .write_all(b"\n[workspace]\n")?;
        }
        Ok(checkout)
    }

    pub(super) fn edit(&self, source: &str) -> anyhow::Result<String> {
        let mut text = source.to_owned();
        for Replacement { before, after } in &self.edits {
            anyhow::ensure!(
                text.matches(before).count() == 1,
                "{}: expected one occurrence of edit source {before:?}",
                self.id,
            );
            text = text.replacen(before, after, 1);
        }
        Ok(text)
    }
}

#[derive(Debug)]
pub(super) struct Checkout {
    pub(super) source: PathBuf,
    pub(super) cargo_output: PathBuf,
}

impl Checkout {
    fn clone(directory: &Path, template: &Path, revision: &str) -> anyhow::Result<Self> {
        fs::create_dir_all(directory)?;
        let source = directory.join("project");
        run(Command::new("git")
            .arg("clone")
            .args([
                "--quiet",
                "--shared",
                "--no-checkout",
                "-c",
                "core.autocrlf=false",
                "--",
            ])
            .arg(template)
            .arg(&source))?;
        git(&source, &["checkout", "--quiet", "--detach", revision])?;
        Ok(Self {
            source,
            cargo_output: directory.join("cargo"),
        })
    }

    pub(super) fn built_fixture(directory: &Path) -> anyhow::Result<Self> {
        let files = [
            (
                "Cargo.toml",
                include_str!(
                    "../../../../../test_targets/bench_fixtures/lsp/build_outputs/Cargo.toml"
                ),
            ),
            (
                "Cargo.lock",
                include_str!(
                    "../../../../../test_targets/bench_fixtures/lsp/build_outputs/Cargo.lock"
                ),
            ),
            (
                "build.rs",
                include_str!(
                    "../../../../../test_targets/bench_fixtures/lsp/build_outputs/build.rs"
                ),
            ),
            (
                "src/lib.rs",
                include_str!(
                    "../../../../../test_targets/bench_fixtures/lsp/build_outputs/src/lib.rs"
                ),
            ),
        ];
        Self::from_files(directory, &files)
    }

    pub(super) fn from_files(directory: &Path, files: &[(&str, &str)]) -> anyhow::Result<Self> {
        let source = directory.join("project");
        for (file, text) in files {
            let path = source.join(file);
            let parent = path
                .parent()
                .context("fixture file has no parent directory")?;
            fs::create_dir_all(parent)?;
            fs::write(path, text)
                .with_context(|| format!("write benchmark fixture file {file}"))?;
        }
        Ok(Self {
            source,
            cargo_output: directory.join("cargo"),
        })
    }

    pub(super) fn cargo(&self) -> Command {
        let mut cargo = Command::new("cargo");
        cargo
            .current_dir(&self.source)
            .env("RUSTUP_TOOLCHAIN", TOOLCHAIN)
            .env("CARGO_TARGET_DIR", &self.cargo_output)
            .env("CARGO_BUILD_BUILD_DIR", &self.cargo_output);
        cargo
    }
}

/// Anchors use one marker in a unique source fragment. Resolve against the actual document text
/// because byte columns differ from LSP columns for non-ASCII source.
pub(super) fn anchor_position(source: &str, anchor: &str) -> anyhow::Result<Position> {
    let offset = anchor_offset(source, anchor)?;
    Ok(position(&source[..offset]))
}

pub(super) fn anchor_offset(source: &str, anchor: &str) -> anyhow::Result<usize> {
    let (before, after) = anchor
        .split_once('|')
        .context("source anchor requires a | marker")?;
    let fragment = format!("{before}{after}");
    let mut matches = source.match_indices(&fragment);
    let (start, _) = matches
        .next()
        .with_context(|| format!("source anchor {anchor:?} is missing"))?;
    anyhow::ensure!(
        matches.next().is_none(),
        "source anchor {anchor:?} is ambiguous"
    );
    Ok(start + before.len())
}

pub(super) fn position(prefix: &str) -> Position {
    let line = prefix.bytes().filter(|&byte| byte == b'\n').count();
    let column = prefix
        .rsplit('\n')
        .next()
        .expect("split always has one part")
        .encode_utf16()
        .count();
    Position::new(line as u32, column as u32)
}

fn git(directory: &Path, args: &[&str]) -> anyhow::Result<String> {
    run(Command::new("git")
        .current_dir(directory)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(args))
}

pub(super) fn run(command: &mut Command) -> anyhow::Result<String> {
    let output = command
        .output()
        .with_context(|| format!("run {command:?}"))?;
    anyhow::ensure!(
        output.status.success(),
        "{command:?} failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
