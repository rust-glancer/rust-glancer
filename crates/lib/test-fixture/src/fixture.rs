use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

use tempfile::TempDir;

/// Parsed fixture files and source markers before they are materialized on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureSpec {
    files: Vec<FixtureFile>,
    markers: FixtureMarkers,
}

impl FixtureSpec {
    pub fn parse(spec: &str) -> Self {
        let spec = Self::trim_fixture_indent(spec);
        let mut files = Vec::new();
        let mut markers = FixtureMarkers::default();
        let mut current_path = None::<String>;
        let mut current_metadata = String::new();
        let mut current_contents = String::new();
        let mut current_offset = 0_u32;

        for line in spec.lines() {
            if let Some(header) = line.strip_prefix("//- ") {
                if let Some(path) = current_path.take() {
                    files.push(FixtureFile {
                        relative_path: path,
                        contents: current_contents,
                        metadata: current_metadata,
                    });
                    current_contents = String::new();
                }

                let (path, metadata) = Self::parse_fixture_header(header);
                current_path = Some(path);
                current_metadata = metadata;
                current_offset = 0;
                continue;
            }

            let Some(path) = &current_path else {
                if line.trim().is_empty() {
                    continue;
                }

                panic!("fixture content must start with `//- /path`; found `{line}`");
            };

            let cleaned = Self::strip_markers_from_line(line, path, current_offset, &mut markers);
            current_offset +=
                u32::try_from(cleaned.len() + 1).expect("fixture line length should fit into u32");
            current_contents.push_str(&cleaned);
            current_contents.push('\n');
        }

        if let Some(path) = current_path {
            files.push(FixtureFile {
                relative_path: path,
                contents: current_contents,
                metadata: current_metadata,
            });
        }

        assert!(
            !files.is_empty(),
            "fixture specification should contain at least one `//- /path` header"
        );

        Self { files, markers }
    }

    pub fn files(&self) -> &[FixtureFile] {
        &self.files
    }

    pub fn markers(&self) -> &FixtureMarkers {
        &self.markers
    }

    fn into_files(self) -> Vec<FixtureFile> {
        self.files
    }

    pub(crate) fn strip_markers_from_line(
        line: &str,
        path: &str,
        line_offset: u32,
        markers: &mut FixtureMarkers,
    ) -> String {
        let mut cleaned = String::new();
        let mut idx = 0;

        while idx < line.len() {
            let rest = &line[idx..];
            if let Some(stripped) = rest.strip_prefix(r"\$")
                && let Some((_, length)) = Self::take_marker(stripped)
            {
                cleaned.push('$');
                cleaned.push_str(&stripped[..length]);
                idx += length + 2;
                continue;
            }

            if let Some(stripped) = rest.strip_prefix('$')
                && let Some((name, length)) = Self::take_marker(stripped)
            {
                markers.push(
                    name,
                    FixtureMarker {
                        path: path.to_string(),
                        offset: line_offset
                            + u32::try_from(cleaned.len())
                                .expect("fixture line length should fit into u32"),
                    },
                );
                idx += length + 1;
                continue;
            }

            let ch = rest
                .chars()
                .next()
                .expect("non-empty line rest should have a first char");
            cleaned.push(ch);
            idx += ch.len_utf8();
        }

        cleaned
    }

    // After the opening `$`, digits form a short marker such as `$0` or `$11`. Read all
    // digits, and accept a closing `$` so `1$0$2` can put a cursor between source digits.
    // Named markers still need their closing `$`, as in `$method$`.
    fn take_marker(text: &str) -> Option<(&str, usize)> {
        let digits = text.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            let length = digits + usize::from(text[digits..].starts_with('$'));
            return Some((&text[..digits], length));
        }

        let end = text.find('$')?;
        let name = &text[..end];
        let mut chars = name.chars();
        let first = chars.next()?;
        if !(first == '_' || first.is_ascii_alphabetic()) {
            return None;
        }
        if chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric()) {
            Some((name, end + 1))
        } else {
            None
        }
    }

    fn parse_fixture_header(header: &str) -> (String, String) {
        let (path, metadata) = header
            .split_once(char::is_whitespace)
            .unwrap_or((header, ""));
        assert!(
            path.starts_with('/'),
            "fixture path should start with `/`: {path}"
        );

        let relative_path = path.trim_start_matches('/');
        assert!(
            !relative_path.is_empty(),
            "fixture path should not be empty"
        );
        (relative_path.to_string(), metadata.trim().to_string())
    }

    fn trim_fixture_indent(spec: &str) -> String {
        let spec = spec.strip_prefix('\n').unwrap_or(spec);
        let min_indent = spec
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(Self::leading_indent)
            .min()
            .unwrap_or(0);

        let mut trimmed = String::new();

        for (idx, line) in spec.lines().enumerate() {
            if idx > 0 {
                trimmed.push('\n');
            }

            if line.trim().is_empty() {
                continue;
            }

            trimmed.push_str(&line[min_indent..]);
        }

        trimmed
    }

    fn leading_indent(line: &str) -> usize {
        line.as_bytes()
            .iter()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureFile {
    relative_path: String,
    pub(crate) contents: String,
    metadata: String,
}

impl FixtureFile {
    pub fn relative_path(&self) -> &str {
        &self.relative_path
    }

    pub fn contents(&self) -> &str {
        &self.contents
    }

    /// Header declarations belong to the consuming suite; file and marker parsing preserves them.
    pub fn metadata(&self) -> &str {
        &self.metadata
    }
}

/// Source marker metadata stripped from a parsed fixture.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FixtureMarkers {
    markers: BTreeMap<String, Vec<FixtureMarker>>,
}

impl FixtureMarkers {
    /// Visits every marker occurrence. Sort by offset when source order matters within a file.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &FixtureMarker)> {
        self.markers.iter().flat_map(|(name, positions)| {
            positions
                .iter()
                .map(move |position| (name.as_str(), position))
        })
    }

    /// Returns one cursor marker by name.
    ///
    /// `$11` and `$11$` both have name `"11"`. Named markers use `$name$`.
    /// Names must be unique across the fixture for this lookup.
    pub fn position(&self, name: &str) -> &FixtureMarker {
        let positions = self
            .markers
            .get(name)
            .unwrap_or_else(|| panic!("marker `{name}` should exist in fixture"));
        assert_eq!(
            positions.len(),
            1,
            "marker `{name}` should appear exactly once for an offset query"
        );

        &positions[0]
    }

    fn push(&mut self, name: impl Into<String>, marker: FixtureMarker) {
        self.markers.entry(name.into()).or_default().push(marker);
    }
}

impl FromIterator<(String, FixtureMarker)> for FixtureMarkers {
    fn from_iter<T: IntoIterator<Item = (String, FixtureMarker)>>(iter: T) -> Self {
        let mut markers = Self::default();
        for (name, marker) in iter {
            markers.push(name, marker);
        }
        markers
    }
}

/// One stripped source marker position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureMarker {
    pub path: String,
    pub offset: u32,
}

/// Renders a fixture-relative path in the portable form used by test snapshots.
///
/// Fixture paths still use host-native `Path` values for filesystem work. Snapshots use `/`
/// between structural components so the same expected output can be shared by every host.
pub fn fixture_path_for_snapshot(path: &Path) -> String {
    assert!(
        !path.is_absolute()
            && !path.has_root()
            && !path
                .components()
                .any(|component| matches!(component, Component::Prefix(_))),
        "snapshot fixture path should be relative: {}",
        path.display(),
    );

    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Creates temporary on-disk crate fixtures from inline file contents.
///
/// Files stay on disk so module discovery follows ordinary filesystem rules. Suites can either
/// declare their workspace in file headers or load a real Cargo project through `metadata()`.
pub struct CrateFixture {
    root: TempDir,
    headers: Vec<(String, String)>,
}

impl CrateFixture {
    /// Materializes a crate fixture from the following syntax (inspired by rust-analyzer):
    ///
    /// ```text
    /// //- /Cargo.toml
    /// [package]
    /// name = "demo"
    ///
    /// //- /src/lib.rs
    /// pub fn work() {}
    /// ```
    ///
    /// Header declarations such as `crate:demo` are retained for suites that construct workspace
    /// metadata directly. Cargo-backed fixtures can keep ordinary manifest files instead.
    pub fn from_fixture_spec(spec: &str) -> Self {
        Self::from_parsed_fixture(FixtureSpec::parse(spec))
    }

    /// Adds rust-glancer's small shared fake sysroot under `/sysroot/library`.
    ///
    /// The fake sysroot contains every compiler-provided crate root expected by discovery, but
    /// exposes only the small API surface needed by name resolution and type inference tests. For
    /// the exact list of modeled APIs, check `assets/fake_sysroot`.
    ///
    /// This method panics if the fixture already defines one of the shared fake sysroot files.
    /// Tests that need a custom sysroot should keep using explicit `/sysroot/library/...` fixture
    /// files instead of silently mixing the two worlds.
    pub fn with_fake_sysroot(self) -> Self {
        for file in crate::fake_sysroot::files() {
            let path = self.path(file.relative_path);
            assert!(
                !path.exists(),
                "fixture already defines fake sysroot file `{}`",
                file.relative_path
            );
            self.write_file(file.relative_path, file.contents);
        }

        self
    }

    fn from_parsed_fixture(fixture: FixtureSpec) -> Self {
        Self::from_files(fixture.into_files())
    }

    /// Materializes already-parsed files without interpreting source markers a second time.
    pub fn from_files(files: impl IntoIterator<Item = FixtureFile>) -> Self {
        let root = Self::create_root_directory();
        let mut fixture = Self {
            root,
            headers: Vec::new(),
        };

        // Parsing reads these files through the filesystem. Retain only their declarations,
        // rather than keeping another copy of every Rust source string in the fixture owner.
        for file in files {
            fixture.write_file(file.relative_path(), file.contents());
            if !file.metadata.is_empty() {
                fixture.headers.push((file.relative_path, file.metadata));
            }
        }

        fixture
    }

    /// Returns annotated file headers; ordinary source-file headers have no declarations.
    pub fn headers(&self) -> impl Iterator<Item = (&str, &str)> {
        self.headers
            .iter()
            .map(|(path, metadata)| (path.as_str(), metadata.as_str()))
    }

    /// Writes one or more fixture files into this crate root and returns the parsed file set.
    ///
    /// This is intentionally generic filesystem materialization, not an analysis update helper.
    /// Higher-level crates can interpret the returned file contents as save events, VFS changes,
    /// generated outputs, or any other domain-specific action they need to test.
    pub fn write_fixture_files(&self, spec: &str) -> FixtureSpec {
        let fixture = FixtureSpec::parse(spec);

        for file in fixture.files() {
            self.write_file(file.relative_path(), file.contents());
        }

        fixture
    }

    fn write_file(&self, relative_path: &str, contents: &str) {
        let path = self.root.path().join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("fixture directories should be created");
        }
        fs::write(path, contents).expect("fixture file should be written");
    }

    /// Resolves a relative path within the fixture root.
    pub fn path(&self, relative_path: &str) -> PathBuf {
        self.root.path().join(relative_path)
    }

    /// Loads cargo metadata for the fixture crate.
    pub fn metadata(&self) -> cargo_metadata::Metadata {
        cargo_metadata::MetadataCommand::new()
            .manifest_path(self.manifest_path())
            .exec()
            .expect("fixture metadata should load")
    }

    /// Returns the package described by the fixture's root manifest.
    pub fn package(&self) -> cargo_metadata::Package {
        let metadata = self.metadata();

        metadata
            .root_package()
            .cloned()
            .or_else(|| metadata.workspace_packages().into_iter().next().cloned())
            .expect("fixture package should be present in metadata")
    }

    fn manifest_path(&self) -> PathBuf {
        self.path("Cargo.toml")
    }

    fn create_root_directory() -> TempDir {
        tempfile::Builder::new()
            .prefix("rust-glancer-test-fixture-")
            .tempdir()
            .expect("fixture root directory should be created")
    }
}

pub fn fixture_crate(fixture: &str) -> CrateFixture {
    CrateFixture::from_fixture_spec(fixture)
}

pub fn fixture_crate_with_markers(fixture: &str) -> (CrateFixture, FixtureMarkers) {
    let parsed = FixtureSpec::parse(fixture);
    let markers = parsed.markers().clone();
    (CrateFixture::from_parsed_fixture(parsed), markers)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::{FixtureSpec, fixture_path_for_snapshot};

    const MINIMAL_CRATE: &str = r#"
//- /Cargo.toml
[package]
name = "fake_sysroot_owner"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Root;
"#;

    #[test]
    fn renders_fixture_paths_with_snapshot_separators() {
        let path = PathBuf::from("crates")
            .join("app")
            .join("src")
            .join("lib.rs");

        assert_eq!(fixture_path_for_snapshot(&path), "crates/app/src/lib.rs");
    }

    #[test]
    fn strips_shared_source_markers_from_fixture_files() {
        let fixture = FixtureSpec::parse(
            r#"
//- /Cargo.toml
[package]
name = "marker_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it(user: User) {
    let local = loc$goto$al;
    user.$0id();
    let escaped = "\$0 and \$name$";
}
"#,
        );
        let source = fixture
            .files()
            .iter()
            .find(|file| file.relative_path() == "src/lib.rs")
            .expect("source file should exist in parsed fixture");

        assert!(source.contents().contains("let local = local;"));
        assert!(source.contents().contains("user.id();"));
        assert!(source.contents().contains(r#""$0 and $name$""#));

        let goto = fixture.markers.position("goto");
        assert_eq!(goto.path, "src/lib.rs");
        assert_eq!(
            goto.offset,
            u32::try_from(
                source
                    .contents()
                    .find("local;")
                    .expect("local should be present")
                    + 3
            )
            .expect("fixture offset should fit into u32")
        );

        let cursor = fixture.markers.position("0");
        assert_eq!(cursor.path, "src/lib.rs");
        assert_eq!(
            cursor.offset,
            u32::try_from(
                source
                    .contents()
                    .find("id();")
                    .expect("method name should be present")
            )
            .expect("fixture offset should fit into u32")
        );
    }

    #[test]
    fn strips_numeric_and_named_markers_with_exact_byte_offsets() {
        let cases = [
            ("a$0b$1c$11d", "abcd", &[("0", 1), ("1", 2), ("11", 3)][..]),
            ("1$0$2 + 3$1$4", "12 + 34", &[("0", 1), ("1", 6)]),
            ("α$10β$11$γ", "αβγ", &[("10", 2), ("11", 4)]),
            ("a$named$b$11c", "abc", &[("named", 1), ("11", 2)]),
            (r"\$0 \$1 \$11 \$2$ \$named$", "$0 $1 $11 $2$ $named$", &[]),
        ];

        for (marked, expected, positions) in cases {
            let fixture = FixtureSpec::parse(&format!("//- /src/lib.rs\n{marked}\n"));
            assert_eq!(
                fixture.files()[0].contents(),
                format!("{expected}\n"),
                "{marked}"
            );
            assert_eq!(
                fixture.markers().iter().count(),
                positions.len(),
                "{marked}"
            );
            for (name, offset) in positions {
                let marker = fixture.markers().position(name);
                assert_eq!(marker.path, "src/lib.rs", "{marked}: {name}");
                assert_eq!(marker.offset, *offset, "{marked}: {name}");
            }
        }
    }

    #[test]
    fn writes_fixture_files_into_existing_crate_root() {
        let fixture = crate::fixture_crate(
            r#"
//- /Cargo.toml
[package]
name = "write_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Root;
"#,
        );

        let saved = fixture.write_fixture_files(
            r#"
//- /src/api.rs
pub struct Api;
"#,
        );

        assert_eq!(saved.files()[0].relative_path(), "src/api.rs");
        assert_eq!(
            std::fs::read_to_string(fixture.path("src/api.rs"))
                .expect("written fixture file should be readable"),
            "pub struct Api;\n"
        );
    }

    #[test]
    fn materializes_shared_fake_sysroot_files() {
        let fixture = crate::fixture_crate(MINIMAL_CRATE).with_fake_sysroot();

        for crate_name in ["core", "alloc", "std", "proc_macro"] {
            assert!(
                fixture
                    .path(&format!("sysroot/library/{crate_name}/src/lib.rs"))
                    .is_file(),
                "shared fake sysroot should materialize the `{crate_name}` crate root"
            );
        }

        assert!(
            fixture
                .path("sysroot/library/core/src/macros/mod.rs")
                .is_file(),
            "shared fake sysroot should materialize the core macro module"
        );
    }

    #[test]
    #[should_panic(expected = "fixture already defines fake sysroot file")]
    fn rejects_shared_fake_sysroot_file_collisions() {
        crate::fixture_crate(
            r#"
//- /Cargo.toml
[package]
name = "fake_sysroot_collision"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Root;

//- /sysroot/library/core/src/lib.rs
pub struct CustomCore;
"#,
        )
        .with_fake_sysroot();
    }
}
