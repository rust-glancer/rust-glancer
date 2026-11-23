use expect_test::expect;

use crate::tests::utils::check_analysis;
#[test]
fn completes_qualified_paths_in_control_flow_patterns() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_control_flow_pattern_path_completions
pub mod api {
    pub struct Profile(pub u8);
    pub struct User(pub u8);
    pub fn build() {}
}

pub struct Users;

pub fn use_it(user: api::User, users: Users) {
    if let api::Us$0er(id) = user {} // complete("if let pattern path completions")

    while let api::Us$0er(id) = user {} // complete("while let pattern path completions")

    for api::Us$0er(id) in users {} // complete("for pattern path completions")
}
"#,
        expect![[r#"
            // if let pattern path completions
            st Profile
            st User

            // while let pattern path completions
            st Profile
            st User

            // for pattern path completions
            st Profile
            st User
        "#]],
    );
}

#[test]
fn completes_for_loop_pattern_bindings() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_for_pattern_binding_completions
pub struct Items;

pub fn use_it(items: Items) {
    for item in items {
        it$0; // complete("for pattern binding completions")
    }
}
"#,
        expect![[r#"
            // for pattern binding completions
            vr item
            vr items
            st Items
            fn use_it
        "#]],
    );
}

#[test]
fn completes_pattern_sites_without_expression_only_candidates() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_pattern_completions
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
        Sta$0 => {} // complete_verbose("match pattern")
    }
    match action {
        crate::Bu$0 => {} // complete("qualified pattern root")
    }
    match action {
        Action::St$0 => {} // complete("qualified enum variant")
    }
    match action {
        Action::Da$0(_) => {} // complete("tuple variant constructor")
    }
    match action {
        Action::Sto$0 { code: _ } => {} // complete("record variant constructor")
    }
    if let Sta$0 = action {} // complete("if let pattern")
    while let Sta$0 = action { break; } // complete("while let pattern")
    let Sta$0 = action; // complete("let pattern")
    for Sta$0 in actions {} // complete("for pattern")
    let _closure = |Sta$0: Action| {}; // complete("closure parameter pattern")
    let TupleMar$0(_) = TupleMarker(0); // complete("tuple struct constructor")
    let RecordMar$0 { field: _ } = RecordMarker { field: 0 }; // complete("record struct constructor")
}

pub fn pattern_param(Sta$0: Action) {} // complete("function parameter pattern")
"#,
        expect![[r#"
            // match pattern
            variant Data
              detail: variant Action::Data
              replace: 447..450
              snippet: Action::Data($0)
            variant Start
              detail: variant Action::Start
              replace: 447..450
              insert: Action::Start
            variant Stop
              detail: variant Action::Stop
              replace: 447..450
              snippet: Action::Stop { $0 }
            enum Action
              detail: enum Action
              replace: 447..450
            type_alias Alias
              detail: type Alias
              replace: 447..450
            struct Marker
              detail: struct Marker
              replace: 447..450
            const READY
              detail: const READY
              replace: 447..450
            struct RecordMarker
              detail: struct RecordMarker
              replace: 447..450
              snippet: RecordMarker { $0 }
            struct TupleMarker
              detail: struct TupleMarker
              replace: 447..450
              snippet: TupleMarker($0)
            module nested
              detail: mod nested
              replace: 447..450

            // qualified pattern root
            en Action
            ta Alias
            st Marker
            ct READY
            st RecordMarker
            st TupleMarker
            md nested

            // qualified enum variant
            ev Data
            ev Start
            ev Stop

            ev Data // tuple variant constructor
            ev Stop // record variant constructor

            // if let pattern
            ev Data
            ev Start
            ev Stop
            en Action
            ta Alias
            st Marker
            ct READY
            st RecordMarker
            st TupleMarker
            md nested

            // while let pattern
            ev Data
            ev Start
            ev Stop
            en Action
            ta Alias
            st Marker
            ct READY
            st RecordMarker
            st TupleMarker
            md nested

            // let pattern
            ev Data
            ev Start
            ev Stop
            en Action
            ta Alias
            st Marker
            ct READY
            st RecordMarker
            st TupleMarker
            md nested

            // for pattern
            en Action
            ta Alias
            st Marker
            ct READY
            st RecordMarker
            st TupleMarker
            md nested

            // closure parameter pattern
            ev Data
            ev Start
            ev Stop
            en Action
            ta Alias
            st Marker
            ct READY
            st RecordMarker
            st TupleMarker
            md nested

            st TupleMarker // tuple struct constructor
            st RecordMarker // record struct constructor

            // function parameter pattern
            ev Data
            ev Start
            ev Stop
            en Action
            ta Alias
            st Marker
            ct READY
            st RecordMarker
            st TupleMarker
            md nested
        "#]],
    );
}

#[test]
fn completes_pattern_keywords_from_request_local_source() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_pattern_keyword_completions
pub fn inspect(value: bool) {
    if let r$0 = value {} // complete_verbose("ref pattern keyword")
    if let m$0 = value {} // complete("mut pattern keyword")
    if let t$0 = value {} // complete("true pattern keyword")
    if let f$0 = value {} // complete("false pattern keyword")
}
"#,
        expect![[r#"
            // ref pattern keyword
            keyword ref
              detail: keyword ref
              replace: 41..42

            kw mut // mut pattern keyword
            kw true // true pattern keyword
            kw false // false pattern keyword
        "#]],
    );
}

#[test]
fn completes_only_invocation_macros_in_patterns() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_pattern_macro_completions
#[rustc_builtin_macro]
pub macro DeriveOnly($item:item) {}

macro_rules! pattern_macro {
    () => {};
}

pub enum Event { Only }

pub fn inspect(event: Event) {
    if let pattern_m$0 = event {} // complete("pattern invocation macro")
}
"#,
        expect![[r#"
            // pattern invocation macro
            ev Only
            en Event
            ma pattern_macro
        "#]],
    );
}

#[test]
fn completes_body_local_pattern_constructors_by_shape() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_local_pattern_constructor_completions
pub fn inspect() {
    struct LocalTuple(u8);
    struct LocalRecord { field: u8 }

    let LocalTu$0(_) = LocalTuple(0); // complete("local tuple constructor")
    let LocalRec$0 { field: _ } = LocalRecord { field: 0 }; // complete("local record constructor")
}
"#,
        expect![[r#"
            st LocalTuple // local tuple constructor
            st LocalRecord // local record constructor
        "#]],
    );
}
