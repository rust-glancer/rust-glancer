use expect_test::Expect;

use crate::{
    DefMap, ImportData, ImportKind, ItemSource, ItemSourceKind, LocalDefKind, Namespace,
    NamespaceSet, ResolvePathResult, ScopeBinding, ScopeBindingProvenance, ScopeEntry,
    ScopeResolutionRef, Visibility,
};
use crate::{DefMapDb, testonly::DefMapFixture};
use rg_ir_model::{CrateId, CrateRef, DefId, DefMapRef, ModuleId, ModuleRef, Path};
use rg_item_tree::VisibilityLevel;
use rg_package_store::PackageLoader;
use rg_parse::{CargoTarget, FileId, Package, ParseDb};
use rg_workspace::{TargetKind, WorkspaceLoweringConfig};

pub(super) fn check_project_def_map(fixture: &str, expect: Expect) {
    let db = DefMapFixtureDb::build(fixture);
    let actual = ProjectDefMapSnapshot::new(&db).render();
    let actual = format!("{}\n", actual.trim_end());
    expect.assert_eq(&actual);
}

pub(super) fn check_project_def_map_with_sysroot(fixture: &str, expect: Expect) {
    let db = DefMapFixtureDb::build_with_sysroot(fixture);
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

pub(super) fn check_project_path_resolution_with_fake_sysroot(
    fixture: &str,
    queries: &[PathResolutionQuery],
    expect: Expect,
) {
    let db = DefMapFixtureDb::build_with_fake_sysroot(fixture);
    let actual = ProjectPathResolutionSnapshot::new(&db, queries).render();
    let actual = format!("{}\n", actual.trim_end());
    expect.assert_eq(&actual);
}

pub(super) struct PathResolutionQuery {
    package_name: &'static str,
    target_kind: TargetKind,
    module_path: &'static str,
    path: &'static str,
    namespaces: NamespaceSet,
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
            namespaces: NamespaceSet::ALL,
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
            namespaces: NamespaceSet::ALL,
        }
    }

    pub(super) fn proc_macro(
        package_name: &'static str,
        module_path: &'static str,
        path: &'static str,
    ) -> Self {
        Self {
            package_name,
            target_kind: TargetKind::ProcMacro,
            module_path,
            path,
            namespaces: NamespaceSet::ALL,
        }
    }

    pub(super) fn types(mut self) -> Self {
        self.namespaces = NamespaceSet::TYPES;
        self
    }

    pub(super) fn values(mut self) -> Self {
        self.namespaces = NamespaceSet::VALUES;
        self
    }

    pub(super) fn macros(mut self) -> Self {
        self.namespaces = NamespaceSet::MACROS;
        self
    }
}

pub(super) struct DefMapFixtureDb {
    fixture: DefMapFixture,
}

impl DefMapFixtureDb {
    pub(super) fn build(fixture: &str) -> Self {
        Self {
            fixture: DefMapFixture::build(fixture),
        }
    }

    pub(super) fn build_with_workspace_config(
        fixture: &str,
        config: WorkspaceLoweringConfig,
    ) -> Self {
        Self {
            fixture: DefMapFixture::build_with_workspace_config(fixture, config),
        }
    }

    pub(super) fn build_with_sysroot(fixture: &str) -> Self {
        Self {
            fixture: DefMapFixture::build_with_sysroot(fixture),
        }
    }

    pub(super) fn build_with_fake_sysroot(fixture: &str) -> Self {
        Self {
            fixture: DefMapFixture::build_with_fake_sysroot(fixture),
        }
    }

    fn parse_db(&self) -> &ParseDb {
        self.fixture.parse_db()
    }

    pub(super) fn def_map_db(&self) -> &DefMapDb {
        self.fixture.def_map_db()
    }

    fn resident_def_map(&self, crate_ref: CrateRef) -> Option<&DefMap> {
        self.fixture.resident_def_map(crate_ref)
    }

    /// Returns the library target for one package.
    pub(super) fn lib(&self, package_name: &str) -> FixtureCrate<'_> {
        self.target(package_name, TargetKind::Lib)
    }

    fn target(&self, package_name: &str, expected_kind: TargetKind) -> FixtureCrate<'_> {
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

        FixtureCrate {
            db: self,
            package,
            target,
            crate_ref: CrateRef {
                package: crate::PackageSlot(package_slot),
                crate_id: CrateId(target.id.0),
            },
        }
    }
}

/// Crate-scoped assertion helper used by behavior-style def-map tests.
pub(super) struct FixtureCrate<'a> {
    db: &'a DefMapFixtureDb,
    package: &'a Package,
    target: &'a CargoTarget,
    crate_ref: CrateRef,
}

impl<'a> FixtureCrate<'a> {
    /// Looks up one textual name in this crate's root module scope.
    pub(super) fn entry(&self, name: &str) -> FixtureEntry<'a> {
        let entry = self
            .db
            .def_map_db()
            .resident_package(self.crate_ref.package)
            .and_then(|package| package.crate_data(self.crate_ref.crate_id))
            .and_then(|crate_data| crate_data.root_module())
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
            .resident_def_map(self.crate_ref)
            .expect("crate def map should exist in fixture db")
    }
}

/// Root-scope entry assertion helper for one textual name.
pub(super) struct FixtureEntry<'a> {
    db: &'a DefMapFixtureDb,
    package_name: &'a str,
    target: &'a CargoTarget,
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
            !self.scope_entry().bindings(Namespace::Types).is_empty(),
            "{reason}: expected {} to have a type binding",
            self.context(),
        );
        self
    }

    /// Asserts that the entry has at least one visible value binding.
    pub(super) fn assert_value_exists(&self, reason: &str) -> &Self {
        assert!(
            !self.scope_entry().bindings(Namespace::Values).is_empty(),
            "{reason}: expected {} to have a value binding",
            self.context(),
        );
        self
    }

    /// Asserts that the entry has no selected value binding.
    pub(super) fn assert_value_missing(&self, reason: &str) -> &Self {
        assert!(
            self.scope_entry().bindings(Namespace::Values).is_empty(),
            "{reason}: expected {} not to have a value binding",
            self.context(),
        );
        self
    }

    /// Asserts that one selected value binding has the requested local definition kind.
    pub(super) fn assert_value_kind(&self, kind: LocalDefKind, reason: &str) -> &Self {
        assert!(
            self.scope_entry()
                .bindings(Namespace::Values)
                .iter()
                .filter_map(|binding| self.binding_origin(binding))
                .any(|origin| origin.local_def_kind() == Some(kind)),
            "{reason}: expected {} to have a `{kind}` value binding",
            self.context(),
        );
        self
    }

    /// Asserts that one type binding resolves to a module with the requested name.
    pub(super) fn assert_module_named(&self, module_name: &str, reason: &str) -> &Self {
        assert!(
            self.scope_entry()
                .bindings(Namespace::Types)
                .iter()
                .filter_map(|binding| self.binding_origin(binding))
                .any(|origin| origin.module_name() == Some(module_name)),
            "{reason}: expected {} to resolve to module `{module_name}`",
            self.context(),
        );
        self
    }

    /// Asserts that one type binding points at an item lowered from the requested source file.
    pub(super) fn assert_type_source_file(&self, file_name: &str, reason: &str) -> &Self {
        assert!(
            self.scope_entry()
                .bindings(Namespace::Types)
                .iter()
                .filter_map(|binding| self.binding_origin(binding))
                .any(|origin| origin.source_file_name().as_deref() == Some(file_name)),
            "{reason}: expected {} to have a type binding from `{file_name}`",
            self.context(),
        );
        self
    }

    /// Asserts that the type namespace selected one definition with the requested route count.
    pub(super) fn assert_type_resolved_with_routes(
        &self,
        route_count: usize,
        reason: &str,
    ) -> &Self {
        let ScopeResolutionRef::Resolved(binding) = self.scope_entry().resolution(Namespace::Types)
        else {
            panic!("{reason}: expected {} to resolve uniquely", self.context());
        };
        assert_eq!(
            binding.routes().len(),
            route_count,
            "{reason}: unexpected route count for {}",
            self.context(),
        );
        self
    }

    /// Asserts that the type namespace retained an explicit ambiguity.
    pub(super) fn assert_type_ambiguous(&self, candidate_count: usize, reason: &str) -> &Self {
        let ScopeResolutionRef::Ambiguous(bindings) =
            self.scope_entry().resolution(Namespace::Types)
        else {
            panic!("{reason}: expected {} to be ambiguous", self.context());
        };
        assert_eq!(
            bindings.len(),
            candidate_count,
            "{reason}: unexpected candidate count for {}",
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
        let origin = match binding.def {
            DefId::Module(module_ref) => module_ref.origin,
            DefId::Local(local_def_ref) => local_def_ref.origin,
            DefId::EnumVariant(variant_ref) => variant_ref.origin,
        };
        let crate_ref = origin.as_crate_ref()?;
        self.db.parse_db().packages().get(crate_ref.package.0)?;
        self.db.resident_def_map(crate_ref)?;

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
    fn local_def_kind(&self) -> Option<LocalDefKind> {
        let DefId::Local(local_def_ref) = self.def else {
            return None;
        };

        self.db
            .resident_def_map(local_def_ref.origin.as_crate_ref()?)?
            .local_def(local_def_ref.local_def)
            .map(|data| data.kind)
    }

    fn module_name(&self) -> Option<&str> {
        let DefId::Module(module_ref) = self.def else {
            return None;
        };

        self.db
            .resident_def_map(module_ref.origin.as_crate_ref()?)?
            .module(module_ref.module)
            .and_then(|module| module.name.as_deref())
    }

    fn source_file_name(&self) -> Option<String> {
        let DefId::Local(local_def_ref) = self.def else {
            return None;
        };
        let crate_ref = local_def_ref.origin.as_crate_ref()?;
        let local_def = self
            .db
            .resident_def_map(crate_ref)?
            .local_def(local_def_ref.local_def)?;
        self.db
            .parse_db()
            .package(crate_ref.package.0)?
            .file_path(local_def.file_id)?
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
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
        let (crate_ref, target) = self.crate_ref(query);
        let module_id = self.module_id(crate_ref, query.module_path);
        let path = Self::parse_path(query.path);
        let def_map = self
            .project
            .def_map_db()
            .read_txn(PackageLoader::resident_only("def-map fixture query"));
        let result = crate::DefMapQuery::new(&def_map)
            .scope_resolver()
            .resolve_path(
                ModuleRef {
                    origin: DefMapRef::Crate(crate_ref),
                    module: module_id,
                },
                &path,
                query.namespaces,
            )
            .expect("path resolution fixture should load def-map packages");

        let namespace_suffix = if query.namespaces == NamespaceSet::TYPES {
            " [types]"
        } else if query.namespaces == NamespaceSet::VALUES {
            " [values]"
        } else if query.namespaces == NamespaceSet::MACROS {
            " [macros]"
        } else {
            ""
        };

        format!(
            "{} [{}] {} resolves {}{} -> {}",
            query.package_name,
            target.kind,
            query.module_path,
            path,
            namespace_suffix,
            self.render_result(&result),
        )
    }

    fn crate_ref(&self, query: &PathResolutionQuery) -> (CrateRef, &'a CargoTarget) {
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
            CrateRef {
                package: crate::PackageSlot(package_slot),
                crate_id: CrateId(target.id.0),
            },
            target,
        )
    }

    fn module_id(&self, crate_ref: CrateRef, module_path: &str) -> ModuleId {
        let def_map = self
            .project
            .resident_def_map(crate_ref)
            .expect("crate def map should exist while resolving path snapshot query");

        def_map
            .modules()
            .iter()
            .enumerate()
            .find_map(|(module_idx, _)| {
                let module_id = ModuleId(module_idx);
                (self.module_path(crate_ref, module_id) == module_path).then_some(module_id)
            })
            .unwrap_or_else(|| panic!("module `{module_path}` should exist in fixture target"))
    }

    fn module_path(&self, crate_ref: CrateRef, module_id: ModuleId) -> String {
        let module = self
            .project
            .resident_def_map(crate_ref)
            .expect("crate def map should exist while building module path")
            .module(module_id)
            .expect("module id should exist while building module path");

        match module.parent {
            Some(parent) => {
                let parent_path = self.module_path(crate_ref, parent);
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
        Path::from_macro_path_text(text, None).expect("fixture path should use valid Rust syntax")
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

/// Package-level DefMap snapshot context.
/// Renders target sections such as `app [lib]`.
struct PackageDefMapSnapshot<'a> {
    project: &'a DefMapFixtureDb,
    package_slot: usize,
    package: &'a Package,
}

impl<'a> PackageDefMapSnapshot<'a> {
    fn render(&self) -> String {
        let crate_dumps = sorted_targets(self.package)
            .into_iter()
            .map(|target| {
                let crate_ref = CrateRef {
                    package: crate::PackageSlot(self.package_slot),
                    crate_id: CrateId(target.id.0),
                };
                CrateDefMapSnapshot {
                    project: self.project,
                    package: self.package,
                    target,
                    crate_ref,
                }
                .render()
                .trim_end()
                .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        format!("package {}\n\n{crate_dumps}", self.package.package_name())
    }
}

/// Crate-level DefMap snapshot context with access to resolved module paths.
/// Renders module scopes such as `crate::nested`.
struct CrateDefMapSnapshot<'a> {
    project: &'a DefMapFixtureDb,
    package: &'a Package,
    target: &'a CargoTarget,
    crate_ref: CrateRef,
}

impl<'a> CrateDefMapSnapshot<'a> {
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
                        .imports()
                        .get(import_id.0)
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
                        self.render_item_source(local_impl.source)
                    ));
                }
            }
        }

        dump
    }

    fn def_map(&self) -> &'a DefMap {
        self.project
            .resident_def_map(self.crate_ref)
            .expect("crate def map should exist while rendering snapshot")
    }

    fn sorted_modules(&self) -> Vec<(String, ModuleId)> {
        let mut modules = self
            .def_map()
            .modules()
            .iter()
            .enumerate()
            .map(|(idx, _)| {
                let module_id = ModuleId(idx);
                (self.module_path(self.crate_ref, module_id), module_id)
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

        if !entry.bindings(Namespace::Types).is_empty() {
            parts.push(format!(
                "type [{}]",
                self.render_namespace_bindings(entry.bindings(Namespace::Types))
            ));
        }

        if !entry.bindings(Namespace::Values).is_empty() {
            parts.push(format!(
                "value [{}]",
                self.render_namespace_bindings(entry.bindings(Namespace::Values))
            ));
        }

        if !entry.bindings(Namespace::Macros).is_empty() {
            parts.push(format!(
                "macro [{}]",
                self.render_namespace_bindings(entry.bindings(Namespace::Macros))
            ));
        }

        parts.join(" | ")
    }

    fn render_namespace_bindings(&self, bindings: &[ScopeBinding]) -> String {
        let mut rendered = bindings
            .iter()
            .flat_map(|binding| {
                binding.routes().iter().filter_map(|route| {
                    self.binding_origin_with_visibility(binding, route.visibility, route.provenance)
                })
            })
            .map(|origin| origin.render())
            .collect::<Vec<_>>();
        rendered.sort();
        rendered.join("; ")
    }

    fn binding_origin_with_visibility(
        &self,
        binding: &'a ScopeBinding,
        visibility: Visibility,
        provenance: ScopeBindingProvenance,
    ) -> Option<BindingOrigin<'a>> {
        let origin = match binding.def {
            DefId::Module(module_ref) => module_ref.origin,
            DefId::Local(local_def_ref) => local_def_ref.origin,
            DefId::EnumVariant(variant_ref) => variant_ref.origin,
        };
        let crate_ref = origin.as_crate_ref()?;
        self.project
            .parse_db()
            .packages()
            .get(crate_ref.package.0)?;
        self.project.resident_def_map(crate_ref)?;

        let visibility_prefix = if provenance.is_direct() {
            match binding.def {
                DefId::Local(local_def) => self
                    .project
                    .resident_def_map(crate_ref)?
                    .local_def(local_def.local_def)
                    .map(|data| BindingOrigin::source_visibility_prefix(&data.visibility))
                    .unwrap_or_else(|| BindingOrigin::semantic_visibility_prefix(visibility)),
                DefId::Module(_) | DefId::EnumVariant(_) => {
                    BindingOrigin::semantic_visibility_prefix(visibility)
                }
            }
        } else {
            BindingOrigin::semantic_visibility_prefix(visibility)
        };

        Some(BindingOrigin {
            project: self.project,
            def: binding.def,
            visibility_prefix,
        })
    }

    fn render_unresolved_import(&self, import: &ImportData) -> String {
        let visibility = match import.visibility {
            Visibility::Module(_) => String::new(),
            Visibility::Public => "pub ".to_string(),
            Visibility::Invisible => "invisible ".to_string(),
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

    fn render_item_source(&self, source: ItemSource) -> String {
        match source.kind {
            ItemSourceKind::ItemTree(item_ref) => self.render_item_tree_ref(item_ref),
            ItemSourceKind::Generated(item_ref) => {
                format!("generated#{}:{}", item_ref.source.0, item_ref.item.0)
            }
            ItemSourceKind::Body(_) => panic!("Body is not expected"),
        }
    }

    fn module_path(&self, crate_ref: CrateRef, module_id: ModuleId) -> String {
        let module = self
            .project
            .resident_def_map(crate_ref)
            .expect("crate def map should exist while building relative module path")
            .module(module_id)
            .expect("module id should exist while building relative module path");

        match module.parent {
            Some(parent) => {
                let parent_path = self.module_path(crate_ref, parent);
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
    visibility_prefix: String,
}

impl BindingOrigin<'_> {
    fn render(&self) -> String {
        let origin = ResolvedDefOrigin {
            project: self.project,
            def: self.def,
        }
        .render();

        format!("{}{origin}", self.visibility_prefix)
    }

    fn semantic_visibility_prefix(visibility: Visibility) -> String {
        match visibility {
            Visibility::Module(_) => String::new(),
            Visibility::Public => "pub ".to_string(),
            Visibility::Invisible => "invisible ".to_string(),
        }
    }

    fn source_visibility_prefix(visibility: &VisibilityLevel) -> String {
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
                    .resident_def_map(local_def_ref.origin.origin_crate())
                    .expect("crate def map should exist while dumping")
                    .local_def(local_def_ref.local_def)
                    .expect("local def id should exist while dumping");
                let module_path = self.render_module_path(ModuleRef {
                    origin: local_def_ref.origin,
                    module: local_def.module,
                });

                format!("{} {}::{}", local_def.kind, module_path, local_def.name)
            }
            DefId::EnumVariant(variant_ref) => {
                let variant = self
                    .project
                    .resident_def_map(variant_ref.origin.origin_crate())
                    .expect("crate def map should exist while dumping")
                    .local_enum_variant(variant_ref.local_enum_variant)
                    .expect("enum variant id should exist while dumping");
                let enum_def = self
                    .project
                    .resident_def_map(variant_ref.origin.origin_crate())
                    .expect("crate def map should exist while dumping")
                    .local_def(variant.enum_def)
                    .expect("enum def id should exist while dumping");
                let module_path = self.render_module_path(ModuleRef {
                    origin: variant_ref.origin,
                    module: variant.module,
                });

                format!(
                    "variant {}::{}::{}",
                    module_path, enum_def.name, variant.name
                )
            }
        }
    }

    fn render_module_path(&self, module_ref: ModuleRef) -> String {
        let crate_ref = module_ref.origin.origin_crate();
        let package = self
            .project
            .parse_db()
            .packages()
            .get(crate_ref.package.0)
            .expect("package slot should exist while dumping");
        let target = package
            .target(
                self.project
                    .def_map_db()
                    .resident_package(crate_ref.package)
                    .and_then(|package| package.crate_data(crate_ref.crate_id))
                    .expect("semantic crate should exist while dumping")
                    .cargo_target(),
            )
            .expect("target id should exist while dumping");

        format!(
            "{}[{}]::{}",
            package.package_name(),
            target.kind,
            self.module_path(crate_ref, module_ref.module),
        )
    }

    fn module_path(&self, crate_ref: CrateRef, module_id: ModuleId) -> String {
        let module = self
            .project
            .resident_def_map(crate_ref)
            .expect("crate def map should exist while building relative module path")
            .module(module_id)
            .expect("module id should exist while building relative module path");

        match module.parent {
            Some(parent) => {
                let parent_path = self.module_path(crate_ref, parent);
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

fn sorted_targets(package: &Package) -> Vec<&CargoTarget> {
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
