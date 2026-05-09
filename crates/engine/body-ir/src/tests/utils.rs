use std::{
    fmt::{self, Write as _},
    marker::PhantomData,
    sync::Arc,
};

use expect_test::Expect;

use crate::{
    BindingData, BodyData, BodyFunctionData, BodyGenericArg, BodyImplData, BodyIrBuildPolicy,
    BodyIrDb, BodyIrReadTxn, BodyItemData, BodyLocalNominalTy, BodyNominalTy, BodyResolution,
    BodySource, BodyTy, ExprData, ExprKind, ResolvedFieldRef, ResolvedFunctionRef, StmtKind,
    TargetBodiesStatus,
    ids::{
        BindingId, BodyFieldRef, BodyFunctionId, BodyFunctionRef, BodyId, BodyImplId, BodyItemId,
        BodyItemRef, ExprId, StmtId,
    },
};
use rg_def_map::{DefId, DefMapDb, LocalDefRef, ModuleRef, TargetRef};
use rg_item_tree::ItemTreeDb;
use rg_package_store::{LoadPackage, PackageLoader, PackageStoreError};
use rg_parse::{Package, ParseDb, Target};
use rg_semantic_ir::{
    FieldRef, FunctionRef, ImplRef, ItemId, ItemOwner, SemanticIrDb, SemanticIrReadTxn, TraitRef,
    TypeDefId, TypeDefRef,
};
use rg_workspace::WorkspaceMetadata;
use test_fixture::{CrateFixture, fixture_crate};

pub(super) fn check_project_body_ir(fixture: &str, expect: Expect) {
    let db = BodyIrFixtureDb::build(fixture);
    let actual = ProjectBodyIrSnapshot::new(&db).render();
    let actual = format!("{}\n", actual.trim_end());
    expect.assert_eq(&actual);
}

pub(super) fn check_project_body_ir_with_policy(
    fixture: &str,
    policy: BodyIrBuildPolicy,
    expect: Expect,
) {
    let db = BodyIrFixtureDb::build_with_policy(fixture, policy);
    let actual = ProjectBodyIrSnapshot::new(&db).render();
    let actual = format!("{}\n", actual.trim_end());
    expect.assert_eq(&actual);
}

struct BodyIrFixtureDb {
    /// Keeps the temporary fixture files on disk while snapshots recover source text by span.
    _fixture: CrateFixture,
    parse: ParseDb,
    def_map: DefMapDb,
    semantic_ir: SemanticIrDb,
    body_ir: BodyIrDb,
}

impl BodyIrFixtureDb {
    fn build(fixture: &str) -> Self {
        Self::build_with_policy(fixture, BodyIrBuildPolicy::default())
    }

    fn build_with_policy(fixture: &str, policy: BodyIrBuildPolicy) -> Self {
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
        let body_ir = BodyIrDb::builder(&parse, &def_map, &semantic_ir)
            .policy(policy)
            .build()
            .expect("fixture body ir db should build");

        Self {
            _fixture: fixture,
            parse,
            def_map,
            semantic_ir,
            body_ir,
        }
    }

    fn parse_db(&self) -> &ParseDb {
        &self.parse
    }

    fn resident_def_map(&self, target: TargetRef) -> Option<&rg_def_map::DefMap> {
        self.def_map
            .resident_package(target.package)?
            .target(target.target)
    }

    fn semantic_ir_db(&self) -> &SemanticIrDb {
        &self.semantic_ir
    }

    fn resident_target_ir(&self, target: TargetRef) -> Option<&rg_semantic_ir::TargetIr> {
        self.semantic_ir
            .resident_package(target.package)?
            .target(target.target)
    }

    fn body_ir_db(&self) -> &BodyIrDb {
        &self.body_ir
    }
}

struct ProjectBodyIrSnapshot<'a> {
    project: &'a BodyIrFixtureDb,
}

impl<'a> ProjectBodyIrSnapshot<'a> {
    fn new(project: &'a BodyIrFixtureDb) -> Self {
        Self { project }
    }

    fn render(&self) -> String {
        sorted_packages(self.project.parse_db())
            .into_iter()
            .map(|(package_slot, package)| {
                let target_dumps = sorted_targets(package)
                    .into_iter()
                    .map(|target| {
                        TargetBodyIrSnapshot {
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

struct TargetBodyIrSnapshot<'a> {
    project: &'a BodyIrFixtureDb,
    target_ref: TargetRef,
    target_name: &'a str,
    target_kind: String,
}

impl TargetBodyIrSnapshot<'_> {
    fn render(&self) -> String {
        let mut dump = format!("{} [{}]", self.target_name, self.target_kind);
        let body_ir = self.body_ir_txn();
        let Some(target_bodies) = body_ir
            .target_bodies(self.target_ref)
            .expect("target body IR should load while rendering body IR")
        else {
            return dump;
        };

        if matches!(target_bodies.status(), TargetBodiesStatus::Skipped) {
            dump.push_str("\nskipped");
            return dump;
        }

        let mut bodies = target_bodies
            .bodies()
            .iter()
            .enumerate()
            .map(|(idx, body)| (self.render_function_ref(body.owner), BodyId(idx)))
            .collect::<Vec<_>>();
        bodies.sort_by(|left, right| left.0.cmp(&right.0));

        for (idx, (_, body_id)) in bodies.into_iter().enumerate() {
            if idx == 0 {
                dump.push('\n');
            } else {
                dump.push_str("\n\n");
            }

            let body = target_bodies
                .body(body_id)
                .expect("body id should exist while rendering body IR");
            self.render_body(body, body_id, &mut dump);
        }

        dump
    }

    fn render_body(&self, body: &BodyData, body_id: BodyId, dump: &mut String) {
        writeln!(
            dump,
            "body b{} {} @ {}",
            body_id.0,
            self.render_function_ref(body.owner),
            self.render_source(body.source),
        )
        .expect("string writes should not fail");

        writeln!(dump, "scopes").expect("string writes should not fail");
        for (idx, scope) in body.scopes.iter().enumerate() {
            let parent = scope
                .parent
                .map(|scope| format!("s{}", scope.0))
                .unwrap_or_else(|| "<none>".to_string());
            let bindings = if scope.bindings.is_empty() {
                "<none>".to_string()
            } else {
                scope
                    .bindings
                    .iter()
                    .map(|binding| format!("v{}", binding.0))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let items = if scope.local_items.is_empty() {
                String::new()
            } else {
                format!(
                    "; items {}",
                    scope
                        .local_items
                        .iter()
                        .map(|item| format!("i{}", item.0))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            let impls = if scope.local_impls.is_empty() {
                String::new()
            } else {
                format!(
                    "; impls {}",
                    scope
                        .local_impls
                        .iter()
                        .map(|impl_id| format!("m{}", impl_id.0))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            writeln!(dump, "- s{idx} parent {parent}: {bindings}{items}{impls}")
                .expect("string writes should not fail");
        }

        if !body.local_items.is_empty() {
            writeln!(dump, "items").expect("string writes should not fail");
            for (idx, item) in body.local_items.iter().enumerate() {
                self.render_local_item(BodyItemId(idx), item, dump);
            }
        }

        if !body.local_impls.is_empty() {
            writeln!(dump, "impls").expect("string writes should not fail");
            for (idx, impl_data) in body.local_impls.iter().enumerate() {
                self.render_local_impl(body, BodyImplId(idx), impl_data, dump);
            }
        }

        writeln!(dump, "bindings").expect("string writes should not fail");
        for (idx, binding) in body.bindings.iter().enumerate() {
            self.render_binding(body, BindingId(idx), binding, dump);
        }

        writeln!(dump, "body").expect("string writes should not fail");
        self.render_expr(body, body.root_expr, 0, dump);
    }

    fn render_local_item(&self, id: BodyItemId, item: &BodyItemData, dump: &mut String) {
        writeln!(
            dump,
            "- i{} {} {} @ {}",
            id.0,
            item.kind,
            item.name,
            self.render_source(item.source),
        )
        .expect("string writes should not fail");
    }

    fn render_local_impl(
        &self,
        body: &BodyData,
        id: BodyImplId,
        impl_data: &BodyImplData,
        dump: &mut String,
    ) {
        let self_item = impl_data
            .self_item
            .map(|item| self.render_body_item_ref(item))
            .unwrap_or_else(|| "<unresolved>".to_string());
        writeln!(
            dump,
            "- m{} impl {} => {} @ {}",
            id.0,
            impl_data.self_ty,
            self_item,
            self.render_source(impl_data.source),
        )
        .expect("string writes should not fail");

        for function in &impl_data.functions {
            let data = body
                .local_function(*function)
                .expect("body function id should exist while rendering local impl");
            self.render_body_function(*function, data, dump);
        }
    }

    fn render_body_function(
        &self,
        id: BodyFunctionId,
        function: &BodyFunctionData,
        dump: &mut String,
    ) {
        let params = function
            .declaration
            .params
            .iter()
            .map(|param| match &param.ty {
                Some(ty) => format!("{}: {ty}", param.pat),
                None => param.pat.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        let ret_ty = function
            .declaration
            .ret_ty
            .as_ref()
            .map(|ty| format!(" -> {ty}"))
            .unwrap_or_default();

        writeln!(dump, "  - f{} fn {}({params}){ret_ty}", id.0, function.name)
            .expect("string writes should not fail");
    }

    fn render_binding(
        &self,
        body: &BodyData,
        id: BindingId,
        binding: &BindingData,
        dump: &mut String,
    ) {
        let annotation = binding
            .annotation
            .as_ref()
            .map(|ty| format!(": {ty}"))
            .unwrap_or_default();
        let name = binding.name.as_deref().unwrap_or("<unsupported>");

        writeln!(
            dump,
            "- v{} {} {} `{}`{} => {} @ {}",
            id.0,
            binding.kind,
            name,
            self.render_source_text(binding.source),
            annotation,
            self.render_ty(&binding.ty),
            self.render_source(binding.source),
        )
        .expect("string writes should not fail");

        assert!(
            body.scope(binding.scope).is_some(),
            "binding scope should exist while rendering"
        );
    }

    fn render_statement(
        &self,
        body: &BodyData,
        statement: StmtId,
        depth: usize,
        dump: &mut String,
    ) {
        let data = body
            .statement(statement)
            .expect("statement id should exist while rendering body IR");

        match &data.kind {
            StmtKind::Let {
                scope: _,
                pat: _,
                bindings,
                annotation,
                initializer,
            } => {
                let bindings = bindings
                    .iter()
                    .map(|binding| format!("v{}", binding.0))
                    .collect::<Vec<_>>()
                    .join(", ");
                let annotation = annotation
                    .as_ref()
                    .map(|ty| format!(": {ty}"))
                    .unwrap_or_default();
                writeln!(
                    dump,
                    "{}stmt s{} let {bindings}{annotation} @ {}",
                    indent(depth),
                    statement.0,
                    self.render_source(data.source),
                )
                .expect("string writes should not fail");
                if let Some(initializer) = initializer {
                    writeln!(dump, "{}initializer", indent(depth + 1))
                        .expect("string writes should not fail");
                    self.render_expr(body, *initializer, depth + 2, dump);
                }
            }
            StmtKind::Expr {
                expr,
                has_semicolon,
            } => {
                let suffix = if *has_semicolon { ";" } else { "" };
                writeln!(
                    dump,
                    "{}stmt s{} expr{suffix} @ {}",
                    indent(depth),
                    statement.0,
                    self.render_source(data.source),
                )
                .expect("string writes should not fail");
                self.render_expr(body, *expr, depth + 1, dump);
            }
            StmtKind::Item { item } => {
                writeln!(
                    dump,
                    "{}stmt s{} item i{} @ {}",
                    indent(depth),
                    statement.0,
                    item.0,
                    self.render_source(data.source),
                )
                .expect("string writes should not fail");
            }
            StmtKind::Impl { impl_id } => {
                writeln!(
                    dump,
                    "{}stmt s{} impl m{} @ {}",
                    indent(depth),
                    statement.0,
                    impl_id.0,
                    self.render_source(data.source),
                )
                .expect("string writes should not fail");
            }
            StmtKind::ItemIgnored => {
                writeln!(
                    dump,
                    "{}stmt s{} item <ignored> @ {}",
                    indent(depth),
                    statement.0,
                    self.render_source(data.source),
                )
                .expect("string writes should not fail");
            }
        }
    }

    fn render_expr(&self, body: &BodyData, expr: ExprId, depth: usize, dump: &mut String) {
        let data = body
            .expr(expr)
            .expect("expr id should exist while rendering body IR");
        writeln!(
            dump,
            "{}expr e{} {}{} => {} @ {}",
            indent(depth),
            expr.0,
            self.render_expr_head(data),
            self.render_resolution(&data.resolution),
            self.render_ty(&data.ty),
            self.render_source(data.source),
        )
        .expect("string writes should not fail");

        match &data.kind {
            ExprKind::Block {
                statements, tail, ..
            } => {
                for statement in statements {
                    self.render_statement(body, *statement, depth + 1, dump);
                }
                if let Some(tail) = tail {
                    writeln!(dump, "{}tail", indent(depth + 1))
                        .expect("string writes should not fail");
                    self.render_expr(body, *tail, depth + 2, dump);
                }
            }
            ExprKind::Call { callee, args } => {
                if let Some(callee) = callee {
                    writeln!(dump, "{}callee", indent(depth + 1))
                        .expect("string writes should not fail");
                    self.render_expr(body, *callee, depth + 2, dump);
                }
                for arg in args {
                    writeln!(dump, "{}arg", indent(depth + 1))
                        .expect("string writes should not fail");
                    self.render_expr(body, *arg, depth + 2, dump);
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                if let Some(scrutinee) = scrutinee {
                    writeln!(dump, "{}scrutinee", indent(depth + 1))
                        .expect("string writes should not fail");
                    self.render_expr(body, *scrutinee, depth + 2, dump);
                }
                for arm in arms {
                    writeln!(dump, "{}arm s{}", indent(depth + 1), arm.scope.0)
                        .expect("string writes should not fail");
                    if let Some(expr) = arm.expr {
                        self.render_expr(body, expr, depth + 2, dump);
                    }
                }
            }
            ExprKind::MethodCall { receiver, args, .. } => {
                if let Some(receiver) = receiver {
                    writeln!(dump, "{}receiver", indent(depth + 1))
                        .expect("string writes should not fail");
                    self.render_expr(body, *receiver, depth + 2, dump);
                }
                for arg in args {
                    writeln!(dump, "{}arg", indent(depth + 1))
                        .expect("string writes should not fail");
                    self.render_expr(body, *arg, depth + 2, dump);
                }
            }
            ExprKind::Field { base, .. } => {
                if let Some(base) = base {
                    writeln!(dump, "{}base", indent(depth + 1))
                        .expect("string writes should not fail");
                    self.render_expr(body, *base, depth + 2, dump);
                }
            }
            ExprKind::Wrapper { inner, .. } => {
                if let Some(inner) = inner {
                    writeln!(dump, "{}inner", indent(depth + 1))
                        .expect("string writes should not fail");
                    self.render_expr(body, *inner, depth + 2, dump);
                }
            }
            ExprKind::Unknown { children, .. } => {
                for child in children {
                    writeln!(dump, "{}child", indent(depth + 1))
                        .expect("string writes should not fail");
                    self.render_expr(body, *child, depth + 2, dump);
                }
            }
            ExprKind::Path { .. } | ExprKind::Literal { .. } => {}
        }
    }

    fn render_expr_head(&self, data: &ExprData) -> String {
        match &data.kind {
            ExprKind::Block { scope, .. } => format!("block s{}", scope.0),
            ExprKind::Path { path } => format!("path {path}"),
            ExprKind::Call { .. } => "call".to_string(),
            ExprKind::Match { .. } => "match".to_string(),
            ExprKind::MethodCall { method_name, .. } => {
                format!("method_call {method_name}")
            }
            ExprKind::Field { field, .. } => {
                let field = field
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "<missing>".to_string());
                format!("field {field}")
            }
            ExprKind::Wrapper { kind, .. } => format!("wrapper {kind}"),
            ExprKind::Literal { kind } => {
                format!("literal {kind} `{}`", self.render_source_text(data.source))
            }
            ExprKind::Unknown { .. } => {
                format!("unknown `{}`", self.render_source_text(data.source))
            }
        }
    }

    fn render_resolution(&self, resolution: &BodyResolution) -> String {
        match resolution {
            BodyResolution::Local(binding) => format!(" -> local v{}", binding.0),
            BodyResolution::LocalItem(item) => {
                format!(" -> local item {}", self.render_body_item_ref(*item))
            }
            BodyResolution::Item(defs) if defs.is_empty() => " -> item <unresolved>".to_string(),
            BodyResolution::Item(defs) => {
                let mut defs = defs
                    .iter()
                    .map(|def| self.render_def(*def))
                    .collect::<Vec<_>>();
                defs.sort();
                format!(" -> item {}", defs.join(" | "))
            }
            BodyResolution::Field(fields) => {
                let mut fields = fields
                    .iter()
                    .map(|field| self.render_resolved_field_ref(*field))
                    .collect::<Vec<_>>();
                fields.sort();
                format!(" -> {}", fields.join(" | "))
            }
            BodyResolution::Function(functions) => {
                let mut functions = functions
                    .iter()
                    .map(|function| self.render_resolved_function_ref(*function))
                    .collect::<Vec<_>>();
                functions.sort();
                format!(" -> {}", functions.join(" | "))
            }
            BodyResolution::EnumVariant(variants) => {
                let mut variants = variants
                    .iter()
                    .map(|variant| self.render_enum_variant_ref(*variant))
                    .collect::<Vec<_>>();
                variants.sort();
                format!(" -> {}", variants.join(" | "))
            }
            BodyResolution::Method(functions) => {
                let mut functions = functions
                    .iter()
                    .map(|function| self.render_resolved_function_ref(*function))
                    .collect::<Vec<_>>();
                functions.sort();
                format!(" -> {}", functions.join(" | "))
            }
            BodyResolution::Unknown => String::new(),
        }
    }

    fn render_ty(&self, ty: &BodyTy) -> String {
        match ty {
            BodyTy::Unit => "()".to_string(),
            BodyTy::Never => "!".to_string(),
            BodyTy::Syntax(ty) => format!("syntax {ty}"),
            BodyTy::Reference(inner) => format!("&{}", self.render_ty(inner)),
            BodyTy::LocalNominal(items) => {
                let mut items = items
                    .iter()
                    .map(|ty| self.render_body_local_nominal_ty(ty))
                    .collect::<Vec<_>>();
                items.sort();
                format!("local nominal {}", items.join(" | "))
            }
            BodyTy::Nominal(types) => {
                let mut types = types
                    .iter()
                    .map(|ty| self.render_body_nominal_ty(ty))
                    .collect::<Vec<_>>();
                types.sort();
                format!("nominal {}", types.join(" | "))
            }
            BodyTy::SelfTy(types) => {
                let mut types = types
                    .iter()
                    .map(|ty| self.render_body_nominal_ty(ty))
                    .collect::<Vec<_>>();
                types.sort();
                format!("Self {}", types.join(" | "))
            }
            BodyTy::Unknown => "<unknown>".to_string(),
        }
    }

    fn render_body_local_nominal_ty(&self, ty: &BodyLocalNominalTy) -> String {
        format!(
            "{}{}",
            self.render_body_item_ref(ty.item),
            self.render_generic_args(&ty.args)
        )
    }

    fn render_body_nominal_ty(&self, ty: &BodyNominalTy) -> String {
        format!(
            "{}{}",
            self.render_type_def_ref(ty.def),
            self.render_generic_args(&ty.args)
        )
    }

    fn render_generic_args(&self, args: &[BodyGenericArg]) -> String {
        if args.is_empty() {
            return String::new();
        }

        format!(
            "<{}>",
            args.iter()
                .map(|arg| self.render_generic_arg(arg))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    fn render_generic_arg(&self, arg: &BodyGenericArg) -> String {
        match arg {
            BodyGenericArg::Type(ty) => self.render_ty(ty),
            BodyGenericArg::Lifetime(lifetime) => lifetime.clone(),
            BodyGenericArg::Const(value) => value.clone(),
            BodyGenericArg::AssocType { name, ty } => match ty {
                Some(ty) => format!("{name} = {}", self.render_ty(ty)),
                None => name.to_string(),
            },
            BodyGenericArg::Unsupported(text) => format!("<unsupported:{text}>"),
        }
    }

    fn render_body_item_ref(&self, item_ref: BodyItemRef) -> String {
        let body_ir = self.body_ir_txn();
        let Some(body) = body_ir
            .body_data(item_ref.body)
            .expect("body item ref should load while rendering body IR")
        else {
            return "<missing>".to_string();
        };
        let Some(item) = body.local_item(item_ref.item) else {
            return "<missing>".to_string();
        };

        format!(
            "{} {}::{} @ {}",
            item.kind,
            self.render_function_ref(body.owner),
            item.name,
            self.render_source(item.source),
        )
    }

    fn render_def(&self, def: DefId) -> String {
        match def {
            DefId::Module(module_ref) => format!("module {}", self.render_module_ref(module_ref)),
            DefId::Local(local_def) => self.render_local_def(local_def),
        }
    }

    fn render_local_def(&self, local_def: LocalDefRef) -> String {
        let Some(target_ir) = self.project.resident_target_ir(local_def.target) else {
            return "<missing>".to_string();
        };
        let Some(item_id) = target_ir.item_for_local_def(local_def.local_def) else {
            return "<unsupported>".to_string();
        };

        match item_id {
            ItemId::Struct(id) => {
                let data = target_ir
                    .items()
                    .struct_data(id)
                    .expect("struct id should exist while rendering body IR");
                format!(
                    "struct {}::{}",
                    self.render_module_ref(data.owner),
                    data.name
                )
            }
            ItemId::Union(id) => {
                let data = target_ir
                    .items()
                    .union_data(id)
                    .expect("union id should exist while rendering body IR");
                format!(
                    "union {}::{}",
                    self.render_module_ref(data.owner),
                    data.name
                )
            }
            ItemId::Enum(id) => {
                let data = target_ir
                    .items()
                    .enum_data(id)
                    .expect("enum id should exist while rendering body IR");
                format!("enum {}::{}", self.render_module_ref(data.owner), data.name)
            }
            ItemId::Trait(id) => {
                let data = target_ir
                    .items()
                    .trait_data(id)
                    .expect("trait id should exist while rendering body IR");
                format!(
                    "trait {}::{}",
                    self.render_module_ref(data.owner),
                    data.name
                )
            }
            ItemId::Function(id) => self.render_function_ref(FunctionRef {
                target: local_def.target,
                id,
            }),
            ItemId::TypeAlias(id) => {
                let data = target_ir
                    .items()
                    .type_alias_data(id)
                    .expect("type alias id should exist while rendering body IR");
                format!(
                    "type {}::{}",
                    self.render_owner(data.owner, local_def.target),
                    data.name
                )
            }
            ItemId::Const(id) => {
                let data = target_ir
                    .items()
                    .const_data(id)
                    .expect("const id should exist while rendering body IR");
                format!(
                    "const {}::{}",
                    self.render_owner(data.owner, local_def.target),
                    data.name
                )
            }
            ItemId::Static(id) => {
                let data = target_ir
                    .items()
                    .static_data(id)
                    .expect("static id should exist while rendering body IR");
                format!(
                    "static {}::{}",
                    self.render_module_ref(data.owner),
                    data.name
                )
            }
        }
    }

    fn render_type_def_ref(&self, ty: TypeDefRef) -> String {
        let target_ir = self
            .project
            .resident_target_ir(ty.target)
            .expect("target semantic IR should exist while rendering body type");

        match ty.id {
            TypeDefId::Struct(id) => {
                let data = target_ir
                    .items()
                    .struct_data(id)
                    .expect("struct id should exist while rendering body type");
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
                    .expect("enum id should exist while rendering body type");
                format!("enum {}::{}", self.render_module_ref(data.owner), data.name)
            }
            TypeDefId::Union(id) => {
                let data = target_ir
                    .items()
                    .union_data(id)
                    .expect("union id should exist while rendering body type");
                format!(
                    "union {}::{}",
                    self.render_module_ref(data.owner),
                    data.name
                )
            }
        }
    }

    fn render_field_ref(&self, field_ref: FieldRef) -> String {
        let semantic_ir = self.semantic_ir_txn();
        let data = semantic_ir
            .field_data(field_ref)
            .expect("field ref should load while rendering body IR")
            .expect("field ref should exist while rendering body IR");
        let name = data
            .field
            .key
            .as_ref()
            .map(|key| key.declaration_label())
            .unwrap_or_else(|| "<missing>".to_string());

        format!(
            "field {}::{name}",
            self.render_type_def_ref(field_ref.owner)
        )
    }

    fn render_resolved_field_ref(&self, field_ref: ResolvedFieldRef) -> String {
        match field_ref {
            ResolvedFieldRef::Semantic(field) => self.render_field_ref(field),
            ResolvedFieldRef::BodyLocal(field) => self.render_body_field_ref(field),
        }
    }

    fn render_body_field_ref(&self, field_ref: BodyFieldRef) -> String {
        let body_ir = self.body_ir_txn();
        let data = body_ir
            .local_field_data(field_ref)
            .expect("body field ref should load while rendering body IR")
            .expect("body field ref should exist while rendering body IR");
        let name = data
            .field
            .key
            .as_ref()
            .map(|key| key.declaration_label())
            .unwrap_or_else(|| "<missing>".to_string());

        format!(
            "field {}::{name}",
            self.render_body_item_ref(field_ref.item)
        )
    }

    fn render_enum_variant_ref(&self, variant_ref: rg_semantic_ir::EnumVariantRef) -> String {
        let semantic_ir = self.semantic_ir_txn();
        let data = semantic_ir
            .enum_variant_data(variant_ref)
            .expect("enum variant ref should load while rendering body IR")
            .expect("enum variant ref should exist while rendering body IR");

        format!(
            "variant {}::{}",
            self.render_type_def_ref(data.owner),
            data.variant.name
        )
    }

    fn render_resolved_function_ref(&self, function_ref: ResolvedFunctionRef) -> String {
        match function_ref {
            ResolvedFunctionRef::Semantic(function) => self.render_function_ref(function),
            ResolvedFunctionRef::BodyLocal(function) => self.render_body_function_ref(function),
        }
    }

    fn render_body_function_ref(&self, function_ref: BodyFunctionRef) -> String {
        let body_ir = self.body_ir_txn();
        let data = body_ir
            .local_function_data(function_ref)
            .expect("body function ref should load while rendering body IR")
            .expect("body function ref should exist while rendering body IR");

        format!("fn {}", data.name)
    }

    fn render_function_ref(&self, function_ref: FunctionRef) -> String {
        let semantic_ir = self.semantic_ir_txn();
        let data = semantic_ir
            .function_data(function_ref)
            .expect("function id should load while rendering body IR")
            .expect("function id should exist while rendering body IR");
        let owner = self.render_owner(data.owner, function_ref.target);

        format!("fn {owner}::{}", data.name)
    }

    fn render_owner(&self, owner: ItemOwner, target: TargetRef) -> String {
        match owner {
            ItemOwner::Module(module_ref) => self.render_module_ref(module_ref),
            ItemOwner::Trait(trait_id) => self.render_trait_ref(TraitRef {
                target,
                id: trait_id,
            }),
            ItemOwner::Impl(impl_id) => self.render_impl_ref(ImplRef {
                target,
                id: impl_id,
            }),
        }
    }

    fn render_trait_ref(&self, trait_ref: TraitRef) -> String {
        let semantic_ir = self.semantic_ir_txn();
        let data = semantic_ir
            .trait_data(trait_ref)
            .expect("trait id should load while rendering body IR")
            .expect("trait id should exist while rendering body IR");

        format!(
            "trait {}::{}",
            self.render_module_ref(data.owner),
            data.name
        )
    }

    fn render_impl_ref(&self, impl_ref: ImplRef) -> String {
        let semantic_ir = self.semantic_ir_txn();
        let data = semantic_ir
            .impl_data(impl_ref)
            .expect("impl id should load while rendering body IR")
            .expect("impl id should exist while rendering body IR");

        match &data.trait_ref {
            Some(trait_ref) => format!("impl {trait_ref} for {}", data.self_ty),
            None => format!("impl {}", data.self_ty),
        }
    }

    fn semantic_ir_txn(&self) -> SemanticIrReadTxn<'_> {
        self.project
            .semantic_ir_db()
            .read_txn(unexpected_package_loader())
    }

    fn body_ir_txn(&self) -> BodyIrReadTxn<'_> {
        self.project
            .body_ir_db()
            .read_txn(unexpected_package_loader())
    }

    fn render_module_ref(&self, module_ref: ModuleRef) -> String {
        let package = self
            .project
            .parse_db()
            .packages()
            .get(module_ref.target.package.0)
            .expect("package slot should exist while rendering body IR module");
        let target = package
            .target(module_ref.target.target)
            .expect("target id should exist while rendering body IR module");

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
            .expect("target def map should exist while rendering body IR module path")
            .module(module_ref.module)
            .expect("module id should exist while rendering body IR module path");

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

    fn render_source(&self, source: BodySource) -> String {
        let line_column = source.span.line_column(
            self.project
                .parse_db()
                .package(self.target_ref.package.0)
                .expect("source package should exist while rendering body IR source")
                .parsed_file(source.file_id)
                .expect("source file should exist while rendering body IR source")
                .line_index()
                .expect("source file line index should load while rendering body IR source"),
        );
        format!(
            "{}:{}-{}:{}",
            line_column.start.line + 1,
            line_column.start.column + 1,
            line_column.end.line + 1,
            line_column.end.column + 1,
        )
    }

    fn render_source_text(&self, source: BodySource) -> String {
        let parsed_file = self
            .project
            .parse_db()
            .package(self.target_ref.package.0)
            .expect("source package should exist while rendering body IR text")
            .parsed_file(source.file_id)
            .expect("source file should exist while rendering body IR text");

        parsed_file
            .text_for_span(source.span)
            .unwrap_or_else(|| "<invalid>".to_string())
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
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
    fn load(&self, package: rg_def_map::PackageSlot) -> Result<Arc<T>, PackageStoreError> {
        panic!(
            "resident body IR fixture should not load offloaded package {}",
            package.0,
        )
    }
}
