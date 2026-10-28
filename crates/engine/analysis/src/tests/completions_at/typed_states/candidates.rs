//! User-typing states for candidate providers that share a semantic completion site.

use expect_test::expect;

use crate::tests::utils::check_analysis;

/// Candidate providers must not accidentally rely on the terminator that follows their site.
///
/// The first half stops before the statement semicolon. The second half keeps syntax after the
/// cursor, which exercises replacement inside an expression or annotation that the user returned
/// to edit.
#[test]
fn completes_each_candidate_domain_across_realistic_line_states() {
    check_analysis(
        r#"
//- /src/lib.rs crate:app
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
    let _ = local_v$0 // complete_with_source("unfinished lexical candidate").in_lib("app").matching("local_value")
}

pub fn generic_unfinished<GenericTy>() {
    let _: Gener$0 // complete_with_source("unfinished generic candidate").in_lib("app").matching("GenericTy")
}

pub fn primitive_unfinished() {
    let _: u1$0 // complete_with_source("unfinished primitive candidate").in_lib("app").matching("u16")
}

pub fn macro_unfinished() {
    let _ = local_m$0 // complete_with_source("unfinished macro candidate").in_lib("app").matching("local_macro")
}

pub fn qualified_macro_unfinished() {
    let _ = crate::exported_m$0 // complete_with_source("unfinished qualified macro candidate").in_lib("app").matching("exported_macro")
}

pub fn module_unfinished() {
    let _: crate::module::ModuleT$0 // complete_with_source("unfinished module candidate").in_lib("app").matching("ModuleType")
}

pub fn associated_unfinished() {
    let _ = MemberTarget::ne$0 // complete_with_source("unfinished associated candidate").in_lib("app").matching("new")
}

pub fn field_unfinished(target: MemberTarget) {
    let _ = target.field_n$0 // complete_with_source("unfinished field candidate").in_lib("app").matching("field_name")
}

pub fn method_unfinished(target: MemberTarget) {
    target.method_n$0 // complete_with_source("unfinished method candidate").in_lib("app").matching("method_name")
}

pub fn lexical_edit(local_value: u8) {
    let _ = local_v$0 + 1; // complete_with_source("edited lexical candidate").in_lib("app").matching("local_value")
}

pub fn generic_edit<GenericTy>() {
    let _: Gener$0 = todo!(); // complete_with_source("edited generic candidate").in_lib("app").matching("GenericTy")
}

pub fn primitive_edit() {
    let _: u1$0 = 0; // complete_with_source("edited primitive candidate").in_lib("app").matching("u16")
}

pub fn macro_edit() {
    let _ = local_m$0!(); // complete_with_source("edited macro candidate").in_lib("app").matching("local_macro")
}

pub fn qualified_macro_edit() {
    let _ = crate::exported_m$0!(); // complete_with_source("edited qualified macro candidate").in_lib("app").matching("exported_macro")
}

pub fn module_edit() {
    let _: crate::module::ModuleT$0 = todo!(); // complete_with_source("edited module candidate").in_lib("app").matching("ModuleType")
}

pub fn associated_edit() {
    let _ = MemberTarget::ne$0(); // complete_with_source("edited associated candidate").in_lib("app").matching("new")
}

pub fn field_edit(target: MemberTarget) {
    let _ = target.field_n$0 + 1; // complete_with_source("edited field candidate").in_lib("app").matching("field_name")
}

pub fn method_edit(target: MemberTarget) {
    target.method_n$0(); // complete_with_source("edited method candidate").in_lib("app").matching("method_name")
}
"#,
        expect![[r#"
            vr local_value // unfinished lexical candidate
            tp GenericTy // unfinished generic candidate
            pt u16 // unfinished primitive candidate
            ma local_macro // unfinished macro candidate
            ma exported_macro // unfinished qualified macro candidate
            st ModuleType // unfinished module candidate
            fn new // unfinished associated candidate
            fd field_name // unfinished field candidate
            im method_name // unfinished method candidate
            vr local_value // edited lexical candidate
            tp GenericTy // edited generic candidate
            pt u16 // edited primitive candidate
            ma local_macro // edited macro candidate
            ma exported_macro // edited qualified macro candidate
            st ModuleType // edited module candidate
            fn new // edited associated candidate
            fd field_name // edited field candidate
            im method_name // edited method candidate
        "#]],
    );
}

#[test]
#[ignore = "TODO(#160)"]
fn completes_auto_import_candidates_across_realistic_line_states() {
    check_analysis(
        r#"
//- /catalog/src/lib.rs crate:catalog root:catalog
pub struct AutoImportedType;

//- /app/src/lib.rs crate:app root:app deps:catalog
pub fn auto_import_unfinished() {
    let _: AutoImportedT$0 // complete_with_source("unfinished auto-import candidate").in_lib("app").matching("AutoImportedType")
}

pub fn auto_import_edit() {
    let _: AutoImportedT$0 = todo!(); // complete_with_source("edited auto-import candidate").in_lib("app").matching("AutoImportedType")
}
"#,
        expect![[r#"
            struct AutoImportedType // unfinished auto-import candidate
            struct AutoImportedType // edited auto-import candidate
        "#]],
    );
}

/// Returning to an existing call must replace only the name, not duplicate its delimiters.
#[test]
fn keeps_existing_call_and_macro_syntax_when_completing_in_place() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_completion_existing_call_syntax
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

local_m$0!(); // complete_verbose_with_source("module macro edit").matching("local_macro")
crate::exported_m$0!(); // complete_verbose_with_source("qualified module macro edit").matching("exported_macro")

pub fn run(target: Target) {
    plain_f$0(); // complete_verbose_with_source("function edit").matching("plain_function")
    let _ = local_m$0!(); // complete_verbose_with_source("body macro edit").matching("local_macro")
    let _ = crate::exported_m$0!(); // complete_verbose_with_source("qualified body macro edit").matching("exported_macro")
    let _ = Target::ne$0(); // complete_verbose_with_source("associated function edit").matching("new")
    target.method_n$0(); // complete_verbose_with_source("method edit").matching("method_name")
}
"#,
        expect![[r#"
            // module macro edit
            macro local_macro
              detail: macro local_macro
              sort: local_macro|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(0) } })
              replace: 262..269

            // qualified module macro edit
            macro exported_macro
              detail: macro exported_macro
              sort: exported_macro|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(1) } })
              replace: 281..291

            // function edit
            fn plain_function
              detail: pub fn plain_function()
              sort: 01-module|plain_function|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(0) })
              replace: 330..337

            // body macro edit
            macro local_macro
              detail: macro local_macro
              sort: 01-module|local_macro|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(0) } })
              replace: 353..360

            // qualified body macro edit
            macro exported_macro
              detail: macro exported_macro
              sort: exported_macro|06|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(1) } })
              replace: 384..394

            // associated function edit
            fn new
              detail: pub fn new() -> Self
              sort: new|06|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(2) })
              replace: 419..421

            // method edit
            inherent_method method_name
              detail: pub fn method_name(&self)
              sort: method_name|01|00|Function(FunctionRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: FunctionId(3) })
              replace: 436..444
        "#]],
    );
}
