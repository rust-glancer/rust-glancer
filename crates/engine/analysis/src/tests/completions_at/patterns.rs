use expect_test::expect;

use super::super::utils::{AnalysisQuery, check_analysis_queries};
#[test]
fn completes_qualified_paths_in_control_flow_patterns() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_control_flow_pattern_path_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub mod api {
    pub struct Profile(pub u8);
    pub struct User(pub u8);
    pub fn build() {}
}

pub struct Users;

pub fn use_it(user: api::User, users: Users) {
    if let api::Us$if_path$er(id) = user {}

    while let api::Us$while_path$er(id) = user {}

    for api::Us$for_path$er(id) in users {}
}
"#,
        &[
            AnalysisQuery::complete("if let pattern path completions", "if_path"),
            AnalysisQuery::complete("while let pattern path completions", "while_path"),
            AnalysisQuery::complete("for pattern path completions", "for_path"),
        ],
        expect![[r#"
            if let pattern path completions
            - struct Profile
            - struct User

            while let pattern path completions
            - struct Profile
            - struct User

            for pattern path completions
            - struct Profile
            - struct User
        "#]],
    );
}

#[test]
fn completes_for_loop_pattern_bindings() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_for_pattern_binding_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Items;

pub fn use_it(items: Items) {
    for item in items {
        it$0;
    }
}
"#,
        &[AnalysisQuery::complete(
            "for pattern binding completions",
            "0",
        )],
        expect![[r#"
            for pattern binding completions
            - struct Items
            - variable item
            - variable items
            - fn use_it
        "#]],
    );
}

#[test]
fn completes_pattern_sites_without_expression_only_candidates() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_pattern_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub enum Action {
    Start,
    Stop { code: u8 },
    Data(u8),
}

pub struct Marker;
pub struct TupleMarker(u8);
pub struct RecordMarker { field: u8 }
pub type Alias = Marker;
pub const READY: Action = Action::Start;
pub static STATIC_ACTION: Action = Action::Start;
pub trait PatternTrait {}
pub mod nested {}
pub fn build_action() -> Action { Action::Start }

pub fn inspect(action: Action, actions: [Action; 1]) {
    match action {
        Sta$match_pat$ => {}
    }
    match action {
        crate::Bu$qualified_root$ => {}
    }
    match action {
        Action::St$qualified_variant$ => {}
    }
    match action {
        Action::Da$tuple_variant$(_) => {}
    }
    match action {
        Action::Sto$record_variant$ { code: _ } => {}
    }
    if let Sta$if_let$ = action {}
    while let Sta$while_let$ = action { break; }
    let Sta$let_pat$ = action;
    for Sta$for_pat$ in actions {}
    let _closure = |Sta$closure_param$: Action| {};
    let TupleMar$tuple_struct$(_) = TupleMarker(0);
    let RecordMar$record_struct$ { field: _ } = RecordMarker { field: 0 };
}

pub fn pattern_param(Sta$function_param$: Action) {}
"#,
        &[
            AnalysisQuery::complete_verbose("match pattern", "match_pat"),
            AnalysisQuery::complete("qualified pattern root", "qualified_root"),
            AnalysisQuery::complete("qualified enum variant", "qualified_variant"),
            AnalysisQuery::complete("tuple variant constructor", "tuple_variant"),
            AnalysisQuery::complete("record variant constructor", "record_variant"),
            AnalysisQuery::complete("if let pattern", "if_let"),
            AnalysisQuery::complete("while let pattern", "while_let"),
            AnalysisQuery::complete("let pattern", "let_pat"),
            AnalysisQuery::complete("for pattern", "for_pat"),
            AnalysisQuery::complete("closure parameter pattern", "closure_param"),
            AnalysisQuery::complete("tuple struct constructor", "tuple_struct"),
            AnalysisQuery::complete("record struct constructor", "record_struct"),
            AnalysisQuery::complete("function parameter pattern", "function_param"),
        ],
        expect![[r#"
            match pattern
            - variant Data
              detail: variant Action::Data
              sort: 00-expected|Data|04|00|EnumVariant(EnumVariantRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), enum_id: EnumId(0), index: 2 })
              replace: 447..450
              snippet: Action::Data($0)
            - variant Start
              detail: variant Action::Start
              sort: 00-expected|Start|04|00|EnumVariant(EnumVariantRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), enum_id: EnumId(0), index: 0 })
              replace: 447..450
              insert: Action::Start
            - variant Stop
              detail: variant Action::Stop
              sort: 00-expected|Stop|04|00|EnumVariant(EnumVariantRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), enum_id: EnumId(0), index: 1 })
              replace: 447..450
              snippet: Action::Stop { $0 }
            - enum Action
              detail: enum Action
              sort: 01-module|Action|04|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(0) } })
              replace: 447..450
            - type_alias Alias
              detail: type Alias
              sort: 01-module|Alias|04|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(4) } })
              replace: 447..450
            - struct Marker
              detail: struct Marker
              sort: 01-module|Marker|04|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(1) } })
              replace: 447..450
            - const READY
              detail: const READY
              sort: 01-module|READY|05|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(5) } })
              replace: 447..450
            - struct RecordMarker
              detail: struct RecordMarker
              sort: 01-module|RecordMarker|04|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(3) } })
              replace: 447..450
              snippet: RecordMarker { $0 }
            - struct TupleMarker
              detail: struct TupleMarker
              sort: 01-module|TupleMarker|04|00|Declaration(DeclarationRef { kind: "local_def", local_def: LocalDefRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), local_def: LocalDefId(2) } })
              replace: 447..450
              snippet: TupleMarker($0)
            - module nested
              detail: mod nested
              sort: 01-module|nested|03|00|Declaration(DeclarationRef { kind: "module", module: ModuleRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), module: ModuleId(1) } })
              replace: 447..450

            qualified pattern root
            - enum Action
            - type_alias Alias
            - struct Marker
            - const READY
            - struct RecordMarker
            - struct TupleMarker
            - module nested

            qualified enum variant
            - variant Data
            - variant Start
            - variant Stop

            tuple variant constructor
            - variant Data

            record variant constructor
            - variant Stop

            if let pattern
            - enum Action
            - type_alias Alias
            - variant Data
            - struct Marker
            - const READY
            - struct RecordMarker
            - variant Start
            - variant Stop
            - struct TupleMarker
            - module nested

            while let pattern
            - enum Action
            - type_alias Alias
            - variant Data
            - struct Marker
            - const READY
            - struct RecordMarker
            - variant Start
            - variant Stop
            - struct TupleMarker
            - module nested

            let pattern
            - enum Action
            - type_alias Alias
            - variant Data
            - struct Marker
            - const READY
            - struct RecordMarker
            - variant Start
            - variant Stop
            - struct TupleMarker
            - module nested

            for pattern
            - enum Action
            - type_alias Alias
            - struct Marker
            - const READY
            - struct RecordMarker
            - struct TupleMarker
            - module nested

            closure parameter pattern
            - enum Action
            - type_alias Alias
            - variant Data
            - struct Marker
            - const READY
            - struct RecordMarker
            - variant Start
            - variant Stop
            - struct TupleMarker
            - module nested

            tuple struct constructor
            - struct TupleMarker

            record struct constructor
            - struct RecordMarker

            function parameter pattern
            - enum Action
            - type_alias Alias
            - variant Data
            - struct Marker
            - const READY
            - struct RecordMarker
            - variant Start
            - variant Stop
            - struct TupleMarker
            - module nested
        "#]],
    );
}

#[test]
fn completes_pattern_keywords_from_request_local_source() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_pattern_keyword_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn inspect(value: bool) {
    if let r$ref_kw$ = value {}
    if let m$mut_kw$ = value {}
    if let t$true_kw$ = value {}
    if let f$false_kw$ = value {}
}
"#,
        &[
            AnalysisQuery::complete_verbose_with_source("ref pattern keyword", "ref_kw"),
            AnalysisQuery::complete_with_source("mut pattern keyword", "mut_kw"),
            AnalysisQuery::complete_with_source("true pattern keyword", "true_kw"),
            AnalysisQuery::complete_with_source("false pattern keyword", "false_kw"),
        ],
        expect![[r#"
            ref pattern keyword
            - keyword ref
              detail: keyword ref
              sort: ~keyword:00:ref
              replace: 41..42

            mut pattern keyword
            - keyword mut

            true pattern keyword
            - keyword true

            false pattern keyword
            - keyword false
        "#]],
    );
}

#[test]
fn completes_only_invocation_macros_in_patterns() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_pattern_macro_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
#[rustc_builtin_macro]
pub macro DeriveOnly($item:item) {}

macro_rules! pattern_macro {
    () => {};
}

pub enum Event { Only }

pub fn inspect(event: Event) {
    if let pattern_m$0$ = event {}
}
"#,
        &[AnalysisQuery::complete("pattern invocation macro", "0")],
        expect![[r#"
            pattern invocation macro
            - enum Event
            - macro pattern_macro
        "#]],
    );
}

#[test]
fn completes_body_local_pattern_constructors_by_shape() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_local_pattern_constructor_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn inspect() {
    struct LocalTuple(u8);
    struct LocalRecord { field: u8 }

    let LocalTu$tuple$(_) = LocalTuple(0);
    let LocalRec$record$ { field: _ } = LocalRecord { field: 0 };
}
"#,
        &[
            AnalysisQuery::complete("local tuple constructor", "tuple"),
            AnalysisQuery::complete("local record constructor", "record"),
        ],
        expect![[r#"
            local tuple constructor
            - struct LocalTuple

            local record constructor
            - struct LocalRecord
        "#]],
    );
}
