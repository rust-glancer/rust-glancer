use std::{fmt, marker::PhantomData, sync::Arc};

use expect_test::Expect;

use crate::{
    DefId, DefMap, DefMapDb, ImportData, ImportKind, ModuleId, ModuleRef, Path, PathSegment,
    ResolvePathResult, ScopeBinding, ScopeEntry, TargetRef,
};
use rg_item_tree::{ItemTreeDb, VisibilityLevel};
use rg_package_store::{LoadPackage, PackageLoader, PackageStoreError};
use rg_parse::{FileId, Package, ParseDb, Target};
use rg_workspace::{SysrootSources, TargetKind, WorkspaceMetadata};
use test_fixture::fixture_crate;

pub(super) fn check_project_def_map(fixture: &str, expect: Expect) {
    let db = DefMapFixtureDb::build(fixture);
    let actual = ProjectDefMapSnapshot::new(&db).render();
    let actual = format!("{}\n", actual.trim_end());
    expect.assert_eq(&actual);
}

pub(super) fn check_project_path_resolution(
    fixture: &str,
    queries: &[PathResolutionQuery],
    expect: Expect,
) {
    let db = DefMapFixtureDb::build(fixture);
    let actual = ProjectPathResolutionSnapshot::new(&db, queries).render();
    let actual = format!("{}\n", actual.trim_end());
    expect.assert_eq(&actual);
}

pub(super) fn check_project_path_resolution_with_sysroot(
    fixture: &str,
    queries: &[PathResolutionQuery],
    expect: Expect,
) {
    let db = DefMapFixtureDb::build_with_sysroot(fixture);
    let actual = ProjectPathResolutionSnapshot::new(&db, queries).render();
    let actual = format!("{}\n", actual.trim_end());
    expect.assert_eq(&actual);
}

pub(super) struct PathResolutionQuery {
    package_name: &'static str,
    target_kind: TargetKind,
    module_path: &'static str,
    path: &'static str,
}

impl PathResolutionQuery {
    pub(super) fn lib(
        package_name: &'static str,
        module_path: &'static str,
        path: &'static str,
    ) -> Self {
        Self {
            package_name,
            target_kind: TargetKind::Lib,
            module_path,
            path,
        }
    }

    pub(super) fn bin(
        package_name: &'static str,
        module_path: &'static str,
        path: &'static str,
    ) -> Self {
        Self {
            package_name,
            target_kind: TargetKind::Bin,
            module_path,
            path,
        }
    }
}

pub(super) struct DefMapFixtureDb {
    parse: ParseDb,
    def_map: DefMapDb,
}

impl DefMapFixtureDb {
    pub(super) fn build(fixture: &str) -> Self {
        let fixture = fixture_crate(fixture);
        let workspace = WorkspaceMetadata::from_cargo(fixture.metadata())
            .expect("fixture workspace metadata should build");
        Self::build_from_workspace(workspace)
    }

    pub(super) fn build_with_sysroot(fixture: &str) -> Self {
        let fixture = fixture_crate(fixture);
        let sysroot = SysrootSources::from_library_root(fixture.path("sysroot/library"))
            .expect("fixture sysroot should be complete");
        let workspace = WorkspaceMetadata::from_cargo(fixture.metadata())
            .expect("fixture workspace metadata should build")
            .with_sysroot_sources(Some(sysroot));
        Self::build_from_workspace(workspace)
    }

    fn build_from_workspace(workspace: WorkspaceMetadata) -> Self {
        let mut parse = ParseDb::build(&workspace).expect("fixture parse db should build");
        let item_tree = ItemTreeDb::build(&mut parse).expect("fixture item tree db should build");
        let def_map = DefMapDb::builder(&workspace, &parse, &item_tree)
            .build()
            .expect("fixture def map db should build");
        Self { parse, def_map }
    }

    fn parse_db(&self) -> &ParseDb {
        &self.parse
    }

    pub(super) fn def_map_db(&self) -> &DefMapDb {
        &self.def_map
    }

    fn resident_def_map(&self, target: TargetRef) -> Option<&DefMap> {
        self.def_map
            .resident_package(target.package)?
            .target(target.target)
    }

    /// Returns the library target for one package.
    pub(super) fn lib(&self, package_name: &str) -> FixtureTarget<'_> {
        self.target(package_name, TargetKind::Lib)
    }

    fn target(&self, package_name: &str, expected_kind: TargetKind) -> FixtureTarget<'_> {
        let (package_slot, package) = self
            .parse_db()
            .packages()
            .iter()
            .enumerate()
            .find(|(_, package)| package.package_name() == package_name)
            .unwrap_or_else(|| panic!("fixture package `{package_name}` should exist"));
        let target = package
            .targets()
            .iter()
            .find(|target| target.kind == expected_kind)
            .unwrap_or_else(|| {
                panic!(
                    "fixture package `{package_name}` should have a {:?} target",
                    expected_kind
                )
            });

        FixtureTarget {
            db: self,
            package,
            target,
            target_ref: TargetRef {
                package: crate::PackageSlot(package_slot),
                target: target.id,
            },
        }
    }
}

/// Target-scoped assertion helper used by behavior-style def-map tests.
pub(super) struct FixtureTarget<'a> {
    db: &'a DefMapFixtureDb,
    package: &'a Package,
    target: &'a Target,
    target_ref: TargetRef,
}

impl<'a> FixtureTarget<'a> {
    /// Looks up one textual name in the root module scope of this target.
    pub(super) fn entry(&self, name: &str) -> FixtureEntry<'a> {
        let entry = self
            .def_map()
            .root_module()
            .and_then(|root_module| self.def_map().module(root_module))
            .and_then(|module| module.scope.entry(name));
        FixtureEntry {
            db: self.db,
            package_name: self.package.package_name(),
            target: self.target,
            name: name.to_string(),
            entry,
        }
    }

    fn def_map(&self) -> &'a DefMap {
        self.db
            .resident_def_map(self.target_ref)
            .expect("target def map should exist in fixture db")
    }
}

/// Root-scope entry assertion helper for one textual name.
pub(super) struct FixtureEntry<'a> {
    db: &'a DefMapFixtureDb,
    package_name: &'a str,
    target: &'a Target,
    name: String,
    entry: Option<&'a ScopeEntry>,
}

impl<'a> FixtureEntry<'a> {
    /// Asserts that the entry is absent from the root scope.
    pub(super) fn assert_missing(&self, reason: &str) -> &Self {
        assert!(
            self.entry.is_none(),
            "{reason}: expected {} to be absent",
            self.context(),
        );
        self
    }

    /// Asserts that the entry has at least one visible type binding.
    pub(super) fn assert_type_exists(&self, reason: &str) -> &Self {
        assert!(
            !self.scope_entry().types().is_empty(),
            "{reason}: expected {} to have a type binding",
            self.context(),
        );
        self
    }

    /// Asserts that the entry has at least one visible value binding.
    pub(super) fn assert_value_exists(&self, reason: &str) -> &Self {
        assert!(
            !self.scope_entry().values().is_empty(),
            "{reason}: expected {} to have a value binding",
            self.context(),
        );
        self
    }

    /// Asserts that one type binding resolves to a module with the requested name.
    pub(super) fn assert_module_named(&self, module_name: &str, reason: &str) -> &Self {
        assert!(
            self.scope_entry()
                .types()
                .iter()
                .filter_map(|binding| self.binding_origin(binding))
                .any(|origin| origin.module_name() == Some(module_name)),
            "{reason}: expected {} to resolve to module `{module_name}`",
            self.context(),
        );
        self
    }

    fn context(&self) -> String {
        format!(
            "root scope entry `{}` in package `{}` target `{}` ({:?})",
            self.name, self.package_name, self.target.name, self.target.kind,
        )
    }

    fn scope_entry(&self) -> &ScopeEntry {
        self.entry.unwrap_or_else(|| {
            panic!(
                "expected {} to exist before asserting on its bindings",
                self.context()
            )
        })
    }

    fn binding_origin(&self, binding: &'a ScopeBinding) -> Option<FixtureBindingOrigin<'a>> {
        let target_ref = match binding.def {
            DefId::Module(module_ref) => module_ref.target,
            DefId::Local(local_def_ref) => local_def_ref.target,
        };
        self.db.parse_db().packages().get(target_ref.package.0)?;
        self.db.resident_def_map(target_ref)?;

        Some(FixtureBindingOrigin {
            db: self.db,
            def: binding.def,
        })
    }
}

/// Project-relative view of one resolved binding origin.
struct FixtureBindingOrigin<'a> {
    db: &'a DefMapFixtureDb,
    def: DefId,
}

impl FixtureBindingOrigin<'_> {
    fn module_name(&self) -> Option<&str> {
        let DefId::Module(module_ref) = self.def else {
            return None;
        };

        self.db
            .resident_def_map(module_ref.target)?
            .module(module_ref.module)
            .and_then(|module| module.name.as_deref())
    }
}

/// Project-level DefMap snapshot context.
/// Renders package sections such as `package app`.
struct ProjectDefMapSnapshot<'a> {
    project: &'a DefMapFixtureDb,
}

impl<'a> ProjectDefMapSnapshot<'a> {
    fn new(project: &'a DefMapFixtureDb) -> Self {
        Self { project }
    }

    fn render(&self) -> String {
        let package_dumps = sorted_packages(self.project.parse_db())
            .into_iter()
            .map(|(package_slot, package)| {
                PackageDefMapSnapshot {
                    project: self.project,
                    package_slot,
                    package,
                }
                .render()
            })
            .collect::<Vec<_>>();

        package_dumps.join("\n\n")
    }
}

/// Project-level path-resolution snapshot context.
struct ProjectPathResolutionSnapshot<'a> {
    project: &'a DefMapFixtureDb,
    queries: &'a [PathResolutionQuery],
}

impl<'a> ProjectPathResolutionSnapshot<'a> {
    fn new(project: &'a DefMapFixtureDb, queries: &'a [PathResolutionQuery]) -> Self {
        Self { project, queries }
    }

    fn render(&self) -> String {
        self.queries
            .iter()
            .map(|query| self.render_query(query))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn render_query(&self, query: &PathResolutionQuery) -> String {
        let (target_ref, target) = self.target_ref(query);
        let module_id = self.module_id(target_ref, query.module_path);
        let path = Self::parse_path(query.path);
        let def_map = self
            .project
            .def_map_db()
            .read_txn(unexpected_package_loader());
        let result = def_map
            .resolve_path(
                ModuleRef {
                    target: target_ref,
                    module: module_id,
                },
                &path,
            )
            .expect("path resolution fixture should load def-map packages");

        format!(
            "{} [{}] {} resolves {} -> {}",
            query.package_name,
            target.kind,
            query.module_path,
            path,
            self.render_result(&result),
        )
    }

    fn target_ref(&self, query: &PathResolutionQuery) -> (TargetRef, &'a Target) {
        let (package_slot, package) = self
            .project
            .parse_db()
            .packages()
            .iter()
            .enumerate()
            .find(|(_, package)| package.package_name() == query.package_name)
            .unwrap_or_else(|| panic!("fixture package `{}` should exist", query.package_name));
        let target = package
            .targets()
            .iter()
            .find(|target| target.kind == query.target_kind)
            .unwrap_or_else(|| {
                panic!(
                    "fixture package `{}` should have a {} target",
                    query.package_name, query.target_kind
                )
            });

        (
            TargetRef {
                package: crate::PackageSlot(package_slot),
                target: target.id,
            },
            target,
        )
    }

    fn module_id(&self, target_ref: TargetRef, module_path: &str) -> ModuleId {
        let def_map = self
            .project
            .resident_def_map(target_ref)
            .expect("target def map should exist while resolving path snapshot query");

        def_map
            .modules()
            .iter()
            .enumerate()
            .find_map(|(module_idx, _)| {
                let module_id = ModuleId(module_idx);
                (self.module_path(target_ref, module_id) == module_path).then_some(module_id)
            })
            .unwrap_or_else(|| panic!("module `{module_path}` should exist in fixture target"))
    }

    fn module_path(&self, target_ref: TargetRef, module_id: ModuleId) -> String {
        let module = self
            .project
            .resident_def_map(target_ref)
            .expect("target def map should exist while building module path")
            .module(module_id)
            .expect("module id should exist while building module path");

        match module.parent {
            Some(parent) => {
                let parent_path = self.module_path(target_ref, parent);
                let name = module
                    .name
                    .as_deref()
                    .expect("non-root modules should have names");
                format!("{parent_path}::{name}")
            }
            None => "crate".to_string(),
        }
    }

    fn parse_path(text: &str) -> Path {
        let (absolute, text) = match text.strip_prefix("::") {
            Some(stripped) => (true, stripped),
            None => (false, text),
        };
        let segments = text
            .split("::")
            .filter(|segment| !segment.is_empty())
            .map(|segment| match segment {
                "self" => PathSegment::SelfKw,
                "super" => PathSegment::SuperKw,
                "crate" => PathSegment::CrateKw,
                name => PathSegment::Name(name.to_string().into()),
            })
            .collect::<Vec<_>>();

        Path { absolute, segments }
    }

    fn render_result(&self, result: &ResolvePathResult) -> String {
        let mut resolved = result
            .resolved
            .iter()
            .map(|def| {
                ResolvedDefOrigin {
                    project: self.project,
                    def: *def,
                }
                .render()
            })
            .collect::<Vec<_>>();
        resolved.sort();

        let mut rendered = if resolved.is_empty() {
            "<none>".to_string()
        } else {
            resolved.join("; ")
        };

        if let Some(unresolved_at) = result.unresolved_at {
            rendered.push_str(&format!(" (unresolved at segment #{unresolved_at})"));
        }

        rendered
    }
}

fn unexpected_package_loader<T: 'static>() -> PackageLoader<'static, T> {
    PackageLoader::new(UnexpectedPackageLoader(PhantomData))
}

struct UnexpectedPackageLoader<T>(PhantomData<fn() -> T>);

impl<T> fmt::Debug for UnexpectedPackageLoader<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnexpectedPackageLoader").finish()
    }
}

impl<T> LoadPackage<T> for UnexpectedPackageLoader<T> {
    fn load(&self, package: rg_workspace::PackageSlot) -> Result<Arc<T>, PackageStoreError> {
        panic!(
            "def-map fixture query should not load offloaded package {}",
            package.0,
        );
    }
}

/// Package-level DefMap snapshot context.
/// Renders target sections such as `app [lib]`.
struct PackageDefMapSnapshot<'a> {
    project: &'a DefMapFixtureDb,
    package_slot: usize,
    package: &'a Package,
}

impl<'a> PackageDefMapSnapshot<'a> {
    fn render(&self) -> String {
        let target_dumps = sorted_targets(self.package)
            .into_iter()
            .map(|target| {
                let target_ref = TargetRef {
                    package: crate::PackageSlot(self.package_slot),
                    target: target.id,
                };
                TargetDefMapSnapshot {
                    project: self.project,
                    package: self.package,
                    target,
                    target_ref,
                }
                .render()
                .trim_end()
                .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        format!("package {}\n\n{target_dumps}", self.package.package_name())
    }
}

/// Target-level DefMap snapshot context with access to resolved module paths.
/// Renders module scopes such as `crate::nested`.
struct TargetDefMapSnapshot<'a> {
    project: &'a DefMapFixtureDb,
    package: &'a Package,
    target: &'a Target,
    target_ref: TargetRef,
}

impl<'a> TargetDefMapSnapshot<'a> {
    fn render(&self) -> String {
        let def_map = self.def_map();
        let mut dump = format!("{} [{}]\n", self.package.package_name(), self.target.kind);

        for (idx, (module_path, module_id)) in self.sorted_modules().into_iter().enumerate() {
            if idx > 0 {
                dump.push('\n');
            }

            dump.push_str(&module_path);
            dump.push('\n');

            let module = def_map
                .module(module_id)
                .expect("module id should exist in def map dump");

            for name in self.sorted_scope_names(&module.scope) {
                let entry = module
                    .scope
                    .entry(&name)
                    .expect("scope entry should exist while dumping");
                dump.push_str(&format!("- {name} : {}\n", self.render_scope_entry(entry)));
            }

            if !module.unresolved_imports.is_empty() {
                dump.push_str("unresolved imports\n");

                for import_id in &module.unresolved_imports {
                    let import = def_map
                        .imports
                        .get(*import_id)
                        .expect("unresolved import id should exist while dumping");
                    dump.push_str(&format!("- {}\n", self.render_unresolved_import(import)));
                }
            }

            if !module.impls.is_empty() {
                dump.push_str("impls\n");

                for impl_id in &module.impls {
                    let local_impl = def_map
                        .local_impls()
                        .get(impl_id.0)
                        .expect("local impl id should exist while dumping");
                    dump.push_str(&format!(
                        "- impl {}\n",
                        self.render_item_tree_ref(local_impl.source)
                    ));
                }
            }
        }

        dump
    }

    fn def_map(&self) -> &'a DefMap {
        self.project
            .resident_def_map(self.target_ref)
            .expect("target def map should exist while rendering snapshot")
    }

    fn sorted_modules(&self) -> Vec<(String, ModuleId)> {
        let mut modules = self
            .def_map()
            .modules
            .iter()
            .enumerate()
            .map(|(idx, _)| {
                let module_id = ModuleId(idx);
                (self.module_path(self.target_ref, module_id), module_id)
            })
            .collect::<Vec<_>>();
        modules.sort_by(|left, right| left.0.cmp(&right.0));
        modules
    }

    fn sorted_scope_names(&self, scope: &crate::ModuleScope) -> Vec<String> {
        let mut names = scope
            .entries()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        names.sort();
        names.into_iter().map(|name| name.to_string()).collect()
    }

    fn render_scope_entry(&self, entry: &ScopeEntry) -> String {
        let mut parts = Vec::new();

        if !entry.types().is_empty() {
            parts.push(format!(
                "type [{}]",
                self.render_namespace_bindings(entry.types())
            ));
        }

        if !entry.values().is_empty() {
            parts.push(format!(
                "value [{}]",
                self.render_namespace_bindings(entry.values())
            ));
        }

        if !entry.macros().is_empty() {
            parts.push(format!(
                "macro [{}]",
                self.render_namespace_bindings(entry.macros())
            ));
        }

        parts.join(" | ")
    }

    fn render_namespace_bindings(&self, bindings: &[ScopeBinding]) -> String {
        let mut rendered = bindings
            .iter()
            .filter_map(|binding| self.binding_origin(binding))
            .map(|origin| origin.render())
            .collect::<Vec<_>>();
        rendered.sort();
        rendered.join("; ")
    }

    fn binding_origin(&self, binding: &'a ScopeBinding) -> Option<BindingOrigin<'a>> {
        let target_ref = match binding.def {
            DefId::Module(module_ref) => module_ref.target,
            DefId::Local(local_def_ref) => local_def_ref.target,
        };
        self.project
            .parse_db()
            .packages()
            .get(target_ref.package.0)?;
        self.project.resident_def_map(target_ref)?;

        Some(BindingOrigin {
            project: self.project,
            def: binding.def,
            binding_visibility: &binding.visibility,
        })
    }

    fn render_unresolved_import(&self, import: &ImportData) -> String {
        let visibility = match &import.visibility {
            VisibilityLevel::Private => String::new(),
            visibility => format!("{visibility} "),
        };
        let path = match import.kind {
            ImportKind::Glob => format!("{}::*", import.path),
            ImportKind::Named | ImportKind::SelfImport => import.path.to_string(),
        };

        format!("{visibility}use {path}{}", import.binding)
    }

    fn render_item_tree_ref(&self, item_ref: rg_item_tree::ItemTreeRef) -> String {
        let file_label = file_label(self.package, item_ref.file_id);
        format!("{file_label}#{}", item_ref.item.0)
    }

    fn module_path(&self, target_ref: TargetRef, module_id: ModuleId) -> String {
        let module = self
            .project
            .resident_def_map(target_ref)
            .expect("target def map should exist while building relative module path")
            .module(module_id)
            .expect("module id should exist while building relative module path");

        match module.parent {
            Some(parent) => {
                let parent_path = self.module_path(target_ref, parent);
                let name = module
                    .name
                    .as_deref()
                    .expect("non-root modules should have names");
                format!("{parent_path}::{name}")
            }
            None => "crate".to_string(),
        }
    }
}

/// Snapshot-only view of where a resolved scope binding came from.
/// Renders origins such as `pub fn app[lib]::crate::make`.
struct BindingOrigin<'a> {
    project: &'a DefMapFixtureDb,
    def: DefId,
    binding_visibility: &'a VisibilityLevel,
}

impl BindingOrigin<'_> {
    fn render(&self) -> String {
        let visibility = Self::visibility_prefix(self.binding_visibility);
        let origin = ResolvedDefOrigin {
            project: self.project,
            def: self.def,
        }
        .render();

        format!("{visibility}{origin}")
    }

    fn visibility_prefix(visibility: &VisibilityLevel) -> String {
        match visibility {
            VisibilityLevel::Private => String::new(),
            _ => format!("{visibility} "),
        }
    }
}

/// Snapshot-only view of one resolved definition.
struct ResolvedDefOrigin<'a> {
    project: &'a DefMapFixtureDb,
    def: DefId,
}

impl ResolvedDefOrigin<'_> {
    fn render(&self) -> String {
        match self.def {
            DefId::Module(module_ref) => {
                format!("module {}", self.render_module_path(module_ref))
            }
            DefId::Local(local_def_ref) => {
                let local_def = self
                    .project
                    .resident_def_map(local_def_ref.target)
                    .expect("target def map should exist while dumping")
                    .local_defs
                    .get(local_def_ref.local_def)
                    .expect("local def id should exist while dumping");
                let module_path = self.render_module_path(crate::ModuleRef {
                    target: local_def_ref.target,
                    module: local_def.module,
                });

                format!("{} {}::{}", local_def.kind, module_path, local_def.name)
            }
        }
    }

    fn render_module_path(&self, module_ref: crate::ModuleRef) -> String {
        let package = self
            .project
            .parse_db()
            .packages()
            .get(module_ref.target.package.0)
            .expect("package slot should exist while dumping");
        let target = package
            .target(module_ref.target.target)
            .expect("target id should exist while dumping");

        format!(
            "{}[{}]::{}",
            package.package_name(),
            target.kind,
            self.module_path(module_ref.target, module_ref.module),
        )
    }

    fn module_path(&self, target_ref: TargetRef, module_id: ModuleId) -> String {
        let module = self
            .project
            .resident_def_map(target_ref)
            .expect("target def map should exist while building relative module path")
            .module(module_id)
            .expect("module id should exist while building relative module path");

        match module.parent {
            Some(parent) => {
                let parent_path = self.module_path(target_ref, parent);
                let name = module
                    .name
                    .as_deref()
                    .expect("non-root modules should have names");
                format!("{parent_path}::{name}")
            }
            None => "crate".to_string(),
        }
    }
}

fn sorted_packages(parse: &ParseDb) -> Vec<(usize, &Package)> {
    let mut packages = parse.packages().iter().enumerate().collect::<Vec<_>>();
    packages.sort_by(|left, right| left.1.package_name().cmp(right.1.package_name()));
    packages
}

fn sorted_targets(package: &Package) -> Vec<&Target> {
    let mut targets = package.targets().iter().collect::<Vec<_>>();
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
    targets
}

fn file_label(package: &Package, file_id: FileId) -> String {
    package
        .file_path(file_id)
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("<unknown>")
        .to_string()
}
