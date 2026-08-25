//! Filesystem paths whose spelling is suitable for identity comparisons.

use std::{
    ffi::OsString,
    io,
    path::{Component, Display, Path, PathBuf},
};

use crate::{MemoryRecorder, MemorySize};

/// An absolute path with one normalized spelling on the current host.
///
/// Existing components are resolved through the filesystem. A path that does not exist yet is
/// anchored at its nearest existing ancestor, so it still compares consistently with paths
/// reported by tools that canonicalize their output.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NormalizedPathBuf(PathBuf);

impl NormalizedPathBuf {
    /// Resolves an absolute external path into its identity-bearing spelling.
    pub fn from_absolute(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("expected an absolute path, got `{}`", path.display()),
            ));
        }

        Self::canonicalize_existing_ancestor(path)
            .map(Self)
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("failed to normalize `{}`: {error}", path.display()),
                )
            })
    }

    /// Resolves a portable relative path against an explicit normalized base.
    ///
    /// Fully absolute inputs are accepted as-is. Windows root-relative (`\foo`) and drive-relative
    /// (`C:foo`) paths are rejected because their result depends on process-global drive state.
    pub fn resolve_from(base: &Self, path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        if path.is_absolute() {
            return Self::from_absolute(path);
        }

        let has_prefix = path
            .components()
            .any(|component| matches!(component, Component::Prefix(_)));
        if path.has_root() || has_prefix {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "expected an absolute path or an ordinary relative path, got `{}`",
                    path.display()
                ),
            ));
        }

        Self::from_absolute(base.as_path().join(path))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }

    pub fn to_path_buf(&self) -> PathBuf {
        self.0.clone()
    }

    pub fn display(&self) -> Display<'_> {
        self.0.display()
    }

    pub fn parent(&self) -> Option<Self> {
        self.0.parent().map(|parent| Self(parent.to_path_buf()))
    }

    pub fn starts_with(&self, base: &Self) -> bool {
        self.0.starts_with(&base.0)
    }

    /// Canonicalizes as much of the path as exists, then restores the missing suffix. Only
    /// `NotFound` is treated as evidence that an ancestor should be tried; permission and invalid
    /// path errors must stay visible to the caller. The path is passed to the filesystem unchanged,
    /// so `..` keeps its normal meaning across symlinks and Windows junctions.
    fn canonicalize_existing_ancestor(path: &Path) -> io::Result<PathBuf> {
        let mut ancestor = path;
        let mut missing_suffix: Vec<OsString> = Vec::new();

        loop {
            match std::fs::canonicalize(ancestor) {
                Ok(mut canonical) => {
                    for component in missing_suffix.iter().rev() {
                        canonical.push(component);
                    }
                    return Ok(canonical);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    let Some(file_name) = ancestor.file_name() else {
                        return Err(error);
                    };
                    missing_suffix.push(file_name.to_owned());

                    let Some(parent) = ancestor.parent() else {
                        return Err(error);
                    };
                    ancestor = parent;
                }
                Err(error) => return Err(error),
            }
        }
    }
}

impl AsRef<Path> for NormalizedPathBuf {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl MemorySize for NormalizedPathBuf {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        self.0.record_memory_children(recorder);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::NormalizedPathBuf;

    static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

    struct PathFixture(PathBuf);

    impl PathFixture {
        fn new() -> Self {
            let id = NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "rust-glancer-normalized-path-{}-{id}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("test fixture directory should be created");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for PathFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn resolves_existing_and_missing_paths_to_one_identity() {
        let fixture = PathFixture::new();
        let nested = fixture.path().join("nested");
        fs::create_dir(&nested).expect("nested fixture directory should be created");
        fs::create_dir(nested.join("child"))
            .expect("nested child fixture directory should be created");
        fs::create_dir_all(fixture.path().join("src/child"))
            .expect("source fixture directories should be created");
        let file = nested.join("file.rs");
        fs::write(&file, "fn main() {}\n").expect("fixture source should be written");

        let base = NormalizedPathBuf::from_absolute(fixture.path())
            .expect("fixture root should be normalized");
        let absolute =
            NormalizedPathBuf::from_absolute(&file).expect("absolute file should be normalized");

        let separator = std::path::MAIN_SEPARATOR;
        let cases = [
            PathBuf::from("nested").join("file.rs"),
            PathBuf::from("nested").join(".").join("file.rs"),
            PathBuf::from("nested")
                .join("child")
                .join("..")
                .join("file.rs"),
            PathBuf::from(format!("nested{separator}{separator}file.rs")),
        ];

        for relative in cases {
            let actual = NormalizedPathBuf::resolve_from(&base, &relative)
                .expect("relative file should be normalized");
            assert_eq!(actual, absolute, "failed for `{}`", relative.display());
        }

        let missing = NormalizedPathBuf::resolve_from(&base, "src/child/../generated.rs")
            .expect("missing descendant should be normalized");
        assert_eq!(missing.as_path(), base.as_path().join("src/generated.rs"));
    }

    #[test]
    fn absolute_constructor_matches_host_canonicalization() {
        let fixture = PathFixture::new();
        let expected = fs::canonicalize(fixture.path()).expect("fixture root should exist");
        let actual = NormalizedPathBuf::from_absolute(fixture.path())
            .expect("fixture root should be normalized");

        assert_eq!(actual.as_path(), expected);
    }

    #[test]
    fn rejects_relative_input_without_a_base() {
        let error = NormalizedPathBuf::from_absolute("src/lib.rs")
            .expect_err("relative input should be rejected");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[cfg(windows)]
    #[test]
    fn canonical_and_conventional_windows_paths_have_one_identity() {
        let fixture = PathFixture::new();
        let canonical = fs::canonicalize(fixture.path()).expect("fixture root should exist");

        let conventional = NormalizedPathBuf::from_absolute(fixture.path())
            .expect("conventional path should be normalized");
        let canonical = NormalizedPathBuf::from_absolute(canonical)
            .expect("canonical path should remain normalized");

        assert_eq!(conventional, canonical);
    }

    #[cfg(windows)]
    #[test]
    fn rejects_ambiguous_windows_relative_forms() {
        let fixture = PathFixture::new();
        let base = NormalizedPathBuf::from_absolute(fixture.path())
            .expect("fixture root should be normalized");

        for path in [Path::new(r"\src\lib.rs"), Path::new(r"C:src\lib.rs")] {
            let error = NormalizedPathBuf::resolve_from(&base, path)
                .expect_err("ambiguous Windows path should be rejected");
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        }
    }
}
