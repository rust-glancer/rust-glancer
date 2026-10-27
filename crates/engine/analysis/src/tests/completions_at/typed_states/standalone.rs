//! User-typing states for standalone semantic completion sites.

use expect_test::expect;

use crate::tests::utils::check_analysis;

/// Standalone sites are discovered directly from incomplete item syntax rather than Body IR.
///
/// Each family is exercised once while the item is still being typed at file end and once with
/// the surrounding item-list syntax already present. Trait-member stubs cover both a bare recovery
/// prefix and the ordinary `fn req` typing state; the latter replaces the whole incomplete
/// declaration prefix with the generated signature.
#[test]
fn completes_standalone_sites_while_items_are_being_typed() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_completion_standalone_typed_states
pub mod macro_edit;
pub mod macro_forward;
pub mod module_edit;
pub mod module_empty;
pub mod module_forward;
pub mod trait_closed;
pub mod trait_empty;
pub mod trait_eof;
pub mod trait_fn_closed;
pub mod trait_fn_eof;

//- /src/trait_closed.rs
trait Service {
    type Output;
    fn required(&self) -> Self::Output;
}

struct Worker;

impl Service for Worker {
    req$0 // complete_with_source("trait member before closing brace").matching("required")
}

//- /src/trait_eof.rs
trait Service {
    type Output;
    fn required(&self) -> Self::Output;
}

struct Worker;

impl Service for Worker {
    req$0 // complete_with_source("trait member at EOF").matching("required")
//- /src/trait_fn_closed.rs
trait Service {
    fn required(&self);
}

struct Worker;

impl Service for Worker {
    fn req$0 // complete_with_source("function trait member before closing brace").matching("required")
}

//- /src/trait_fn_eof.rs
trait Service {
    fn required(&self);
}

struct Worker;

impl Service for Worker {
    fn req$0 // complete_with_source("function trait member at EOF").matching("required")
//- /src/trait_empty.rs
trait Service {
    type Output;
    fn required(&self) -> Self::Output;
}

struct Worker;

impl Service for Worker {
    $0 // complete_with_source("trait member before its first prefix at EOF").matching("required")
//- /src/macro_edit.rs
macro_rules! local_item {
    () => { struct Generated; };
}

local_i$0!(); // complete_with_source("module macro before invocation suffix").matching("local_item")

//- /src/macro_forward.rs
macro_rules! local_item {
    () => { struct Generated; };
}

local_i$0 // complete_with_source("module macro at EOF").matching("local_item")
//- /src/module_edit.rs
mod pars$0; // complete_with_source("module declaration before semicolon").matching("parser")

//- /src/module_edit/parser.rs
pub struct Parser;

//- /src/module_forward.rs
mod pars$0 // complete_with_source("module declaration at EOF").matching("parser")
//- /src/module_forward/parser.rs
pub struct Parser;

//- /src/module_empty.rs
mod $0 // complete_with_source("module declaration before its first prefix at EOF").matching("parser")
//- /src/module_empty/parser.rs
pub struct Parser;
"#,
        expect![[r#"
            fn required // trait member before closing brace
            fn required // trait member at EOF
            fn required // function trait member before closing brace
            fn required // function trait member at EOF
            fn required // trait member before its first prefix at EOF
            ma local_item // module macro before invocation suffix
            ma local_item // module macro at EOF
            md parser // module declaration before semicolon
            md parser // module declaration at EOF
            md parser // module declaration before its first prefix at EOF
        "#]],
    );
}
