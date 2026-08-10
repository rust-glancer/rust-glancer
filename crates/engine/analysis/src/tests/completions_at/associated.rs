use expect_test::expect;

use super::super::utils::{AnalysisQuery, check_analysis_queries};
#[test]
fn completes_associated_items_for_types_traits_aliases_and_qualified_anchors() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_associated_path_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
use Event::Sta$import_variant$;

pub trait Parent {
    type ParentItem;
    const PARENT: u8;
    fn parent_fn() -> Self::ParentItem;
}

pub trait Factory: Parent {
    type Output;
    const LIMIT: u8;
    fn make() -> Self::Output;
    fn inspect(&self);
}

pub struct Widget<T>(T);

impl<T> Widget<T> {
    pub const ZERO: u8 = 0;
    pub fn new(value: T) -> Self { Self(value) }
    pub fn touch(&self) {}
}

impl<T> Parent for Widget<T> {
    type ParentItem = T;
    const PARENT: u8 = 1;
    fn parent_fn() -> Self::ParentItem { loop {} }
}

impl<T> Factory for Widget<T> {
    type Output = T;
    const LIMIT: u8 = 2;
    fn make() -> Self::Output { loop {} }
    fn inspect(&self) {}
}

pub type WidgetAlias<T> = Widget<T>;

pub enum Event {
    Start,
    Data(u8),
    Stop { code: u8 },
}

pub fn inspect_body<T: Factory>() {
    let _ = Widget::<u8>::ne$concrete$();
    let _ = WidgetAlias::<u8>::ZE$alias$ + 1;
    let _ = T::ma$param_value$();
    let _: T::Out$param_type$ = todo!();
    let _ = <T as Factory>::LI$qualified_value$ + 1;
    let _: <T as Factory>::Par$qualified_type$ = todo!();
    let _ = Factory::ma$direct_trait$();
}

pub fn inspect_signature<T: Factory>(_: T::Out$signature_param$) -> <T as Factory>::Par$signature_fq$ {
    loop {}
}
"#,
        &[
            AnalysisQuery::complete("concrete associated items", "concrete"),
            AnalysisQuery::complete("alias associated items", "alias"),
            AnalysisQuery::complete("parameter value associated items", "param_value"),
            AnalysisQuery::complete("parameter type associated items", "param_type"),
            AnalysisQuery::complete("qualified value associated items", "qualified_value"),
            AnalysisQuery::complete("qualified type associated items", "qualified_type"),
            AnalysisQuery::complete("direct trait associated items", "direct_trait"),
            AnalysisQuery::complete("signature parameter associated items", "signature_param"),
            AnalysisQuery::complete("signature qualified associated items", "signature_fq"),
            AnalysisQuery::complete("enum variant import items", "import_variant"),
        ],
        expect![[r#"
            concrete associated items
            - const LIMIT
            - type_alias Output
            - const PARENT
            - type_alias ParentItem
            - const ZERO
            - fn inspect
            - fn make
            - fn new
            - fn parent_fn
            - fn touch

            alias associated items
            - const LIMIT
            - type_alias Output
            - const PARENT
            - type_alias ParentItem
            - const ZERO
            - fn inspect
            - fn make
            - fn new
            - fn parent_fn
            - fn touch

            parameter value associated items
            - const LIMIT
            - type_alias Output
            - const PARENT
            - type_alias ParentItem
            - fn inspect
            - fn make
            - fn parent_fn

            parameter type associated items
            - type_alias Output
            - type_alias ParentItem

            qualified value associated items
            - const LIMIT
            - type_alias Output
            - const PARENT
            - type_alias ParentItem
            - fn inspect
            - fn make
            - fn parent_fn

            qualified type associated items
            - type_alias Output
            - type_alias ParentItem

            direct trait associated items
            - const LIMIT
            - type_alias Output
            - const PARENT
            - type_alias ParentItem
            - fn inspect
            - fn make
            - fn parent_fn

            signature parameter associated items
            - type_alias Output
            - type_alias ParentItem

            signature qualified associated items
            - type_alias Output
            - type_alias ParentItem

            enum variant import items
            - variant Data
            - variant Start
            - variant Stop
        "#]],
    );
}

#[test]
fn completes_associated_type_binding_names_from_traits_and_supertraits() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_associated_type_binding_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub trait Parent {
    type ParentItem;
    const PARENT_CONST: u8;
}

pub trait Iterator: Parent {
    type Item;
    type Existing;
    fn next(&mut self) -> Option<Self::Item>;
}

pub fn signature<T: Iterator<It$signature$ = u8, Existing = u8>>() {}

pub fn body() {
    trait LocalParent {
        type LocalParentItem;
    }
    trait LocalIterator: LocalParent {
        type LocalItem;
        type Existing;
        const LOCAL_CONST: u8;
    }
    fn consume<T: LocalIterator<Loc$body$ = u8, Existing = u8>>() {}
}
"#,
        &[
            AnalysisQuery::complete_verbose("signature associated binding", "signature"),
            AnalysisQuery::complete_verbose("body associated binding", "body"),
        ],
        expect![[r#"
            signature associated binding
            - type_alias Item
              detail: type Item
              sort: Item|04|00|Declaration(DeclarationRef { kind: "item", item: TypeAlias(TypeAliasRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: TypeAliasId(1) }) })
              replace: 212..214
            - type_alias ParentItem
              detail: type ParentItem
              sort: ParentItem|04|00|Declaration(DeclarationRef { kind: "item", item: TypeAlias(TypeAliasRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: TypeAliasId(0) }) })
              replace: 212..214

            body associated binding
            - type_alias LocalItem
              detail: type LocalItem
              sort: LocalItem|04|00|Declaration(DeclarationRef { kind: "item", item: TypeAlias(TypeAliasRef { origin: Body(BodyRef { crate_ref: CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }, body: BodyId(1) }), id: TypeAliasId(1) }) })
              replace: 474..477
            - type_alias LocalParentItem
              detail: type LocalParentItem
              sort: LocalParentItem|04|00|Declaration(DeclarationRef { kind: "item", item: TypeAlias(TypeAliasRef { origin: Body(BodyRef { crate_ref: CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }, body: BodyId(1) }), id: TypeAliasId(0) }) })
              replace: 474..477
        "#]],
    );
}

#[test]
fn completes_body_local_associated_items_and_impl_self() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_body_local_associated_completions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_local() {
    trait Build {
        type Output;
        const READY: bool;
        fn build() -> Self::Output;
    }

    struct Local;

    impl Local {
        const EMPTY: Self = Self;
        fn create() -> Self { Self }
        fn inspect(&self) {}

        fn inside() {
            let _ = Self::cr$self_prefix$;
        }
    }

    impl Build for Local {
        type Output = Local;
        const READY: bool = true;
        fn build() -> Self::Output { Local }
    }

    let _ = Local::cr$local_prefix$;
    let _: Local::Out$local_type$;
}
"#,
        &[
            AnalysisQuery::complete("body local Self associated items", "self_prefix"),
            AnalysisQuery::complete("body local value associated items", "local_prefix"),
            AnalysisQuery::complete("body local type associated items", "local_type"),
        ],
        expect![[r#"
            body local Self associated items
            - const EMPTY
            - type_alias Output
            - const READY
            - fn build
            - fn create
            - fn inside
            - fn inspect

            body local value associated items
            - const EMPTY
            - type_alias Output
            - const READY
            - fn build
            - fn create
            - fn inside
            - fn inspect

            body local type associated items
            - type_alias Output
        "#]],
    );
}
