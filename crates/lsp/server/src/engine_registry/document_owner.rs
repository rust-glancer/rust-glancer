use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::Context as _;

use super::{
    routing::{EngineId, normalize_path},
    state::{EngineRegistryInner, ReservedEngineRoute},
};

/// A document-scoped routing decision before the engine process has necessarily become ready.
#[derive(Debug)]
pub(super) struct DocumentOwner {
    route: ReservedEngineRoute,
    source: DocumentOwnerSource,
}

impl DocumentOwner {
    /// Resolves the engine that owns a document, applying the requested cache behavior.
    pub(super) fn new(
        inner: &mut EngineRegistryInner,
        path: &Path,
        cache_policy: OpenFileCachePolicy,
    ) -> anyhow::Result<Option<Self>> {
        // Open file that needs to be removed from cache.
        if cache_policy == OpenFileCachePolicy::Remove {
            return Ok(inner.remove_open_file(path, None).map(Self::cached));
        }

        // Do we know this file? If yes, return it.
        if let Some(id) = inner.open_file_owner(path) {
            return Ok(Some(Self::cached(id)));
        }

        // Outside configured folders, files are usually dependencies or sysroot sources reached
        // from an active project, so we assume that it's a part of the same engine.
        //
        // TODO: This is not a correct approach, this is a heuristic. It can fail in some cases
        // where it shouldn't. However, it's good enough for 95% normal user flows and there
        // is a ton of other things that are missing in this project, so implementing a perfect
        // solution is not a priority for now. Additionally, implementing a _proper_ solution
        // is going to be a tradeoff anyway, e.g.:
        // - If we just open a random Rust file, do we start a new LSP for it? When do we shut
        //   it down, if so?
        // - If we open a local project that is dependency of another project in the same
        //   workspace, do we start LSP for it? What if we first open a dependency, and then
        //   "parent"?
        // Answering these queestions is postponed until it _really_ becomes an issue and
        // there will be real users affected by this heuristic.
        let Some(discovery_workspace) = inner
            .routing
            .discovery_workspace_for(path)
            .map(Path::to_path_buf)
        else {
            return Ok(Self::fallback(inner, path, cache_policy));
        };

        // This is an unknown workspace file.
        if cache_policy == OpenFileCachePolicy::Ignore {
            tracing::warn!(
                path = %path.display(),
                "had to invoke locate-project for unopened file"
            );
        }

        // Ask Cargo for the workspace root from the containing VS Code workspace folder rather
        // than from the document directory. Nested rust-toolchain overrides can be older than the
        // workspace itself and break lightweight routing before analysis gets involved.
        if let Some(workspace_root) = Self::locate_workspace_root(path, &discovery_workspace)? {
            if let Some(owner) =
                Self::for_cargo_workspace(inner, path, workspace_root, cache_policy)
            {
                return Ok(Some(owner));
            }
        }

        // Cargo could not associate the file with a routable workspace, so keep the request
        // contextual and use the last active engine if one is available.
        Ok(Self::fallback(inner, path, cache_policy))
    }

    /// Reuses the engine remembered when the document was opened.
    fn cached(id: EngineId) -> Self {
        Self::existing(id, DocumentOwnerSource::OpenFileCache)
    }

    /// Resolves Cargo's workspace root and reserves the workspace engine.
    fn for_cargo_workspace(
        inner: &mut EngineRegistryInner,
        path: &Path,
        workspace_root: PathBuf,
        cache_policy: OpenFileCachePolicy,
    ) -> Option<Self> {
        let route = inner.reserve_workspace_root(workspace_root)?;

        if cache_policy.should_record() {
            inner.set_open_file(path.to_path_buf(), route.id());
        }

        Some(Self {
            route,
            source: DocumentOwnerSource::CargoWorkspace,
        })
    }

    /// Falls back to the last active ready engine for files outside known workspaces.
    fn fallback(
        inner: &mut EngineRegistryInner,
        path: &Path,
        cache_policy: OpenFileCachePolicy,
    ) -> Option<Self> {
        let id = inner.active_ready_id()?;

        if cache_policy.should_record() {
            inner.set_open_file(path.to_path_buf(), id);
        }

        Some(Self::existing(id, DocumentOwnerSource::ActiveFallback))
    }

    pub(super) fn id(&self) -> EngineId {
        self.route.id()
    }

    pub(super) fn source(&self) -> DocumentOwnerSource {
        self.source
    }

    pub(super) fn into_route(self) -> ReservedEngineRoute {
        self.route
    }

    fn existing(id: EngineId, source: DocumentOwnerSource) -> Self {
        Self {
            route: ReservedEngineRoute::Existing(id),
            source,
        }
    }

    fn locate_workspace_root(
        path: &Path,
        discovery_workspace: &Path,
    ) -> anyhow::Result<Option<PathBuf>> {
        let path = normalize_path(path);
        let discovery_workspace = normalize_path(discovery_workspace);
        let Some(document_dir) = path
            .is_dir()
            .then(|| path.to_path_buf())
            .or_else(|| path.parent().map(Path::to_path_buf))
        else {
            return Ok(None);
        };
        let Some(manifest_path) = Self::nearest_manifest(&document_dir, &discovery_workspace)
        else {
            return Ok(None);
        };

        let output =
            Self::run_locate_project(&discovery_workspace, &manifest_path).with_context(|| {
                format!(
                    "while attempting to locate Cargo workspace from {}",
                    manifest_path.display()
                )
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if stderr.contains("could not find `Cargo.toml`") {
                return Ok(None);
            }

            tracing::warn!(
                cwd = %discovery_workspace.display(),
                manifest_path = %manifest_path.display(),
                status = %output.status,
                stderr = %stderr,
                "cargo locate-project failed; falling back to active engine"
            );
            return Ok(None);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let workspace_manifest = stdout
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(PathBuf::from)
            .with_context(|| {
                format!(
                    "while attempting to read Cargo workspace manifest from locate-project output in {}",
                    manifest_path.display()
                )
            })?;
        let workspace_manifest = if workspace_manifest.is_absolute() {
            workspace_manifest
        } else {
            discovery_workspace.join(workspace_manifest)
        };
        let workspace_manifest = workspace_manifest.canonicalize().with_context(|| {
            format!(
                "while attempting to canonicalize Cargo workspace manifest {}",
                workspace_manifest.display()
            )
        })?;

        Ok(Some(
            workspace_manifest
                .parent()
                .expect("Cargo workspace manifest path should have a parent directory")
                .to_path_buf(),
        ))
    }

    fn run_locate_project(cwd: &Path, manifest_path: &Path) -> std::io::Result<Output> {
        Command::new("cargo")
            .current_dir(cwd)
            .arg("locate-project")
            .arg("--workspace")
            .arg("--message-format")
            .arg("plain")
            .arg("--manifest-path")
            .arg(manifest_path)
            .output()
    }

    fn nearest_manifest(document_dir: &Path, discovery_workspace: &Path) -> Option<PathBuf> {
        let mut current = document_dir;
        while current.starts_with(discovery_workspace) {
            let manifest_path = current.join("Cargo.toml");
            if manifest_path.is_file() {
                return Some(manifest_path);
            }

            current = current.parent()?;
        }

        None
    }
}

/// Explains which rule selected a document owner, mostly for tracing and tests.
#[derive(Clone, Copy, Debug)]
pub(super) enum DocumentOwnerSource {
    OpenFileCache,
    CargoWorkspace,
    ActiveFallback,
}

/// Controls whether a resolved document owner should be remembered until didClose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OpenFileCachePolicy {
    /// Remember resolved ownership for an opened document.
    Record,
    /// Route without changing the open-file cache.
    Ignore,
    /// Forget an opened document and use only its remembered owner.
    Remove,
}

impl OpenFileCachePolicy {
    pub(super) fn should_record(self) -> bool {
        self == Self::Record
    }
}

#[cfg(test)]
mod tests {
    use test_fixture::fixture_crate;

    use crate::engine_registry::routing::normalize_path;

    use super::DocumentOwner;

    #[test]
    fn nearest_manifest_walks_up_from_document_directory() {
        let fixture = fixture_crate(
            r#"
//- /workspace/Cargo.toml
[workspace]
members = ["crates/app"]
resolver = "3"

//- /workspace/crates/app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /workspace/crates/app/src/nested/module.rs
pub struct App;
"#,
        );
        let document_dir = normalize_path(fixture.path("workspace/crates/app/src/nested"));
        let workspace = normalize_path(fixture.path("workspace"));

        let manifest = DocumentOwner::nearest_manifest(&document_dir, &workspace)
            .expect("nested source directory should resolve its package manifest");

        assert_eq!(
            manifest,
            normalize_path(fixture.path("workspace/crates/app/Cargo.toml"))
        );
    }
}
