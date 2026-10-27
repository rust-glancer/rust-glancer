use expect_test::expect;

use crate::tests::utils::check_analysis;

#[test]
fn completes_unkeyed_associated_items() {
    check_analysis(
        r#"
//- /runtime/src/lib.rs crate:runtime root:runtime
impl u32 {
    pub const MAX: u32 = 0;
    pub fn from_be_bytes(_bytes: [u8; 4]) -> Self { 0 }
    pub fn count_ones(self) -> u32 { 0 }
}

pub trait PrimitiveFactory<T> {
    const DEFAULT: T;
    fn make() -> T;
}

impl PrimitiveFactory<u16> for u32 {
    const DEFAULT: u16 = 0;
    fn make() -> u16 { 0 }
}

//- /app/src/lib.rs crate:app root:app deps:runtime
use runtime::PrimitiveFactory;

pub fn use_it() {
    let _ = u32::$0; // complete("primitive associated items").in_lib("app")
}
"#,
        expect![[r#"
            // primitive associated items
            ct DEFAULT
            ct MAX
            fn count_ones
            fn from_be_bytes
            fn make
        "#]],
    );
}

#[test]
fn completes_associated_items_for_types_traits_aliases_and_qualified_anchors() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_associated_path_completions
use Event::Sta$0; // complete("enum variant import items")

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
    let _ = Widget::<u8>::ne$0(); // complete("concrete associated items")
    let _ = WidgetAlias::<u8>::ZE$0 + 1; // complete("alias associated items")
    let _ = T::ma$0(); // complete("parameter value associated items")
    let _: T::Out$0 = todo!(); // complete("parameter type associated items")
    let _ = <T as Factory>::LI$0 + 1; // complete("qualified value associated items")
    let _: <T as Factory>::Par$0 = todo!(); // complete("qualified type associated items")
    let _ = Factory::ma$0(); // complete("direct trait associated items")
}

pub fn inspect_signature<T: Factory>(_: T::Out$0) -> <T as Factory>::Par$1 { // complete("signature parameter associated items"), complete("signature qualified associated items")
    loop {}
}
"#,
        expect![[r#"
            // enum variant import items
            ev Data
            ev Start
            ev Stop

            // concrete associated items
            ct LIMIT
            ta Output
            ct PARENT
            ta ParentItem
            ct ZERO
            fn inspect
            fn make
            fn new
            fn parent_fn
            fn touch

            // alias associated items
            ct LIMIT
            ta Output
            ct PARENT
            ta ParentItem
            ct ZERO
            fn inspect
            fn make
            fn new
            fn parent_fn
            fn touch

            // parameter value associated items
            ct LIMIT
            ta Output
            ct PARENT
            ta ParentItem
            fn inspect
            fn make
            fn parent_fn

            // parameter type associated items
            ta Output
            ta ParentItem

            // qualified value associated items
            ct LIMIT
            ta Output
            ct PARENT
            ta ParentItem
            fn inspect
            fn make
            fn parent_fn

            // qualified type associated items
            ta Output
            ta ParentItem

            // direct trait associated items
            ct LIMIT
            ta Output
            ct PARENT
            ta ParentItem
            fn inspect
            fn make
            fn parent_fn

            // signature parameter associated items
            ta Output
            ta ParentItem

            // signature qualified associated items
            ta Output
            ta ParentItem
        "#]],
    );
}

#[test]
fn completes_associated_type_binding_names_from_traits_and_supertraits() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_associated_type_binding_completions
pub trait Parent {
    type ParentItem;
    const PARENT_CONST: u8;
}

pub trait Iterator: Parent {
    type Item;
    type Existing;
    fn next(&mut self) -> Option<Self::Item>;
}

pub fn signature<T: Iterator<It$0 = u8, Existing = u8>>() {} // complete_verbose("signature associated binding")

pub fn body() {
    trait LocalParent {
        type LocalParentItem;
    }
    trait LocalIterator: LocalParent {
        type LocalItem;
        type Existing;
        const LOCAL_CONST: u8;
    }
    fn consume<T: LocalIterator<Loc$0 = u8, Existing = u8>>() {} // complete_verbose("body associated binding")
}
"#,
        expect![[r#"
            // signature associated binding
            type_alias Item
              detail: type Item
              sort: Item|04|00|Declaration(DeclarationRef { kind: "item", item: TypeAlias(TypeAliasRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: TypeAliasId(1) }) })
              replace: 212..214
            type_alias ParentItem
              detail: type ParentItem
              sort: ParentItem|04|00|Declaration(DeclarationRef { kind: "item", item: TypeAlias(TypeAliasRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: TypeAliasId(0) }) })
              replace: 212..214

            // body associated binding
            type_alias LocalItem
              detail: type LocalItem
              sort: LocalItem|04|00|Declaration(DeclarationRef { kind: "item", item: TypeAlias(TypeAliasRef { origin: Body(BodyRef { crate_ref: CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }, body: BodyId(1) }), id: TypeAliasId(1) }) })
              replace: 474..477
            type_alias LocalParentItem
              detail: type LocalParentItem
              sort: LocalParentItem|04|00|Declaration(DeclarationRef { kind: "item", item: TypeAlias(TypeAliasRef { origin: Body(BodyRef { crate_ref: CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }, body: BodyId(1) }), id: TypeAliasId(0) }) })
              replace: 474..477
        "#]],
    );
}

#[test]
fn completes_body_local_associated_items_and_impl_self() {
    check_analysis(
        r#"
//- /src/lib.rs crate:analysis_body_local_associated_completions
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
            let _ = Self::cr$0; // complete("body local Self associated items")
        }
    }

    impl Build for Local {
        type Output = Local;
        const READY: bool = true;
        fn build() -> Self::Output { Local }
    }

    let _ = Local::cr$0; // complete("body local value associated items")
    let _: Local::Out$0; // complete("body local type associated items")
}
"#,
        expect![[r#"
            // body local Self associated items
            ct EMPTY
            ta Output
            ct READY
            fn build
            fn create
            fn inside
            fn inspect

            // body local value associated items
            ct EMPTY
            ta Output
            ct READY
            fn build
            fn create
            fn inside
            fn inspect

            ta Output // body local type associated items
        "#]],
    );
}
