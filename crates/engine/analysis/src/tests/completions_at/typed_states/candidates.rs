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
    let _ = local_v$0 // complete("unfinished lexical candidate").in_lib("app").matching("local_value")
}

pub fn generic_unfinished<GenericTy>() {
    let _: Gener$0 // complete("unfinished generic candidate").in_lib("app").matching("GenericTy")
}

pub fn primitive_unfinished() {
    let _: u1$0 // complete("unfinished primitive candidate").in_lib("app").matching("u16")
}

pub fn macro_unfinished() {
    let _ = local_m$0 // complete("unfinished macro candidate").in_lib("app").matching("local_macro")
}

pub fn qualified_macro_unfinished() {
    let _ = crate::exported_m$0 // complete("unfinished qualified macro candidate").in_lib("app").matching("exported_macro")
}

pub fn module_unfinished() {
    let _: crate::module::ModuleT$0 // complete("unfinished module candidate").in_lib("app").matching("ModuleType")
}

pub fn associated_unfinished() {
    let _ = MemberTarget::ne$0 // complete("unfinished associated candidate").in_lib("app").matching("new")
}

pub fn field_unfinished(target: MemberTarget) {
    let _ = target.field_n$0 // complete("unfinished field candidate").in_lib("app").matching("field_name")
}

pub fn method_unfinished(target: MemberTarget) {
    target.method_n$0 // complete("unfinished method candidate").in_lib("app").matching("method_name")
}

pub fn lexical_edit(local_value: u8) {
    let _ = local_v$0 + 1; // complete("edited lexical candidate").in_lib("app").matching("local_value")
}

pub fn generic_edit<GenericTy>() {
    let _: Gener$0 = todo!(); // complete("edited generic candidate").in_lib("app").matching("GenericTy")
}

pub fn primitive_edit() {
    let _: u1$0 = 0; // complete("edited primitive candidate").in_lib("app").matching("u16")
}

pub fn macro_edit() {
    let _ = local_m$0!(); // complete("edited macro candidate").in_lib("app").matching("local_macro")
}

pub fn qualified_macro_edit() {
    let _ = crate::exported_m$0!(); // complete("edited qualified macro candidate").in_lib("app").matching("exported_macro")
}

pub fn module_edit() {
    let _: crate::module::ModuleT$0 = todo!(); // complete("edited module candidate").in_lib("app").matching("ModuleType")
}

pub fn associated_edit() {
    let _ = MemberTarget::ne$0(); // complete("edited associated candidate").in_lib("app").matching("new")
}

pub fn field_edit(target: MemberTarget) {
    let _ = target.field_n$0 + 1; // complete("edited field candidate").in_lib("app").matching("field_name")
}

pub fn method_edit(target: MemberTarget) {
    target.method_n$0(); // complete("edited method candidate").in_lib("app").matching("method_name")
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
fn completes_auto_import_candidates_across_realistic_line_states() {
    check_analysis(
        r#"
//- /catalog/src/lib.rs crate:catalog root:catalog
pub struct AutoImportedType;

//- /app/src/lib.rs crate:app root:app deps:catalog
pub fn auto_import_unfinished() {
    let _: AutoImportedT$0 // complete("unfinished auto-import candidate").in_lib("app").auto_import().matching("AutoImportedType")
}

pub fn auto_import_edit() {
    let _: AutoImportedT$0 = todo!(); // complete("edited auto-import candidate").in_lib("app").auto_import().matching("AutoImportedType")
}
"#,
        expect![[r#"
            st AutoImportedType // unfinished auto-import candidate
            st AutoImportedType // edited auto-import candidate
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

local_m$0!(); // complete_verbose("module macro edit").matching("local_macro")
crate::exported_m$0!(); // complete_verbose("qualified module macro edit").matching("exported_macro")

pub fn run(target: Target) {
    plain_f$0(); // complete_verbose("function edit").matching("plain_function")
    let _ = local_m$0!(); // complete_verbose("body macro edit").matching("local_macro")
    let _ = crate::exported_m$0!(); // complete_verbose("qualified body macro edit").matching("exported_macro")
    let _ = Target::ne$0(); // complete_verbose("associated function edit").matching("new")
    target.method_n$0(); // complete_verbose("method edit").matching("method_name")
}
"#,
        expect![[r#"
            // module macro edit
            macro local_macro
              detail: macro local_macro
              replace: 262..269

            // qualified module macro edit
            macro exported_macro
              detail: macro exported_macro
              replace: 281..291

            // function edit
            fn plain_function
              detail: pub fn plain_function()
              replace: 330..337

            // body macro edit
            macro local_macro
              detail: macro local_macro
              replace: 353..360

            // qualified body macro edit
            macro exported_macro
              detail: macro exported_macro
              replace: 384..394

            // associated function edit
            fn new
              detail: pub fn new() -> Self
              replace: 419..421

            // method edit
            inherent_method method_name
              detail: pub fn method_name(&self)
              replace: 436..444
        "#]],
    );
}
