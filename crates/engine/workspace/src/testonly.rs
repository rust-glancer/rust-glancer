//! Build workspace metadata from declarations instead of asking Cargo to discover it.
//!
//! A crate root can declare `crate:app root:app deps:api edition:2024`. Other files need only
//! their paths. `root:` is the package directory relative to the fixture root; it defaults to
//! the fixture root itself. Several roots with the same `crate:` declare one package's targets.
//! `kind:bin name:tool` declares a named binary; the defaults are `lib` and edition 2024.
//! `targets:lib=app,bin=tool` declares several targets sharing the same source file.
//!
//! `deps:alias=package` declares a renamed dependency; `dev-deps:` supplies development edges.
//! `cfg:unix,feature=enabled` supplies active cfg facts, while `features:enabled,disabled` lists
//! feature names for completion. Packages are workspace members unless marked `origin:dependency`.

use std::{collections::BTreeMap, path::Path};

use rg_cfg_eval::CfgOptions;
use rg_text::RustEdition;

use crate::{
    CargoBuildOutputScanStats, CargoTarget, Package, PackageDependency, PackageId, PackageOrigin,
    TargetKind, WorkspaceMetadata,
};

impl WorkspaceMetadata {
    /// Keep the ordinary filesystem-backed parsing pipeline, but supply its workspace graph
    /// directly. Semantic tests should spend their time analyzing Rust, not resolving Cargo
    /// manifests. Tests of Cargo discovery still enter through `for_tests` or `lower`.
    pub fn from_fixture<'a>(
        root: &Path,
        headers: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Self {
        let root = root.canonicalize().expect("fixture root should exist");
        let target_cfg = CfgOptions::current_host();
        let mut packages = Vec::<Package>::new();
        let mut dependency_names = BTreeMap::<String, [String; 2]>::new();

        // Each annotated source file declares its targets. Group targets by package before
        // connecting dependencies, so a declaration can refer to a package written later.
        for (path, header) in headers {
            if header.is_empty() {
                continue;
            }
            let mut fields = BTreeMap::new();
            for field in header.split_whitespace() {
                let (key, value) = field
                    .split_once(':')
                    .expect("fixture declarations should use key:value");
                assert!(
                    matches!(
                        key,
                        "crate"
                            | "root"
                            | "kind"
                            | "name"
                            | "targets"
                            | "edition"
                            | "deps"
                            | "dev-deps"
                            | "cfg"
                            | "features"
                            | "origin"
                    ),
                    "unknown fixture declaration `{key}`"
                );
                assert!(
                    fields.insert(key, value).is_none(),
                    "duplicate `{key}` declaration"
                );
            }
            let name = *fields
                .get("crate")
                .expect("annotated root needs crate:name");
            let package_root = root.join(fields.get("root").copied().unwrap_or(""));
            let package_root = package_root
                .canonicalize()
                .expect("fixture package root should exist");
            let edition = match fields.get("edition").copied().unwrap_or("2024") {
                "2015" => RustEdition::Edition2015,
                "2018" => RustEdition::Edition2018,
                "2021" => RustEdition::Edition2021,
                "2024" => RustEdition::Edition2024,
                edition => panic!("unknown fixture edition `{edition}`"),
            };
            let target_specs = if let Some(targets) = fields.get("targets") {
                assert!(
                    !fields.contains_key("kind") && !fields.contains_key("name"),
                    "targets: supplies both target kinds and names"
                );
                targets
                    .split(',')
                    .map(|target| {
                        let (kind, name) = target
                            .split_once('=')
                            .expect("shared targets should use kind=name");
                        (kind, Some(name))
                    })
                    .collect::<Vec<_>>()
            } else {
                vec![(
                    fields.get("kind").copied().unwrap_or("lib"),
                    fields.get("name").copied(),
                )]
            };
            let source_path = root
                .join(path)
                .canonicalize()
                .expect("fixture target should exist");
            let targets = target_specs
                .into_iter()
                .map(|(kind, target_name)| {
                    let kind = match kind {
                        "lib" => TargetKind::Lib,
                        "bin" => TargetKind::Bin,
                        "proc-macro" => TargetKind::ProcMacro,
                        "example" => TargetKind::Example,
                        "test" => TargetKind::Test,
                        "bench" => TargetKind::Bench,
                        kind => panic!("unknown fixture target kind `{kind}`"),
                    };
                    let target_name = target_name.map_or_else(
                        || {
                            if kind.is_lib() {
                                name.replace('-', "_")
                            } else {
                                name.to_string()
                            }
                        },
                        str::to_string,
                    );
                    CargoTarget {
                        name: target_name,
                        kind,
                        src_path: source_path.clone(),
                    }
                })
                .collect::<Vec<_>>();
            let origin = match fields.get("origin").copied().unwrap_or("workspace") {
                "workspace" => PackageOrigin::Workspace,
                "dependency" => PackageOrigin::Dependency,
                origin => panic!("unknown fixture package origin `{origin}`"),
            };
            let mut cfg_options = target_cfg.clone();
            for cfg in fields.get("cfg").into_iter().flat_map(|cfg| cfg.split(',')) {
                if let Some((key, value)) = cfg.split_once('=') {
                    cfg_options.insert_key_value(key, value);
                } else {
                    cfg_options.insert_atom(cfg);
                }
            }
            let mut declared_features = fields
                .get("features")
                .into_iter()
                .flat_map(|features| features.split(',').map(str::to_string))
                .collect::<Vec<_>>();
            declared_features.sort();
            let deps =
                ["deps", "dev-deps"].map(|key| fields.get(key).copied().unwrap_or("").to_string());
            if let Some(previous) = dependency_names.insert(name.to_string(), deps.clone()) {
                assert_eq!(previous, deps, "targets must agree on package dependencies");
            }

            if let Some(package) = packages.iter_mut().find(|package| package.name == name) {
                assert_eq!(package.root_dir(), package_root);
                assert_eq!(package.edition, edition);
                assert_eq!(package.origin, origin);
                assert_eq!(package.cfg_options, cfg_options);
                assert_eq!(package.declared_features, declared_features);
                package.targets.extend(targets);
            } else {
                let is_workspace_member = origin == PackageOrigin::Workspace;
                packages.push(Package {
                    id: PackageId(format!("fixture:{name}")),
                    name: name.to_string(),
                    edition,
                    origin,
                    is_workspace_member,
                    manifest_path: package_root.join("Cargo.toml"),
                    cfg_options,
                    declared_features,
                    // Generated build outputs belong to Cargo integration fixtures.
                    cargo_generated_sources: None,
                    targets,
                    dependencies: Vec::new(),
                });
            }
        }
        assert!(!packages.is_empty(), "fixture needs a crate: declaration");

        // Stable package and target order keeps query snapshots independent of declaration order.
        packages.sort_by(|left, right| left.name.cmp(&right.name));
        let ids = packages
            .iter()
            .map(|package| (package.name.clone(), package.id.clone()))
            .collect::<BTreeMap<_, _>>();
        for package in &mut packages {
            package.targets.sort_by(|left, right| {
                (left.kind.sort_order(), &left.name).cmp(&(right.kind.sort_order(), &right.name))
            });
            // A dependency used in both normal and dev targets is still one resolved edge.
            let [normal, dev] = &dependency_names[&package.name];
            let mut dependencies = BTreeMap::new();
            for (names, is_dev) in [(normal, false), (dev, true)] {
                for dependency in names.split(',').filter(|name| !name.is_empty()) {
                    let (alias, name) = dependency
                        .split_once('=')
                        .unwrap_or((dependency, dependency));
                    let (is_normal, is_development) =
                        dependencies.entry((alias, name)).or_insert((false, false));
                    *is_normal |= !is_dev;
                    *is_development |= is_dev;
                }
            }
            for ((alias, name), (is_normal, is_dev)) in dependencies {
                let id = ids
                    .get(name)
                    .unwrap_or_else(|| panic!("unknown fixture dependency `{name}`"));
                // TODO: Add build edges when semantic fixtures need build-script scopes.
                package.dependencies.push(PackageDependency::new(
                    id.clone(),
                    alias,
                    is_normal,
                    false,
                    is_dev,
                ));
            }
        }
        Self::from_parts(
            root.clone(),
            root.join("target"),
            CargoBuildOutputScanStats::default(),
            target_cfg,
            packages,
        )
    }
}

#[cfg(test)]
mod tests {
    use rg_text::RustEdition;
    use test_fixture::fixture_crate;

    use crate::{TargetKind, WorkspaceMetadata};

    #[test]
    fn declared_workspace_connects_targets_dependencies_and_cfg() {
        let fixture = fixture_crate(
            r#"
//- /app/src/main.rs crate:app root:app kind:bin name:tool deps:api=library dev-deps:api=library cfg:feature=enabled features:enabled,disabled
fn main() {}
//- /app/src/lib.rs crate:app root:app deps:api=library dev-deps:api=library cfg:feature=enabled features:enabled,disabled
pub fn work() {}
//- /library/src/lib.rs crate:library root:library edition:2021 origin:dependency
pub struct Item;
"#,
        );
        let workspace = WorkspaceMetadata::from_fixture(&fixture.path(""), fixture.headers());
        let app = &workspace.packages()[0];
        let library = &workspace.packages()[1];
        assert_eq!(
            app.targets
                .iter()
                .map(|target| (&*target.name, &target.kind))
                .collect::<Vec<_>>(),
            [("app", &TargetKind::Lib), ("tool", &TargetKind::Bin)]
        );
        assert!(app.cfg_options.contains_key_value("feature", "enabled"));
        assert_eq!(app.declared_features, ["disabled", "enabled"]);
        assert_eq!(app.dependencies.len(), 1);
        assert_eq!(app.dependencies[0].name(), "api");
        assert_eq!(app.dependencies[0].package_id(), &library.id);
        assert!(app.dependencies[0].is_normal());
        assert!(app.dependencies[0].is_dev());
        assert_eq!(library.edition, RustEdition::Edition2021);
        assert!(!library.is_workspace_member);
        assert!(app.targets.iter().all(|target| target.src_path.is_file()));
    }
}
