use std::{
    fmt::{self, Write as _},
    marker::PhantomData,
    sync::Arc,
};

use expect_test::Expect;

use crate::{
    ItemStore, SemanticIrDb, SemanticIrReadTxn,
    ids::{FunctionRef, ImplRef, TraitRef, TypeDefId, TypeDefRef},
};
use rg_def_map::{DefMapDb, ModuleId, ModuleRef, PackageSlot, Path, PathSegment, TargetRef};
use rg_item_tree::{FieldItem, FieldList, ItemTreeDb, ParamKind, VisibilityLevel};
use rg_package_store::{LoadPackage, PackageLoader, PackageStoreError};
use rg_parse::{Package, ParseDb, Target};
use rg_workspace::{TargetKind, WorkspaceMetadata};
use test_fixture::fixture_crate;

use crate::ids::{AssocItemId, ConstId, FunctionId, ImplId, ItemId, TypeAliasId};

pub(super) fn check_project_semantic_ir(fixture: &str, expect: Expect) {
    let db = SemanticIrFixtureDb::build(fixture);
    let actual = ProjectSemanticIrSnapshot::new(&db).render();
    let actual = format!("{}\n", actual.trim_end());
    expect.assert_eq(&actual);
}

pub(super) fn check_project_semantic_queries(
    fixture: &str,
    queries: &[SemanticQuery],
    expect: Expect,
) {
    let db = SemanticIrFixtureDb::build(fixture);
    let actual = ProjectSemanticQuerySnapshot::new(&db, queries).render();
    let actual = format!("{}\n", actual.trim_end());
    expect.assert_eq(&actual);
}

pub(super) struct SemanticQuery {
    package_name: &'static str,
    target_kind: TargetKind,
    module_path: &'static str,
    path: &'static str,
}

impl SemanticQuery {
    pub(super) fn lib(package_name: &'static str, path: &'static str) -> Self {
        Self::lib_from(package_name, "crate", path)
    }

    pub(super) fn lib_from(
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

    pub(super) fn bin(package_name: &'static str, path: &'static str) -> Self {
        Self::bin_from(package_name, "crate", path)
    }

    pub(super) fn bin_from(
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

struct SemanticIrFixtureDb {
    parse: ParseDb,
    def_map: DefMapDb,
    semantic_ir: SemanticIrDb,
}

impl SemanticIrFixtureDb {
    fn build(fixture: &str) -> Self {
        let fixture = fixture_crate(fixture);
        let workspace = WorkspaceMetadata::from_cargo(fixture.metadata())
            .expect("fixture workspace metadata should build");
        let mut parse = ParseDb::build(&workspace).expect("fixture parse db should build");
        let item_tree = ItemTreeDb::build(&mut parse).expect("fixture item tree db should build");
        let def_map = DefMapDb::builder(&workspace, &parse, &item_tree)
            .build()
            .expect("fixture def map db should build");
        let semantic_ir = SemanticIrDb::builder(&item_tree, &def_map)
            .build()
            .expect("fixture semantic ir db should build");

        Self {
            parse,
            def_map,
            semantic_ir,
        }
    }

    fn parse_db(&self) -> &ParseDb {
        &self.parse
    }

    fn def_map_db(&self) -> &DefMapDb {
        &self.def_map
    }

    fn resident_def_map(&self, target: TargetRef) -> Option<&rg_def_map::DefMap> {
        self.def_map
            .resident_package(target.package)?
            .target(target.target)
    }

    fn semantic_ir_db(&self) -> &SemanticIrDb {
        &self.semantic_ir
    }

    fn resident_target_ir(&self, target: TargetRef) -> Option<&crate::TargetIr> {
        self.semantic_ir
            .resident_package(target.package)?
            .target(target.target)
    }
}

struct ProjectSemanticIrSnapshot<'a> {
    project: &'a SemanticIrFixtureDb,
}

impl<'a> ProjectSemanticIrSnapshot<'a> {
    fn new(project: &'a SemanticIrFixtureDb) -> Self {
        Self { project }
    }

    fn render(&self) -> String {
        sorted_packages(self.project.parse_db())
            .into_iter()
            .map(|(package_slot, package)| {
                let target_dumps = sorted_targets(package)
                    .into_iter()
                    .map(|target| {
                        TargetSemanticIrSnapshot {
                            project: self.project,
                            target_ref: TargetRef {
                                package: rg_def_map::PackageSlot(package_slot),
                                target: target.id,
                            },
                            target_name: &target.name,
                            target_kind: target.kind.to_string(),
                        }
                        .render()
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");

                format!("package {}\n\n{target_dumps}", package.package_name())
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

struct ProjectSemanticQuerySnapshot<'a> {
    project: &'a SemanticIrFixtureDb,
    queries: &'a [SemanticQuery],
}

impl<'a> ProjectSemanticQuerySnapshot<'a> {
    fn new(project: &'a SemanticIrFixtureDb, queries: &'a [SemanticQuery]) -> Self {
        Self { project, queries }
    }

    fn render(&self) -> String {
        self.queries
            .iter()
            .map(|query| self.render_query(query))
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn render_query(&self, query: &SemanticQuery) -> String {
        let (target_ref, target) = self.target_ref(query);
        let module_id = self.module_id(target_ref, query.module_path);
        let path = Self::parse_path(query.path);
        let def_map_txn = self
            .project
            .def_map_db()
            .read_txn(unexpected_package_loader());
        let semantic_ir_txn = self
            .project
            .semantic_ir_db()
            .read_txn(unexpected_package_loader());
        let mut type_defs = semantic_ir_txn
            .type_defs_for_path(
                &def_map_txn,
                ModuleRef {
                    target: target_ref,
                    module: module_id,
                },
                &path,
            )
            .expect("fixture semantic query should resolve type path");
        type_defs.sort_by_key(|ty| self.render_type_def_ref(&semantic_ir_txn, *ty));

        if type_defs.is_empty() {
            return format!(
                "query {} [{}] {} resolves {} -> <unresolved>",
                query.package_name, target.kind, query.module_path, path,
            );
        }

        type_defs
            .into_iter()
            .map(|ty| {
                let mut dump = format!(
                    "query {} [{}] {} resolves {} -> {}",
                    query.package_name,
                    target.kind,
                    query.module_path,
                    path,
                    self.render_type_def_ref(&semantic_ir_txn, ty),
                );
                self.render_query_section(
                    &mut dump,
                    "impls",
                    semantic_ir_txn
                        .impls_for_type(ty)
                        .expect("fixture semantic query should find impls for type")
                        .into_iter()
                        .map(|impl_ref| self.render_impl_ref(&semantic_ir_txn, impl_ref))
                        .collect(),
                );
                self.render_query_section(
                    &mut dump,
                    "trait impls",
                    semantic_ir_txn
                        .trait_impls_for_type(ty)
                        .expect("fixture semantic query should find trait impls for type")
                        .into_iter()
                        .map(|trait_impl| {
                            format!(
                                "{} => {}",
                                self.render_impl_ref(&semantic_ir_txn, trait_impl.impl_ref),
                                self.render_trait_ref(&semantic_ir_txn, trait_impl.trait_ref),
                            )
                        })
                        .collect(),
                );
                self.render_query_section(
                    &mut dump,
                    "traits",
                    semantic_ir_txn
                        .traits_for_type(ty)
                        .expect("fixture semantic query should find traits for type")
                        .into_iter()
                        .map(|trait_ref| self.render_trait_ref(&semantic_ir_txn, trait_ref))
                        .collect(),
                );
                self.render_query_section(
                    &mut dump,
                    "inherent functions",
                    semantic_ir_txn
                        .inherent_functions_for_type(ty)
                        .expect("fixture semantic query should find inherent functions for type")
                        .into_iter()
                        .map(|function_ref| {
                            self.render_function_ref(&semantic_ir_txn, function_ref)
                        })
                        .collect(),
                );
                self.render_query_section(
                    &mut dump,
                    "trait functions",
                    semantic_ir_txn
                        .trait_functions_for_type(ty)
                        .expect("fixture semantic query should find trait functions for type")
                        .into_iter()
                        .map(|function_ref| {
                            self.render_function_ref(&semantic_ir_txn, function_ref)
                        })
                        .collect(),
                );
                self.render_query_section(
                    &mut dump,
                    "trait impl functions",
                    semantic_ir_txn
                        .trait_impl_functions_for_type(ty)
                        .expect("fixture semantic query should find trait impl functions for type")
                        .into_iter()
                        .map(|function_ref| {
                            self.render_function_ref(&semantic_ir_txn, function_ref)
                        })
                        .collect(),
                );
                dump
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn render_query_section(&self, dump: &mut String, title: &str, mut lines: Vec<String>) {
        if !dump.ends_with('\n') {
            writeln!(dump).expect("string writes should not fail");
        }
        writeln!(dump, "{title}").expect("string writes should not fail");
        lines.sort();

        if lines.is_empty() {
            writeln!(dump, "- <none>").expect("string writes should not fail");
            return;
        }

        for line in lines {
            writeln!(dump, "- {line}").expect("string writes should not fail");
        }
    }

    fn target_ref(&self, query: &SemanticQuery) -> (TargetRef, &'a rg_parse::Target) {
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
                package: rg_def_map::PackageSlot(package_slot),
                target: target.id,
            },
            target,
        )
    }

    fn module_id(&self, target_ref: TargetRef, module_path: &str) -> ModuleId {
        let def_map = self
            .project
            .resident_def_map(target_ref)
            .expect("target def map should exist while rendering semantic query");

        def_map
            .modules()
            .iter()
            .enumerate()
            .find_map(|(module_idx, _)| {
                let module_id = ModuleId(module_idx);
                (self.module_path(ModuleRef {
                    target: target_ref,
                    module: module_id,
                }) == module_path)
                    .then_some(module_id)
            })
            .unwrap_or_else(|| panic!("module `{module_path}` should exist in fixture target"))
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

    fn render_type_def_ref(&self, semantic_ir: &SemanticIrReadTxn<'_>, ty: TypeDefRef) -> String {
        let target_ir = semantic_ir
            .target_ir(ty.target)
            .expect("target semantic IR should load while rendering type ref")
            .expect("target semantic IR should exist while rendering type ref");

        match ty.id {
            TypeDefId::Struct(id) => {
                let data = target_ir
                    .items()
                    .struct_data(id)
                    .expect("struct id should exist while rendering query");
                format!(
                    "struct {}::{}",
                    self.render_module_ref(data.owner),
                    data.name
                )
            }
            TypeDefId::Enum(id) => {
                let data = target_ir
                    .items()
                    .enum_data(id)
                    .expect("enum id should exist while rendering query");
                format!("enum {}::{}", self.render_module_ref(data.owner), data.name)
            }
            TypeDefId::Union(id) => {
                let data = target_ir
                    .items()
                    .union_data(id)
                    .expect("union id should exist while rendering query");
                format!(
                    "union {}::{}",
                    self.render_module_ref(data.owner),
                    data.name
                )
            }
        }
    }

    fn render_trait_ref(&self, semantic_ir: &SemanticIrReadTxn<'_>, trait_ref: TraitRef) -> String {
        let data = semantic_ir
            .trait_data(trait_ref)
            .expect("trait id should load while rendering query")
            .expect("trait id should exist while rendering query");

        format!(
            "trait {}::{}",
            self.render_module_ref(data.owner),
            data.name
        )
    }

    fn render_impl_ref(&self, semantic_ir: &SemanticIrReadTxn<'_>, impl_ref: ImplRef) -> String {
        let data = semantic_ir
            .impl_data(impl_ref)
            .expect("impl id should load while rendering query")
            .expect("impl id should exist while rendering query");

        match &data.trait_ref {
            Some(trait_ref) => format!("impl {trait_ref} for {}", data.self_ty),
            None => format!("impl {}", data.self_ty),
        }
    }

    fn render_function_ref(
        &self,
        semantic_ir: &SemanticIrReadTxn<'_>,
        function_ref: FunctionRef,
    ) -> String {
        let data = semantic_ir
            .function_data(function_ref)
            .expect("function id should load while rendering query")
            .expect("function id should exist while rendering query");
        let owner = match data.owner {
            crate::ids::ItemOwner::Module(module_ref) => self.render_module_ref(module_ref),
            crate::ids::ItemOwner::Trait(trait_id) => self.render_trait_ref(
                semantic_ir,
                TraitRef {
                    target: function_ref.target,
                    id: trait_id,
                },
            ),
            crate::ids::ItemOwner::Impl(impl_id) => self.render_impl_ref(
                semantic_ir,
                ImplRef {
                    target: function_ref.target,
                    id: impl_id,
                },
            ),
        };

        format!("fn {owner}::{}", data.name)
    }

    fn render_module_ref(&self, module_ref: ModuleRef) -> String {
        let package = self
            .project
            .parse_db()
            .packages()
            .get(module_ref.target.package.0)
            .expect("package slot should exist while rendering query");
        let target = package
            .target(module_ref.target.target)
            .expect("target id should exist while rendering query");

        format!(
            "{}[{}]::{}",
            package.package_name(),
            target.kind,
            self.module_path(module_ref),
        )
    }

    fn module_path(&self, module_ref: ModuleRef) -> String {
        let module = self
            .project
            .resident_def_map(module_ref.target)
            .expect("target def map should exist while rendering query module path")
            .module(module_ref.module)
            .expect("module id should exist while rendering query module path");

        match module.parent {
            Some(parent) => {
                let parent_path = self.module_path(ModuleRef {
                    target: module_ref.target,
                    module: parent,
                });
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

struct TargetSemanticIrSnapshot<'a> {
    project: &'a SemanticIrFixtureDb,
    target_ref: TargetRef,
    target_name: &'a str,
    target_kind: String,
}

impl TargetSemanticIrSnapshot<'_> {
    fn render(&self) -> String {
        let mut dump = format!("{} [{}]\n", self.target_name, self.target_kind);
        let def_map = self
            .project
            .resident_def_map(self.target_ref)
            .expect("target def map should exist while rendering semantic IR");
        let target_ir = self
            .project
            .resident_target_ir(self.target_ref)
            .expect("target semantic IR should exist while rendering");

        for (idx, (module_path, module_id)) in self.sorted_modules().into_iter().enumerate() {
            if idx > 0 {
                dump.push('\n');
            }

            writeln!(&mut dump, "{module_path}").expect("string writes should not fail");
            let module = def_map
                .module(module_id)
                .expect("module id should exist while rendering semantic IR");

            for local_def in &module.local_defs {
                let Some(item_id) = target_ir.item_for_local_def(*local_def) else {
                    continue;
                };
                self.render_item(item_id, 0, &mut dump);
            }

            for local_impl in &module.impls {
                let impl_id = target_ir
                    .impls()
                    .get(local_impl.0)
                    .copied()
                    .expect("local impl id should map to semantic impl id");
                self.render_impl(impl_id, 0, &mut dump);
            }
        }

        dump
    }

    fn render_item(&self, item_id: ItemId, depth: usize, dump: &mut String) {
        match item_id {
            ItemId::Struct(id) => {
                let data = self
                    .items()
                    .struct_data(id)
                    .expect("struct id should exist while rendering");
                writeln!(
                    dump,
                    "{}- {}struct {}{}",
                    indent(depth),
                    visibility_prefix(&data.visibility),
                    data.name,
                    generic_params(&data.generics),
                )
                .expect("string writes should not fail");
                self.render_fields(&data.fields, depth + 1, dump);
            }
            ItemId::Union(id) => {
                let data = self
                    .items()
                    .union_data(id)
                    .expect("union id should exist while rendering");
                writeln!(
                    dump,
                    "{}- {}union {}{}",
                    indent(depth),
                    visibility_prefix(&data.visibility),
                    data.name,
                    generic_params(&data.generics),
                )
                .expect("string writes should not fail");
                self.render_named_fields(&data.fields, depth + 1, dump);
            }
            ItemId::Enum(id) => {
                let data = self
                    .items()
                    .enum_data(id)
                    .expect("enum id should exist while rendering");
                writeln!(
                    dump,
                    "{}- {}enum {}{}",
                    indent(depth),
                    visibility_prefix(&data.visibility),
                    data.name,
                    generic_params(&data.generics),
                )
                .expect("string writes should not fail");
                for variant in &data.variants {
                    writeln!(dump, "{}- variant {}", indent(depth + 1), variant.name)
                        .expect("string writes should not fail");
                    self.render_fields(&variant.fields, depth + 2, dump);
                }
            }
            ItemId::Trait(id) => {
                let data = self
                    .items()
                    .trait_data(id)
                    .expect("trait id should exist while rendering");
                let super_traits = if data.super_traits.is_empty() {
                    String::new()
                } else {
                    format!(
                        ": {}",
                        data.super_traits
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(" + ")
                    )
                };
                writeln!(
                    dump,
                    "{}- {}trait {}{}{}{}",
                    indent(depth),
                    visibility_prefix(&data.visibility),
                    data.name,
                    generic_params(&data.generics),
                    super_traits,
                    where_clause(&data.generics),
                )
                .expect("string writes should not fail");
                for assoc_item in &data.items {
                    self.render_assoc_item(*assoc_item, depth + 1, dump);
                }
            }
            ItemId::Function(id) => self.render_function(id, depth, dump),
            ItemId::TypeAlias(id) => self.render_type_alias(id, depth, dump),
            ItemId::Const(id) => self.render_const(id, depth, dump),
            ItemId::Static(id) => {
                let data = self
                    .items()
                    .static_data(id)
                    .expect("static id should exist while rendering");
                let mutability = match data.mutability {
                    rg_item_tree::Mutability::Shared => "",
                    rg_item_tree::Mutability::Mutable => "mut ",
                };
                let ty = data
                    .ty
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "<unknown>".to_string());
                writeln!(
                    dump,
                    "{}- {}static {mutability}{}: {ty}",
                    indent(depth),
                    visibility_prefix(&data.visibility),
                    data.name,
                )
                .expect("string writes should not fail");
            }
        }
    }

    fn render_impl(&self, id: ImplId, depth: usize, dump: &mut String) {
        let data = self
            .items()
            .impl_data(id)
            .expect("impl id should exist while rendering");
        match &data.trait_ref {
            Some(trait_ref) => writeln!(
                dump,
                "{}- impl{} {} for {}{}",
                indent(depth),
                generic_params(&data.generics),
                trait_ref,
                data.self_ty,
                where_clause(&data.generics),
            )
            .expect("string writes should not fail"),
            None => writeln!(
                dump,
                "{}- impl{} {}{}",
                indent(depth),
                generic_params(&data.generics),
                data.self_ty,
                where_clause(&data.generics),
            )
            .expect("string writes should not fail"),
        }
        for assoc_item in &data.items {
            self.render_assoc_item(*assoc_item, depth + 1, dump);
        }
    }

    fn render_assoc_item(&self, item_id: AssocItemId, depth: usize, dump: &mut String) {
        match item_id {
            AssocItemId::Function(id) => self.render_function(id, depth, dump),
            AssocItemId::TypeAlias(id) => self.render_type_alias(id, depth, dump),
            AssocItemId::Const(id) => self.render_const(id, depth, dump),
        }
    }

    fn render_function(&self, id: FunctionId, depth: usize, dump: &mut String) {
        let data = self
            .items()
            .function_data(id)
            .expect("function id should exist while rendering");
        let params = data
            .signature
            .params()
            .iter()
            .map(render_param)
            .collect::<Vec<_>>()
            .join(", ");
        let ret_ty = data
            .signature
            .ret_ty()
            .map(|ty| format!(" -> {ty}"))
            .unwrap_or_default();
        let generics = data.signature.generics();
        writeln!(
            dump,
            "{}- {}fn {}{}({params}){ret_ty}{}",
            indent(depth),
            visibility_prefix(&data.visibility),
            data.name,
            generic_params_opt(generics),
            where_clause_opt(generics),
        )
        .expect("string writes should not fail");
    }

    fn render_type_alias(&self, id: TypeAliasId, depth: usize, dump: &mut String) {
        let data = self
            .items()
            .type_alias_data(id)
            .expect("type alias id should exist while rendering");
        let bounds = if data.signature.bounds().is_empty() {
            String::new()
        } else {
            format!(
                ": {}",
                data.signature
                    .bounds()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" + ")
            )
        };
        let aliased_ty = data
            .signature
            .aliased_ty()
            .map(|ty| format!(" = {ty}"))
            .unwrap_or_default();
        let generics = data.signature.generics();
        writeln!(
            dump,
            "{}- {}type {}{}{}{}{}",
            indent(depth),
            visibility_prefix(&data.visibility),
            data.name,
            generic_params_opt(generics),
            bounds,
            where_clause_opt(generics),
            aliased_ty,
        )
        .expect("string writes should not fail");
    }

    fn render_const(&self, id: ConstId, depth: usize, dump: &mut String) {
        let data = self
            .items()
            .const_data(id)
            .expect("const id should exist while rendering");
        let ty = data
            .signature
            .ty()
            .map(ToString::to_string)
            .unwrap_or_else(|| "<unknown>".to_string());
        writeln!(
            dump,
            "{}- {}const {}: {ty}",
            indent(depth),
            visibility_prefix(&data.visibility),
            data.name,
        )
        .expect("string writes should not fail");
    }

    fn render_fields(&self, fields: &FieldList, depth: usize, dump: &mut String) {
        match fields {
            FieldList::Named(fields) => self.render_named_fields(fields, depth, dump),
            FieldList::Tuple(fields) => {
                for (idx, field) in fields.iter().enumerate() {
                    writeln!(
                        dump,
                        "{}- {}field #{idx}: {}",
                        indent(depth),
                        visibility_prefix(&field.visibility),
                        field.ty,
                    )
                    .expect("string writes should not fail");
                }
            }
            FieldList::Unit => {}
        }
    }

    fn render_named_fields(&self, fields: &[FieldItem], depth: usize, dump: &mut String) {
        for field in fields {
            writeln!(
                dump,
                "{}- {}field {}: {}",
                indent(depth),
                visibility_prefix(&field.visibility),
                field
                    .key
                    .as_ref()
                    .map(|key| key.declaration_label())
                    .unwrap_or_else(|| "<missing>".to_string()),
                field.ty,
            )
            .expect("string writes should not fail");
        }
    }

    fn sorted_modules(&self) -> Vec<(String, ModuleId)> {
        let def_map = self
            .project
            .resident_def_map(self.target_ref)
            .expect("target def map should exist while sorting semantic IR modules");
        let mut modules = def_map
            .modules()
            .iter()
            .enumerate()
            .map(|(idx, _)| {
                let module_id = ModuleId(idx);
                (self.module_path(module_id), module_id)
            })
            .collect::<Vec<_>>();
        modules.sort_by(|left, right| left.0.cmp(&right.0));
        modules
    }

    fn module_path(&self, module_id: ModuleId) -> String {
        let module = self
            .project
            .resident_def_map(self.target_ref)
            .expect("target def map should exist while rendering module path")
            .module(module_id)
            .expect("module id should exist while rendering module path");

        match module.parent {
            Some(parent) => {
                let parent_path = self.module_path(parent);
                let name = module
                    .name
                    .as_deref()
                    .expect("non-root modules should have names");
                format!("{parent_path}::{name}")
            }
            None => "crate".to_string(),
        }
    }

    fn items(&self) -> &ItemStore {
        self.project
            .resident_target_ir(self.target_ref)
            .expect("target semantic IR should exist while rendering items")
            .items()
    }
}

fn render_param(param: &rg_item_tree::ParamItem) -> String {
    match (param.kind, &param.ty) {
        (ParamKind::SelfParam, _) => param.pat.clone(),
        (ParamKind::Normal, Some(ty)) => format!("{}: {ty}", param.pat),
        (ParamKind::Normal, None) => param.pat.clone(),
    }
}

fn generic_params(generics: &rg_item_tree::GenericParams) -> String {
    let mut generics = generics.clone();
    generics.where_predicates.clear();
    generics.to_string()
}

fn generic_params_opt(generics: Option<&rg_item_tree::GenericParams>) -> String {
    generics.map(generic_params).unwrap_or_default()
}

fn where_clause(generics: &rg_item_tree::GenericParams) -> String {
    if generics.where_predicates.is_empty() {
        return String::new();
    }

    format!(
        " where {}",
        generics
            .where_predicates
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn where_clause_opt(generics: Option<&rg_item_tree::GenericParams>) -> String {
    generics.map(where_clause).unwrap_or_default()
}

fn visibility_prefix(visibility: &VisibilityLevel) -> String {
    match visibility {
        VisibilityLevel::Private => String::new(),
        _ => format!("{visibility} "),
    }
}

fn indent(depth: usize) -> String {
    "  ".repeat(depth)
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
    fn load(&self, package: PackageSlot) -> Result<Arc<T>, PackageStoreError> {
        panic!(
            "resident semantic IR fixture should not load offloaded package {}",
            package.0,
        )
    }
}
