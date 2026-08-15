//! User-typing states for candidate providers that share a semantic completion site.

use expect_test::expect;

use super::super::super::utils::{AnalysisQuery, check_analysis_queries};

/// Candidate providers must not accidentally rely on the terminator that follows their site.
///
/// The first half stops before the statement semicolon. The second half keeps syntax after the
/// cursor, which exercises replacement inside an expression or annotation that the user returned
/// to edit.
#[test]
fn completes_each_candidate_domain_across_realistic_line_states() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod module {
    pub struct ModuleType;
}

macro_rules! local_macro {
    () => { 1u8 };
}

#[macro_export]
macro_rules! exported_macro {
    () => { 2u8 };
}

pub struct MemberTarget {
    pub field_name: u8,
}

impl MemberTarget {
    pub fn new() -> Self {
        Self { field_name: 0 }
    }

    pub fn method_name(&self) {}
}

pub fn lexical_unfinished(local_value: u8) {
    let _ = local_v$lexical_unfinished$
}

pub fn generic_unfinished<GenericTy>() {
    let _: Gener$generic_unfinished$
}

pub fn primitive_unfinished() {
    let _: u1$primitive_unfinished$
}

pub fn macro_unfinished() {
    let _ = local_m$macro_unfinished$
}

pub fn qualified_macro_unfinished() {
    let _ = crate::exported_m$qualified_macro_unfinished$
}

pub fn module_unfinished() {
    let _: crate::module::ModuleT$module_unfinished$
}

pub fn associated_unfinished() {
    let _ = MemberTarget::ne$associated_unfinished$
}

pub fn field_unfinished(target: MemberTarget) {
    let _ = target.field_n$field_unfinished$
}

pub fn method_unfinished(target: MemberTarget) {
    target.method_n$method_unfinished$
}

pub fn lexical_edit(local_value: u8) {
    let _ = local_v$lexical_edit$ + 1;
}

pub fn generic_edit<GenericTy>() {
    let _: Gener$generic_edit$ = todo!();
}

pub fn primitive_edit() {
    let _: u1$primitive_edit$ = 0;
}

pub fn macro_edit() {
    let _ = local_m$macro_edit$!();
}

pub fn qualified_macro_edit() {
    let _ = crate::exported_m$qualified_macro_edit$!();
}

pub fn module_edit() {
    let _: crate::module::ModuleT$module_edit$ = todo!();
}

pub fn associated_edit() {
    let _ = MemberTarget::ne$associated_edit$();
}

pub fn field_edit(target: MemberTarget) {
    let _ = target.field_n$field_edit$ + 1;
}

pub fn method_edit(target: MemberTarget) {
    target.method_n$method_edit$();
}
"#,
        &[
            query(
                "unfinished lexical candidate",
                "lexical_unfinished",
                "local_value",
            ),
            query(
                "unfinished generic candidate",
                "generic_unfinished",
                "GenericTy",
            ),
            query(
                "unfinished primitive candidate",
                "primitive_unfinished",
                "u16",
            ),
            query(
                "unfinished macro candidate",
                "macro_unfinished",
                "local_macro",
            ),
            query(
                "unfinished qualified macro candidate",
                "qualified_macro_unfinished",
                "exported_macro",
            ),
            query(
                "unfinished module candidate",
                "module_unfinished",
                "ModuleType",
            ),
            query(
                "unfinished associated candidate",
                "associated_unfinished",
                "new",
            ),
            query(
                "unfinished field candidate",
                "field_unfinished",
                "field_name",
            ),
            query(
                "unfinished method candidate",
                "method_unfinished",
                "method_name",
            ),
            query("edited lexical candidate", "lexical_edit", "local_value"),
            query("edited generic candidate", "generic_edit", "GenericTy"),
            query("edited primitive candidate", "primitive_edit", "u16"),
            query("edited macro candidate", "macro_edit", "local_macro"),
            query(
                "edited qualified macro candidate",
                "qualified_macro_edit",
                "exported_macro",
            ),
            query("edited module candidate", "module_edit", "ModuleType"),
            query("edited associated candidate", "associated_edit", "new"),
            query("edited field candidate", "field_edit", "field_name"),
            query("edited method candidate", "method_edit", "method_name"),
        ],
        expect![[r#"
            unfinished lexical candidate
            - variable local_value

            unfinished generic candidate
            - type_parameter GenericTy

            unfinished primitive candidate
            - primitive_type u16

            unfinished macro candidate
            - macro local_macro

            unfinished qualified macro candidate
            - macro exported_macro

            unfinished module candidate
            - struct ModuleType

            unfinished associated candidate
            - fn new

            unfinished field candidate
            - field field_name

            unfinished method candidate
            - inherent_method method_name

            edited lexical candidate
            - variable local_value

            edited generic candidate
            - type_parameter GenericTy

            edited primitive candidate
            - primitive_type u16

            edited macro candidate
            - macro local_macro

            edited qualified macro candidate
            - macro exported_macro

            edited module candidate
            - struct ModuleType

            edited associated candidate
            - fn new

            edited field candidate
            - field field_name

            edited method candidate
            - inherent_method method_name
        "#]],
    );
}

#[test]
#[ignore = "TODO(#160)"]
fn completes_auto_import_candidates_across_realistic_line_states() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[workspace]
members = ["catalog", "app"]
resolver = "3"

//- /catalog/Cargo.toml
[package]
name = "catalog"
version = "0.1.0"
edition = "2024"

//- /catalog/src/lib.rs
pub struct AutoImportedType;

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
catalog = { path = "../catalog" }

//- /app/src/lib.rs
pub fn auto_import_unfinished() {
    let _: AutoImportedT$auto_import_unfinished$
}

pub fn auto_import_edit() {
    let _: AutoImportedT$auto_import_edit$ = todo!();
}
"#,
        &[
            query(
                "unfinished auto-import candidate",
                "auto_import_unfinished",
                "AutoImportedType",
            ),
            query(
                "edited auto-import candidate",
                "auto_import_edit",
                "AutoImportedType",
            ),
        ],
        expect![[r#"
            unfinished auto-import candidate
            - struct AutoImportedType

            edited auto-import candidate
            - struct AutoImportedType
        "#]],
    );
}

/// Returning to an existing call must replace only the name, not duplicate its delimiters.
#[test]
fn keeps_existing_call_and_macro_syntax_when_completing_in_place() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_completion_existing_call_syntax"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! local_macro {
    () => { 1u8 };
}

#[macro_export]
macro_rules! exported_macro {
    () => { 2u8 };
}

pub fn plain_function() {}

pub struct Target;

impl Target {
    pub fn new() -> Self {
        Self
    }

    pub fn method_name(&self) {}
}

local_m$module_macro_edit$!();
crate::exported_m$qualified_module_macro_edit$!();

pub fn run(target: Target) {
    plain_f$function_edit$();
    let _ = local_m$body_macro_edit$!();
    let _ = crate::exported_m$qualified_body_macro_edit$!();
    let _ = Target::ne$associated_edit$();
    target.method_n$method_edit$();
}
"#,
        &[
            verbose_query("module macro edit", "module_macro_edit", "local_macro"),
            verbose_query(
                "qualified module macro edit",
                "qualified_module_macro_edit",
                "exported_macro",
            ),
            verbose_query("function edit", "function_edit", "plain_function"),
            verbose_query("body macro edit", "body_macro_edit", "local_macro"),
            verbose_query(
                "qualified body macro edit",
                "qualified_body_macro_edit",
                "exported_macro",
            ),
            verbose_query("associated function edit", "associated_edit", "new"),
            verbose_query("method edit", "method_edit", "method_name"),
        ],
        expect![[r#"
            module macro edit
            - macro local_macro
              detail: macro local_macro
              sort: local_macro|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(0) } })
              replace: 262..269

            qualified module macro edit
            - macro exported_macro
              detail: macro exported_macro
              sort: exported_macro|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(1) } })
              replace: 281..291

            function edit
            - fn plain_function
              detail: pub fn plain_function()
              sort: 01-module|plain_function|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(0) })
              replace: 330..337

            body macro edit
            - macro local_macro
              detail: macro local_macro
              sort: 01-module|local_macro|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(0) } })
              replace: 353..360

            qualified body macro edit
            - macro exported_macro
              detail: macro exported_macro
              sort: exported_macro|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(1) } })
              replace: 384..394

            associated function edit
            - fn new
              detail: pub fn new() -> Self
              sort: new|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(2) })
              replace: 419..421

            method edit
            - inherent_method method_name
              detail: pub fn method_name(&self)
              sort: method_name|01|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(3) })
              replace: 436..444
        "#]],
    );
}

fn query(title: &'static str, marker: &'static str, label: &'static str) -> AnalysisQuery {
    AnalysisQuery::complete_with_source(title, marker)
        .in_lib("app")
        .matching(label)
}

fn verbose_query(title: &'static str, marker: &'static str, label: &'static str) -> AnalysisQuery {
    AnalysisQuery::complete_verbose_with_source(title, marker).matching(label)
}
