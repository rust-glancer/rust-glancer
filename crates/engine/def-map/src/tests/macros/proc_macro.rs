use expect_test::expect;
use rg_workspace::TargetKind;

use crate::{
    LocalDefKind,
    testonly::DefMapFixture,
    tests::utils::{self, PathResolutionQuery},
};

const PROC_MACRO_FIXTURE: &str = r#"
//- /macros/src/lib.rs crate:fixture_macros root:macros kind:proc-macro
extern crate proc_macro;

#[proc_macro]
pub fn emit(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    input
}

#[proc_macro_attribute]
pub fn traced(
    _attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    item
}

#[proc_macro_derive(Stored)]
pub fn stored(_item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    proc_macro::TokenStream::new()
}

pub fn leaked_value() {}
pub struct LeakedType;

mod internal {
    pub fn reexported() {}
}
pub use internal::reexported;

//- /app/src/lib.rs crate:fixture_app root:app deps:fixture_macros
use fixture_macros::{leaked_value, LeakedType, reexported};
"#;

#[test]
fn proc_macro_export_retains_its_implementation_identity() {
    let fixture = DefMapFixture::build(PROC_MACRO_FIXTURE);
    let crate_ref = fixture.crate_ref("fixture_macros", TargetKind::ProcMacro);
    let def_map = fixture
        .resident_def_map(crate_ref)
        .expect("proc-macro def map should exist");
    let root = def_map
        .modules()
        .first()
        .expect("proc-macro root module should exist");

    let implementation = root
        .local_defs
        .iter()
        .copied()
        .find(|local_def| {
            def_map.local_def(*local_def).is_some_and(|data| {
                data.kind == LocalDefKind::Function && data.name.as_str() == "stored"
            })
        })
        .expect("derive implementation function should be collected");
    let export = root
        .local_defs
        .iter()
        .copied()
        .find(|local_def| {
            def_map.local_def(*local_def).is_some_and(|data| {
                data.kind == LocalDefKind::MacroDefinition && data.name.as_str() == "Stored"
            })
        })
        .expect("derive macro export should be collected");

    assert_ne!(
        implementation, export,
        "implementation and export must have distinct identities"
    );
    assert_eq!(
        def_map
            .macro_definition(export)
            .and_then(|data| data.proc_macro_implementation()),
        Some(implementation),
        "the macro export should navigate to its implementation function",
    );
}

#[test]
fn proc_macro_target_exports_only_direct_proc_macros() {
    utils::check_project_path_resolution(
        PROC_MACRO_FIXTURE,
        &[
            PathResolutionQuery::proc_macro("fixture_macros", "crate", "emit").values(),
            PathResolutionQuery::lib("fixture_app", "crate", "fixture_macros::emit").macros(),
            PathResolutionQuery::lib("fixture_app", "crate", "fixture_macros::emit").values(),
            PathResolutionQuery::lib("fixture_app", "crate", "fixture_macros::Stored").macros(),
            PathResolutionQuery::lib("fixture_app", "crate", "fixture_macros::stored"),
            PathResolutionQuery::lib("fixture_app", "crate", "fixture_macros::leaked_value"),
            PathResolutionQuery::lib("fixture_app", "crate", "fixture_macros::LeakedType"),
            PathResolutionQuery::lib("fixture_app", "crate", "fixture_macros::reexported"),
            PathResolutionQuery::lib("fixture_app", "crate", "leaked_value"),
            PathResolutionQuery::lib("fixture_app", "crate", "LeakedType"),
            PathResolutionQuery::lib("fixture_app", "crate", "reexported"),
        ],
        expect![[r#"
            fixture_macros [proc-macro] crate resolves emit [values] -> fn fixture_macros[proc-macro]::crate::emit
            fixture_app [lib] crate resolves fixture_macros::emit [macros] -> macro_definition fixture_macros[proc-macro]::crate::emit
            fixture_app [lib] crate resolves fixture_macros::emit [values] -> <none> (unresolved at segment #1)
            fixture_app [lib] crate resolves fixture_macros::Stored [macros] -> macro_definition fixture_macros[proc-macro]::crate::Stored
            fixture_app [lib] crate resolves fixture_macros::stored -> <none> (unresolved at segment #1)
            fixture_app [lib] crate resolves fixture_macros::leaked_value -> <none> (unresolved at segment #1)
            fixture_app [lib] crate resolves fixture_macros::LeakedType -> <none> (unresolved at segment #1)
            fixture_app [lib] crate resolves fixture_macros::reexported -> <none> (unresolved at segment #1)
            fixture_app [lib] crate resolves leaked_value -> <none> (unresolved at segment #0)
            fixture_app [lib] crate resolves LeakedType -> <none> (unresolved at segment #0)
            fixture_app [lib] crate resolves reexported -> <none> (unresolved at segment #0)
        "#]],
    );
}

#[test]
fn registers_proc_macros_from_written_included_and_generated_functions() {
    let declarations = r#"
#[proc_macro]
pub fn emit(input: proc_macro::TokenStream) -> proc_macro::TokenStream { input }
#[proc_macro_derive(Stored)]
pub fn stored(input: proc_macro::TokenStream) -> proc_macro::TokenStream { input }
"#;
    let sources = [
        declarations.to_owned(),
        format!("include!(\"items.rs\");\n//- /macros/src/items.rs\n{declarations}"),
        format!("macro_rules! definitions {{ () => {{ {declarations} }}; }} definitions!();"),
    ];
    for source in sources {
        let fixture = format!(
            r#"
//- /macros/src/lib.rs crate:fixture_macros root:macros kind:proc-macro
{source}
//- /app/src/lib.rs crate:app root:app deps:fixture_macros
use fixture_macros::{{emit, Stored}};
"#
        );
        utils::check_project_path_resolution_with_fake_sysroot(
            &fixture,
            &[
                PathResolutionQuery::lib("app", "crate", "fixture_macros::emit").macros(),
                PathResolutionQuery::lib("app", "crate", "fixture_macros::Stored").macros(),
                PathResolutionQuery::lib("app", "crate", "emit").macros(),
                PathResolutionQuery::lib("app", "crate", "Stored").macros(),
                PathResolutionQuery::proc_macro("fixture_macros", "crate", "emit").values(),
            ],
            expect![[r#"
                app [lib] crate resolves fixture_macros::emit [macros] -> macro_definition fixture_macros[proc-macro]::crate::emit
                app [lib] crate resolves fixture_macros::Stored [macros] -> macro_definition fixture_macros[proc-macro]::crate::Stored
                app [lib] crate resolves emit [macros] -> macro_definition fixture_macros[proc-macro]::crate::emit
                app [lib] crate resolves Stored [macros] -> macro_definition fixture_macros[proc-macro]::crate::Stored
                fixture_macros [proc-macro] crate resolves emit [values] -> fn fixture_macros[proc-macro]::crate::emit
            "#]],
        );
    }
}
