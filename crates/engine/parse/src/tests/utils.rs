use std::{fmt::Write as _, path::Path};

use expect_test::Expect;
use rg_workspace::WorkspaceMetadata;
use test_fixture::fixture_crate;

use crate::{Package, ParseDb, Target};

/// Builds the named test fixture crate, parses it with `ParseDb` using `RootsOnly` mode,
/// renders a project parse snapshot, and asserts the rendered output equals `expect`.
///
/// `fixture` is the path or name of the fixture crate (relative to the test fixtures root).
/// `expect` is an `insta::Expect` that contains the expected snapshot text.
///
/// # Examples
///
/// ```
/// // inside a test
/// use insta::assert_display_snapshot;
/// // `expect!` is provided by the insta crate's macro for inline snapshots
/// check_parse_db("basic_crate", expect![[r#"
/// packages: 1 workspace member, 0 dependencies
/// ...
/// "#]]);
/// ```
pub(super) fn check_parse_db(fixture: &str, expect: Expect) {
    check_parse_db_with(fixture, ParseFixtureMode::RootsOnly, expect);
}

/// Builds the test fixture, runs module discovery for each parsed workspace package, renders a parse snapshot, and asserts it matches `expect`.
///
/// This triggers `ParseFixtureMode::DiscoverModules` so packages will run module discovery before the snapshot is produced.
///
/// # Examples
///
/// ```
/// // Assert the parsed project (after module discovery) matches the stored snapshot.
/// check_parse_db_after_module_discovery("my_fixture", expect![[r#"
/// packages: 1 workspace members: 1 dependencies: 0
/// - my_crate (member)
///   targets:
///     - my_crate lib src/lib.rs
///   files:
///     - src/lib.rs
/// "#]]);
/// ```
pub(super) fn check_parse_db_after_module_discovery(fixture: &str, expect: Expect) {
    check_parse_db_with(fixture, ParseFixtureMode::DiscoverModules, expect);
}

/// Builds a parse database from a test fixture, optionally runs module discovery, and asserts the rendered project snapshot matches `expect`.
///
/// This helper:
/// - creates the fixture crate and canonicalizes its root paths,
/// - constructs `WorkspaceMetadata` from the fixture's Cargo metadata and builds a `ParseDb`,
/// - if `mode` is `ParseFixtureMode::DiscoverModules`, runs `discover_modules()` for each mutable package,
/// - renders a `ProjectParseSnapshot` and compares the sanitized output against `expect`.
///
/// # Examples
///
/// ```no_run
/// # use crate::tests::utils::{check_parse_db_with, ParseFixtureMode};
/// # use expect_test::Expect;
/// # fn demo() {
/// check_parse_db_with("my_fixture", ParseFixtureMode::RootsOnly, Expect::new());
/// # }
/// ```
fn check_parse_db_with(fixture: &str, mode: ParseFixtureMode, expect: Expect) {
    let fixture = fixture_crate(fixture);
    let root = fixture
        .path("")
        .canonicalize()
        .expect("fixture root should be canonicalizable");
    let display_root = fixture.path("");
    let workspace = WorkspaceMetadata::from_cargo(fixture.metadata())
        .expect("fixture workspace metadata should build");
    let mut parse = ParseDb::build(&workspace).expect("fixture parse db should build");
    if matches!(mode, ParseFixtureMode::DiscoverModules) {
        for package in parse.packages_mut() {
            package
                .discover_modules()
                .expect("fixture module discovery should succeed");
        }
    }
    let actual = ProjectParseSnapshot::new(&parse, &root, &display_root).render();
    let actual = format!("{}\n", actual.trim_end());
    expect.assert_eq(&actual);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParseFixtureMode {
    RootsOnly,
    DiscoverModules,
}

struct ProjectParseSnapshot<'a> {
    parse: &'a ParseDb,
    root: &'a Path,
    display_root: &'a Path,
}

impl<'a> ProjectParseSnapshot<'a> {
    fn new(parse: &'a ParseDb, root: &'a Path, display_root: &'a Path) -> Self {
        Self {
            parse,
            root,
            display_root,
        }
    }

    fn render(&self) -> String {
        let workspace_member_count = self.parse.workspace_packages().count();
        let dependency_count = self
            .parse
            .packages()
            .len()
            .saturating_sub(workspace_member_count);
        let mut dump = String::new();
        writeln!(
            &mut dump,
            "packages {} (workspace members: {}, dependencies: {})",
            self.parse.packages().len(),
            workspace_member_count,
            dependency_count,
        )
        .expect("string writes should not fail");

        for package in self.sorted_packages() {
            writeln!(&mut dump).expect("string writes should not fail");
            self.render_package(package, &mut dump);
        }

        dump
    }

    fn sorted_packages(&self) -> Vec<&Package> {
        let mut packages = self.parse.packages().iter().collect::<Vec<_>>();
        packages.sort_by(|left, right| {
            left.package_name()
                .cmp(right.package_name())
                .then_with(|| left.id().to_string().cmp(&right.id().to_string()))
        });
        packages
    }

    fn render_package(&self, package: &Package, dump: &mut String) {
        let membership = if package.is_workspace_member() {
            "member"
        } else {
            "dependency"
        };
        writeln!(dump, "package {} [{membership}]", package.package_name())
            .expect("string writes should not fail");

        writeln!(dump, "targets").expect("string writes should not fail");
        for target in Self::sorted_targets(package) {
            writeln!(
                dump,
                "- {} [{}] -> {}",
                target.name,
                target.kind,
                self.path_label(&target.src_path),
            )
            .expect("string writes should not fail");
        }

        writeln!(dump, "files").expect("string writes should not fail");
        let mut files = package.parsed_files().collect::<Vec<_>>();
        files.sort_by(|left, right| left.path().cmp(right.path()));
        for file in files {
            writeln!(dump, "- {}", self.path_label(file.path()))
                .expect("string writes should not fail");
        }
    }

    fn sorted_targets(package: &Package) -> Vec<&Target> {
        let mut targets = package.targets().iter().collect::<Vec<_>>();
        targets.sort_by(|left, right| {
            left.kind
                .sort_order()
                .cmp(&right.kind.sort_order())
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.src_path.cmp(&right.src_path))
        });
        targets
    }

    fn path_label(&self, path: &Path) -> String {
        if let Ok(relative) = path.strip_prefix(self.root) {
            return relative.display().to_string();
        }
        if let Ok(relative) = path.strip_prefix(self.display_root) {
            return relative.display().to_string();
        }
        if let Ok(canonical_path) = path.canonicalize() {
            if let Ok(relative) = canonical_path.strip_prefix(self.root) {
                return relative.display().to_string();
            }
        }

        path.display().to_string()
    }
}
