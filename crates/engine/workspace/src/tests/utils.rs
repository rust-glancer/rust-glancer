use std::{fmt::Write as _, path::Path};

use expect_test::Expect;

use crate::{PackageOrigin, SysrootSources, WorkspaceMetadata};
use test_fixture::fixture_crate;

pub(super) fn check_workspace_metadata(fixture: &str, expect: Expect) {
    let fixture = fixture_crate(fixture);
    let actual = render_workspace_metadata(
        &WorkspaceMetadata::from_cargo(fixture.metadata())
            .expect("fixture workspace metadata should build"),
    );
    let actual = format!("{}\n", actual.trim_end());
    expect.assert_eq(&actual);
}

pub(super) fn check_workspace_metadata_with_sysroot(fixture: &str, expect: Expect) {
    let fixture = fixture_crate(fixture);
    let sysroot = SysrootSources::from_library_root(fixture.path("sysroot/library"))
        .expect("fixture sysroot should be complete");
    let workspace = WorkspaceMetadata::from_cargo(fixture.metadata())
        .expect("fixture workspace metadata should build")
        .with_sysroot_sources(Some(sysroot));
    let actual = render_workspace_metadata(&workspace);
    let actual = format!("{}\n", actual.trim_end());
    expect.assert_eq(&actual);
}

fn render_workspace_metadata(workspace: &WorkspaceMetadata) -> String {
    let mut dump = String::new();
    writeln!(&mut dump, "workspace .").expect("string writes should not fail");

    let mut packages = workspace.packages().iter().collect::<Vec<_>>();
    packages.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.manifest_path.cmp(&right.manifest_path))
    });

    for package in packages {
        writeln!(&mut dump).expect("string writes should not fail");
        render_package(workspace, package, &mut dump);
    }

    dump
}

fn render_package(workspace: &WorkspaceMetadata, package: &crate::Package, dump: &mut String) {
    let membership = match &package.origin {
        PackageOrigin::Workspace => "member",
        PackageOrigin::Dependency => "dependency",
        PackageOrigin::Sysroot(_) => "sysroot",
    };
    writeln!(dump, "package {} [{membership}]", package.name)
        .expect("string writes should not fail");
    writeln!(
        dump,
        "manifest {}",
        relative_path(workspace.workspace_root(), &package.manifest_path)
    )
    .expect("string writes should not fail");
    writeln!(dump, "source {}", package.source).expect("string writes should not fail");
    writeln!(dump, "edition {}", package.edition).expect("string writes should not fail");
    writeln!(dump, "targets").expect("string writes should not fail");

    let mut targets = package.targets.iter().collect::<Vec<_>>();
    targets.sort_by(|left, right| {
        (
            left.kind.sort_order(),
            left.name.as_str(),
            left.src_path.as_path(),
        )
            .cmp(&(
                right.kind.sort_order(),
                right.name.as_str(),
                right.src_path.as_path(),
            ))
    });

    for target in targets {
        writeln!(
            dump,
            "- {} [{}] {}",
            target.name,
            target.kind,
            relative_path(workspace.workspace_root(), &target.src_path),
        )
        .expect("string writes should not fail");
    }

    writeln!(dump, "dependencies").expect("string writes should not fail");

    if package.dependencies.is_empty() {
        writeln!(dump, "- <none>").expect("string writes should not fail");
        return;
    }

    let mut dependencies = package
        .dependencies
        .iter()
        .map(|dependency| {
            let package_name = workspace
                .package(dependency.package_id())
                .map(|package| package.name.as_str())
                .unwrap_or("<missing>");
            (dependency, package_name)
        })
        .collect::<Vec<_>>();
    dependencies.sort_by(|left, right| {
        (
            left.0.name(),
            left.1,
            left.0.is_normal(),
            left.0.is_build(),
            left.0.is_dev(),
        )
            .cmp(&(
                right.0.name(),
                right.1,
                right.0.is_normal(),
                right.0.is_build(),
                right.0.is_dev(),
            ))
    });

    for (dependency, package_name) in dependencies {
        let kind_label = render_dependency_kinds(dependency);
        writeln!(
            dump,
            "- {} -> {}{}",
            dependency.name(),
            package_name,
            kind_label
        )
        .expect("string writes should not fail");
    }
}

fn render_dependency_kinds(dependency: &crate::PackageDependency) -> String {
    let mut kinds = Vec::new();

    if dependency.is_normal() {
        kinds.push("normal");
    }
    if dependency.is_build() {
        kinds.push("build");
    }
    if dependency.is_dev() {
        kinds.push("dev");
    }

    if kinds == ["normal"] {
        String::new()
    } else {
        format!(" [{}]", kinds.join(", "))
    }
}

fn relative_path(root: &Path, path: &Path) -> String {
    let relative_path = path.strip_prefix(root).unwrap_or(path);

    if relative_path.as_os_str().is_empty() {
        ".".to_string()
    } else {
        relative_path.display().to_string()
    }
}
