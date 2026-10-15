//! Analysis-level inference regression tests.
//!
//! While inference is not an inherent analysis property, it is a dedicated scope that affects
//! analysis a lot. This module encapsulates tests that ensure inference behavior in a wider
//! analysis context.

use expect_test::expect;

use super::utils::{
    AnalysisQuery, check_analysis, check_analysis_queries,
    check_analysis_queries_with_fake_sysroot, check_analysis_with_fake_sysroot,
};

#[test]
fn infers_auto_traits_from_aggregate_fields() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "auto_trait_fields"
version = "0.1.0"
edition = "2024"
//- /src/lib.rs
pub unsafe auto trait Send {}
pub unsafe auto trait Sync {}
pub struct Record<T> { pub value: T }
pub enum Choice<T> { Value(T), Empty }
pub union Bits { byte: u8, word: u32 }
pub struct Node<'a> { next: &'a Node<'a> }
pub struct Blocked;
impl !Send for Blocked {}
pub trait Inspect { fn inspect(&self) -> u16; }
impl<T: Send + Sync> Inspect for T {
    fn inspect(&self) -> u16 { 0 }
}
pub fn check(record: Record<u8>, choice: Choice<bool>, bits: Bits, node: Node<'_>, blocked: Record<Blocked>) {
    let array = [0u8; 3];
    let tuple = (0u8, true);
    let $0result = (record.inspect(), choice.inspect(), bits.inspect(), node.inspect(), array.inspect(), tuple.inspect()); // ty("aggregate methods")
    blocked.inspect()$0; // ty("negative field impl")
}
"#,
        expect![[r#"
            (u16, u16, u16, u16, u16, u16) // aggregate methods
            <unknown> // negative field impl
        "#]],
    );
}

#[test]
fn preserves_auto_traits_on_boxed_trait_objects() {
    check_analysis_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "auto_trait_objects"
version = "0.1.0"
edition = "2024"
//- /src/lib.rs
use std::boxed::Box;
pub trait Stream {
    type Item;
    fn peek(&self) -> Self::Item;
}
pub struct Source<T> { pub value: T }
impl<T> Stream for Source<T> {
    type Item = T;
    fn peek(&self) -> T { loop {} }
}
pub fn make<T>() -> Source<T> { loop {} }
pub fn id<T>(value: T) -> T { value }
pub fn object() -> Box<dyn Stream<Item = u8> + Send + Sync> {
    let source = make();
    id(Box::new(source$0)) // ty("constructor input").in_lib("auto_trait_objects")
}
pub fn item<T>(value: Box<dyn Stream<Item = T> + Sync + Send>) -> T { loop {} }
pub trait Inspect { fn inspect(&self) -> u16; }
impl<T: Send + Sync> Inspect for T {
    fn inspect(&self) -> u16 { 0 }
}
pub fn check() {
    let object = id(object());
    let $0result = (object.peek(), object.inspect(), item(id(object))); // ty("object methods").in_lib("auto_trait_objects")
}
pub fn borrowed(value: &(dyn Stream<Item = u8> + Send + Sync)) {
    value.inspect()$0; // ty("borrowed object bounds").in_lib("auto_trait_objects")
}
pub fn reordered<T>(first: &Box<dyn Stream<Item = T> + Send + Sync>, second: &Box<dyn Sync + Stream<Item = T> + Send>) {
    let value = if true { first } else { second };
    value.peek()$0; // ty("reordered bounds").in_lib("auto_trait_objects")
}
"#,
        expect![[r#"
            nominal struct auto_trait_objects[lib]::crate::Source<u8> // constructor input
            (u8, u16, u8) // object methods
            u16 // borrowed object bounds
            param T // reordered bounds
        "#]],
    );
}

#[test]
fn converts_references_to_trait_objects_without_erasing_the_source() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "object_references"
version = "0.1.0"
edition = "2024"
//- /src/lib.rs
pub trait Stream {
    type Item;
    fn peek(&self) -> Self::Item;
    fn next(&mut self) -> Self::Item;
}
pub struct Source<T> { pub value: T }
impl<T> Stream for Source<T> {
    type Item = T;
    fn peek(&self) -> T { loop {} }
    fn next(&mut self) -> T { loop {} }
}
pub fn make<T>() -> Source<T> { loop {} }

pub fn check() {
    let source = make();
    let object: &dyn Stream<Item = u8> = &source;
    let mut mutable = make();
    let mutable_object: &mut dyn Stream<Item = u16> = &mut mutable;
    let $0result = (object.peek(), mutable_object.next(), source.value, mutable.value); // ty("object results and concrete source items")
}
"#,
        expect![[r#"
            (u8, u16, u8, u16) // object results and concrete source items
        "#]],
    );
}

#[test]
fn converts_smart_pointers_to_trait_objects_without_erasing_generic_arguments() {
    for pointer in ["boxed::Box", "sync::Arc", "rc::Rc"] {
        std::println!("pointer: {pointer}");
        check_analysis_queries_with_fake_sysroot(
            &r#"
//- /Cargo.toml
[package]
name = "object_pointers"
version = "0.1.0"
edition = "2024"
//- /src/lib.rs
use std::POINTER as Pointer;

pub trait Stream {
    type Item;
    fn peek(&self) -> Self::Item;
}
pub struct Source<T> { pub value: T }
impl<T> Stream for Source<T> {
    type Item = T;
    fn peek(&self) -> T { loop {} }
}
pub fn make<T>() -> Source<T> { loop {} }
pub fn id<T>(value: T) -> T { value }

pub fn object() -> Pointer<dyn Stream<Item = u8>> {
    let source = make();
    id(Pointer::new(source$source$))
}
pub fn check() {
    object().peek()$result$;
}
"#
            .replace("POINTER", pointer),
            &[
                AnalysisQuery::ty("constructor keeps concrete input", "source")
                    .in_lib("object_pointers"),
                AnalysisQuery::ty("object method result", "result").in_lib("object_pointers"),
            ],
            expect![[r#"
                nominal struct object_pointers[lib]::crate::Source<u8> // constructor keeps concrete input
                u8 // object method result
            "#]],
        );
    }
}

#[test]
fn infers_references_to_trait_objects() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_trait_object_references"
version = "0.1.0"
edition = "2024"
//- /src/lib.rs
pub trait Opaque {}
pub type Borrowed<'a, T: ?Sized> = &'a T;
pub fn make<T: 'static>() -> &'static T { loop {} }

impl dyn Opaque {
    pub fn inspect(&self) { self$0; } // ty("shared self")
    pub fn inspect_mut(&mut self) { self$0; } // ty("mutable self")
}

pub fn references<'a>(shared: &'a dyn Opaque, mutable: &mut dyn Opaque, nested: &&mut dyn Opaque) {
    shared$0; // ty("shared referent")
    mutable$0; // ty("mutable referent")
    nested$0; // ty("nested referent")
    let copied = shared;
    copied$0; // ty("copied reference")
    let borrowed = &shared;
    borrowed$0; // ty("borrowed reference")
    let annotated: &dyn Opaque = shared;
    annotated$0; // ty("annotated reference")
    let alias: Borrowed<'a, dyn Opaque> = shared;
    alias$0; // ty("alias reference")
    let returned = make();
    returned$0; // ty("finalized generic reference")
}
"#,
        expect![[r#"
            &dyn trait analysis_trait_object_references[lib]::crate::Opaque // shared self
            &mut dyn trait analysis_trait_object_references[lib]::crate::Opaque // mutable self
            &'_ dyn trait analysis_trait_object_references[lib]::crate::Opaque // shared referent
            &mut dyn trait analysis_trait_object_references[lib]::crate::Opaque // mutable referent
            &&mut dyn trait analysis_trait_object_references[lib]::crate::Opaque // nested referent
            &'_ dyn trait analysis_trait_object_references[lib]::crate::Opaque // copied reference
            &&'_ dyn trait analysis_trait_object_references[lib]::crate::Opaque // borrowed reference
            &dyn trait analysis_trait_object_references[lib]::crate::Opaque // annotated reference
            &'_ dyn trait analysis_trait_object_references[lib]::crate::Opaque // alias reference
            &'static <unknown> // finalized generic reference
        "#]],
    );
}

#[test]
fn infers_bindings_from_reference_patterns() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_reference_patterns"
version = "0.1.0"
edition = "2024"
//- /src/lib.rs
pub trait Opaque {}
pub struct Holder { pub value: dyn Opaque }

pub fn peeled(&value: &u8) { value$0; } // ty("peeled parameter")
pub fn borrowed(ref value: u8) { value$0; } // ty("borrowed parameter")
pub fn borrowed_mut(ref mut value: u8) { value$0; } // ty("mutably borrowed parameter")
pub fn peeled_trait_object(&value: &&dyn Opaque) { value$0; } // ty("peeled trait object parameter")
pub fn borrowed_trait_object(ref value: &dyn Opaque) { value$0; } // ty("borrowed trait object parameter")
pub fn tuple((value,): (&dyn Opaque,)) { value$0; } // ty("tuple parameter")
pub fn record(Holder { value }: &Holder) { value$0; } // ty("record parameter")
pub fn record_mut(Holder { value }: &mut Holder) { value$0; } // ty("mutable record parameter")

pub fn fields(holder: &mut Holder) {
    let Holder { value: implicit } = &*holder;
    implicit$0; // ty("implicitly borrowed field")
    let Holder { value: ref explicit } = *holder;
    explicit$0; // ty("explicitly borrowed field")
    let Holder { value: ref mut mutable } = *holder;
    mutable$0; // ty("mutably borrowed field")
}
"#,
        expect![[r#"
            u8 // peeled parameter
            &u8 // borrowed parameter
            &mut u8 // mutably borrowed parameter
            &dyn trait analysis_reference_patterns[lib]::crate::Opaque // peeled trait object parameter
            &&dyn trait analysis_reference_patterns[lib]::crate::Opaque // borrowed trait object parameter
            &dyn trait analysis_reference_patterns[lib]::crate::Opaque // tuple parameter
            &dyn trait analysis_reference_patterns[lib]::crate::Opaque // record parameter
            &mut dyn trait analysis_reference_patterns[lib]::crate::Opaque // mutable record parameter
            &dyn trait analysis_reference_patterns[lib]::crate::Opaque // implicitly borrowed field
            &dyn trait analysis_reference_patterns[lib]::crate::Opaque // explicitly borrowed field
            &mut dyn trait analysis_reference_patterns[lib]::crate::Opaque // mutably borrowed field
        "#]],
    );
}

#[test]
fn selects_trait_methods_for_reference_receivers() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_reference_method"
version = "0.1.0"
edition = "2024"
//- /src/lib.rs
pub trait Opaque {}
pub trait Kind {
    type Output;
    fn kind(self) -> Self::Output;
}
impl<T: ?Sized> Kind for &T {
    type Output = u16;
    fn kind(self) -> u16 { 0 }
}
impl Kind for u8 {
    type Output = u32;
    fn kind(self) -> u32 { 0 }
}
pub fn inspect(value: &dyn Opaque) {
    let result = value.kind();
    result$0; // ty("reference method result")
}
"#,
        expect![[r#"
            u16 // reference method result
        "#]],
    );
}

#[test]
fn infers_types_from_const_expressions() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_const_arithmetic"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub const SIZE: usize = 2;
pub struct Holder;
impl Holder { pub const SIZE: usize = 2; }
pub const fn size() -> usize { 2 }
pub struct Buffer<const N: usize = { 12 / 3 }>;
pub struct Small<const N: u8 = { 2 + 2 }>;

pub fn use_it(annotated: [u16; 2 + 2], argument: Buffer<{ 2 + 2 }>, defaulted: Buffer, small: Small) {
    let rep$0eated = [0u8; 2 + 2]$1; // hover("hover array repeat"), ty("array repeat")
    let precedence = [0u8; (2 + 3) * 4]$0; // ty("parenthesized arithmetic")
    let bases = [0u8; 0x10 + 0o10 + 0b10]$0; // ty("integer bases")
    let shifted = [0u8; 1 << (2 + 2)]$0; // ty("shifted array length")
    let masked = [0u8; (0b1010 | 0b0101) & !(1 << 3) ^ (8 >> 1)]$0; // ty("bitwise array length")
    let declaration = annotated$0; // ty("array type annotation")
    let generic_arg = argument$0; // ty("const generic argument")
    let generic_default = defaulted$0; // ty("const generic default")
    let other_integer = small$0; // ty("u8 const generic default")
    let named = [0u8; SIZE]$0; // ty("named const")
    let named_operand = [0u8; 2 + SIZE]$0; // ty("named operand")
    let associated = [0u8; 2 + Holder::SIZE]$0; // ty("associated const")
    let called = [0u8; 2 + size()]$0; // ty("const function")
    let statements = [0u8; { let n = 2; n + 2 }]$0; // ty("block with statements")
}

pub fn bits(annotated: [u16; (1 << 3) | 1], argument: Buffer<{ (1 << 3) >> 1 }>) {
    annotated$0; // ty("bitwise array type annotation")
    argument$0; // ty("shifted const generic argument")
}
"#,
        expect![[r#"
            // hover array repeat
            range: 9:9-9:17
            block:
              kind: variable
              signature:
                let repeated: [u8; 4]

            [u8; 4] // array repeat
            [u8; 20] // parenthesized arithmetic
            [u8; 26] // integer bases
            [u8; 16] // shifted array length
            [u8; 3] // bitwise array length
            [u16; 4] // array type annotation
            nominal struct analysis_const_arithmetic[lib]::crate::Buffer<4> // const generic argument
            nominal struct analysis_const_arithmetic[lib]::crate::Buffer<4> // const generic default
            nominal struct analysis_const_arithmetic[lib]::crate::Small<_> // u8 const generic default
            [u8; _] // named const
            [u8; _] // named operand
            [u8; _] // associated const
            [u8; _] // const function
            [u8; _] // block with statements
            [u16; 9] // bitwise array type annotation
            nominal struct analysis_const_arithmetic[lib]::crate::Buffer<4> // shifted const generic argument
        "#]],
    );
}

#[test]
fn infers_default_calls_alongside_unresolved_impl_receivers() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_default_with_unresolved_impl"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub trait Default { fn default() -> Self; }
pub struct Vec<T>(T);

impl Default for MissingGeneratedType { fn default() -> Self {} }
impl<T> Default for Vec<T> { fn default() -> Self {} }
type Bytes = Vec<u8>;
impl Default for AnotherMissingType { fn default() -> Self {} }

pub fn use_it() {
    let inferred = Vec::default()$0; // ty("omitted element type")
    let explicit = Vec::<u8>::default()$0; // ty("explicit element type")
    let qualified = <Vec<u8> as Default>::default()$0; // ty("qualified trait call")
    let alias = Bytes::default()$0; // ty("type alias receiver")
}
"#,
        expect![[r#"
            nominal struct analysis_default_with_unresolved_impl[lib]::crate::Vec<<unknown>> // omitted element type
            nominal struct analysis_default_with_unresolved_impl[lib]::crate::Vec<u8> // explicit element type
            nominal struct analysis_default_with_unresolved_impl[lib]::crate::Vec<u8> // qualified trait call
            nominal struct analysis_default_with_unresolved_impl[lib]::crate::Vec<u8> // type alias receiver
        "#]],
    );
}

#[test]
fn infers_inherent_impl_bounds_from_constructor_arguments() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_inherent_constructor_bounds"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub trait Marker {}
impl Marker for User {}
pub struct Wrapper<T> { pub value: T }
impl<T: Marker> Wrapper<T> {
    pub fn new(value: T) -> Self { Self { value } }
}

pub fn use_it() {
    let wrapper = Wrapper::new(User)$0; // ty("constructor result").in_lib("analysis_inherent_constructor_bounds")
    let value = wrapper.value$0; // ty("inferred member").in_lib("analysis_inherent_constructor_bounds")
}
"#,
        expect![[r#"
            nominal struct analysis_inherent_constructor_bounds[lib]::crate::Wrapper<nominal struct analysis_inherent_constructor_bounds[lib]::crate::User> // constructor result
            nominal struct analysis_inherent_constructor_bounds[lib]::crate::User // inferred member
        "#]],
    );
}

#[test]
fn refines_unsuffixed_numeric_literals_from_let_annotations() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_numeric_literal_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it() {
    let default_int = 1$0; // ty("default integer literal")
    let annotated_int: u64 = 1$0; // ty("annotated integer literal")
    let default_float = 1.0$0; // ty("default float literal")
    let annotated_float: f32 = 1.0$0; // ty("annotated float literal")
    let annotated_half: f16 = 1.0$0; // ty("annotated half float literal")
    let annotated_quad: f128 = 1.0$0; // ty("annotated quad float literal")
    let suffixed_half = 1.0f16$0; // ty("suffixed half float literal")
    let suffixed_quad = 1.0f128$0; // ty("suffixed quad float literal")
    let mismatch: bool = 1$0; // ty("mismatched numeric literal")
}
"#,
        expect![[r#"
            i32 // default integer literal
            u64 // annotated integer literal
            f64 // default float literal
            f32 // annotated float literal
            f16 // annotated half float literal
            f128 // annotated quad float literal
            f16 // suffixed half float literal
            f128 // suffixed quad float literal
            i32 // mismatched numeric literal
        "#]],
    );
}

#[test]
fn infers_clones_with_extended_float_impls() {
    check_analysis_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_float_clone"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Manual;
impl Clone for Manual {
    fn clone(&self) -> Self { Manual }
}

#[derive(Clone)]
pub struct Derived;

pub fn use_it(half: f16, quad: f128, manual: Manual, derived: Derived) {
    let half_copy = half.clone()$0; // ty("half float clone").in_lib("analysis_float_clone")
    let quad_copy = quad.clone()$0; // ty("quad float clone").in_lib("analysis_float_clone")
    let manual_copy = manual.clone()$0; // ty("handwritten clone").in_lib("analysis_float_clone")
    let derived_copy = derived.clone()$0; // ty("derived clone").in_lib("analysis_float_clone")
}
"#,
        expect![[r#"
            f16 // half float clone
            f128 // quad float clone
            nominal struct analysis_float_clone[lib]::crate::Manual // handwritten clone
            nominal struct analysis_float_clone[lib]::crate::Derived // derived clone
        "#]],
    );
}

#[test]
fn infers_standard_range_forms_and_shared_endpoints() {
    check_analysis_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_ranges"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it(end: u16) {
    let exclusive = (1$0..end)$1; // ty("start refined by end").in_lib("analysis_ranges"), ty("exclusive range").in_lib("analysis_ranges")
    let from = (2u32..)$0; // ty("range from").in_lib("analysis_ranges")
    let to = (..3)$0; // ty("range to with numeric fallback").in_lib("analysis_ranges")
    let inclusive = ('a'..='z')$0; // ty("inclusive character range").in_lib("analysis_ranges")
    let to_inclusive = (..=4u64)$0; // ty("inclusive range to").in_lib("analysis_ranges")
    let full = (..)$0; // ty("full range").in_lib("analysis_ranges")
    let reversed = (end..5$0)$1; // ty("end refined by start").in_lib("analysis_ranges"), ty("range with typed start").in_lib("analysis_ranges")
}
"#,
        expect![[r#"
            u16 // start refined by end
            nominal struct core[lib]::crate::ops::Range<u16> // exclusive range
            nominal struct core[lib]::crate::ops::RangeFrom<u32> // range from
            nominal struct core[lib]::crate::ops::RangeTo<i32> // range to with numeric fallback
            nominal struct core[lib]::crate::ops::RangeInclusive<char> // inclusive character range
            nominal struct core[lib]::crate::ops::RangeToInclusive<u64> // inclusive range to
            nominal struct core[lib]::crate::ops::RangeFull // full range
            u16 // end refined by start
            nominal struct core[lib]::crate::ops::Range<u16> // range with typed start
        "#]],
    );
}

#[test]
fn range_expectations_and_later_uses_refine_endpoints() {
    check_analysis_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_range_evidence"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
use std::ops::{Range, RangeFrom, RangeInclusive};
pub type Inclusive = RangeInclusive<u8>;
pub fn make<T>() -> T { loop {} }
pub fn take_from(value: RangeFrom<usize>) {}
pub fn take_inclusive(value: RangeInclusive<u64>) {}
pub fn stop() -> ! { loop {} }

pub fn use_it() {
    let expected: Range<u32> = (make()$0..make()$1)$2; // ty("expected start").in_lib("analysis_range_evidence"), ty("expected end").in_lib("analysis_range_evidence"), ty("annotated range").in_lib("analysis_range_evidence")
    let aliased: Inclusive = (make()$0..=1)$1; // ty("endpoint under alias expectation").in_lib("analysis_range_evidence"), ty("alias expectation").in_lib("analysis_range_evidence")

    let endpoint = 1$0; // ty("literal refined through range local").in_lib("analysis_range_evidence")
    let bounds$0 = (endpoint$1..)$2; // ty("refined range binding").in_lib("analysis_range_evidence"), ty("endpoint local").in_lib("analysis_range_evidence"), ty("range before later use").in_lib("analysis_range_evidence")
    take_from(bounds);

    let start = make();
    let end = make();
    let generic = (start..=end)$0; // ty("generic range before later use").in_lib("analysis_range_evidence")
    take_inclusive(generic);
    start$0; // ty("generic start after later use").in_lib("analysis_range_evidence")
    end$0; // ty("generic end after later use").in_lib("analysis_range_evidence")

    let diverging: Range<u8> = (stop()$0..1)$1; // ty("diverging endpoint").in_lib("analysis_range_evidence"), ty("range with diverging endpoint").in_lib("analysis_range_evidence")
}
"#,
        expect![[r#"
            u32 // expected start
            u32 // expected end
            nominal struct core[lib]::crate::ops::Range<u32> // annotated range
            u8 // endpoint under alias expectation
            nominal struct core[lib]::crate::ops::RangeInclusive<u8> // alias expectation
            usize // literal refined through range local
            nominal struct core[lib]::crate::ops::RangeFrom<usize> // refined range binding
            usize // endpoint local
            nominal struct core[lib]::crate::ops::RangeFrom<usize> // range before later use
            nominal struct core[lib]::crate::ops::RangeInclusive<u64> // generic range before later use
            u64 // generic start after later use
            u64 // generic end after later use
            ! // diverging endpoint
            nominal struct core[lib]::crate::ops::Range<u8> // range with diverging endpoint
        "#]],
    );
}

#[test]
fn identifies_range_by_language_item_instead_of_name() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_range_identity"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
#[lang = "Range"]
pub struct Interval<T> { pub start: T, pub end: T }

pub fn use_it() {
    let interval = (1u8..2)$0; // ty("renamed range declaration")
    interval.end$0; // ty("field on renamed range")
}
"#,
        expect![[r#"
            nominal struct analysis_range_identity[lib]::crate::Interval<u8> // renamed range declaration
            u8 // field on renamed range
        "#]],
    );
}

#[test]
fn infers_standard_index_outputs_from_the_index_type() {
    check_analysis_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_index_outputs"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub const LENGTH: usize = 4;
pub fn symbolic_lengths<const N: usize>() {
    let named = [1_u8; LENGTH][0$0]$1; // ty("named array length index").in_lib("analysis_index_outputs"), ty("named array length output").in_lib("analysis_index_outputs")
    let generic = [1_u16; N][0$0]$1; // ty("generic array length index").in_lib("analysis_index_outputs"), ty("generic array length output").in_lib("analysis_index_outputs")
}

pub fn use_it(mut bytes: [u8; 4], slice: &[u16], text: &str) {
    let index = 1$0; // ty("scalar index").in_lib("analysis_index_outputs")
    let element = (&bytes[index])$0; // ty("element reference").in_lib("analysis_index_outputs")
    let array_ranges = (
        &bytes[1..3], &bytes[1..], &bytes[..3],
        &bytes[1..=2], &bytes[..=2], &bytes[..],
    )$0; // ty("all array range forms").in_lib("analysis_index_outputs")
    let slice_ranges = (
        &slice[1..3], &slice[1..], &slice[..3],
        &slice[1..=2], &slice[..=2], &slice[..],
    )$0; // ty("all slice range forms").in_lib("analysis_index_outputs")
    let string_ranges = (
        &text[1..3], &text[1..], &text[..3],
        &text[1..=2], &text[..=2], &text[..],
    )$0; // ty("all string range forms").in_lib("analysis_index_outputs")
    let mutable = (&mut bytes[1..])$0; // ty("mutable slice reference").in_lib("analysis_index_outputs")
    let start = 1$0; // ty("endpoint inferred from indexing").in_lib("analysis_index_outputs")
    let bounds = (start..)$0; // ty("stored range").in_lib("analysis_index_outputs")
    let indirect = (&slice[bounds])$0; // ty("indexing with stored range").in_lib("analysis_index_outputs")
}
"#,
        expect![[r#"
            usize // named array length index
            u8 // named array length output
            usize // generic array length index
            u16 // generic array length output
            usize // scalar index
            &u8 // element reference
            (&[u8], &[u8], &[u8], &[u8], &[u8], &[u8]) // all array range forms
            (&[u16], &[u16], &[u16], &[u16], &[u16], &[u16]) // all slice range forms
            (&str, &str, &str, &str, &str, &str) // all string range forms
            &mut [u8] // mutable slice reference
            usize // endpoint inferred from indexing
            nominal struct core[lib]::crate::ops::RangeFrom<usize> // stored range
            &[u16] // indexing with stored range
        "#]],
    );
}

#[test]
fn selects_index_receivers_with_isolated_live_evidence() {
    check_analysis_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_index_receivers"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
use std::ops::{Deref, Index};
pub struct Key;
pub struct Store<T>(T);
impl<T> Index<Key> for Store<T> {
    type Output = T;
    fn index(&self, _: Key) -> &T { &self.0 }
}
pub struct Wrapper<T>(Store<T>);
impl<T> Deref for Wrapper<T> { type Target = Store<T>; }
pub struct Preferred<T>(Store<T>);
impl<T> Deref for Preferred<T> { type Target = Store<T>; }
impl<T> Index<Key> for Preferred<T> { type Output = u32; }

pub trait Marker {}
impl Marker for u32 {}
pub struct Bytes;
impl Index<u16> for Bytes { type Output = u8; }
pub struct Conditional<T>(T);
impl<T: Marker> Index<T> for Conditional<T> { type Output = u32; }
impl<T> Deref for Conditional<T> { type Target = Bytes; }
pub fn make<T>() -> T { loop {} }

pub fn use_it(store: Store<u8>, wrapper: Wrapper<u8>, preferred: Preferred<u8>, conditional: Conditional<u8>) {
    let direct = store[Key]$0; // ty("custom index type").in_lib("analysis_index_receivers")
    let dereferenced = wrapper[Key]$0; // ty("index through Deref").in_lib("analysis_index_receivers")
    let preferred = preferred[Key]$0; // ty("direct Index before Deref").in_lib("analysis_index_receivers")
    let index = make();
    let fallback = conditional[index]$0; // ty("Deref after rejected Index bound").in_lib("analysis_index_receivers")
    index$0; // ty("index unaffected by rejected receiver").in_lib("analysis_index_receivers")
}
pub fn generic<C: Index<I, Output = T>, I, T>(container: C, index: I) -> T {
    container[index]$0 // ty("generic Index bound").in_lib("analysis_index_receivers")
}
pub fn deref_bound<P: Deref<Target = Store<u8>>>(pointer: P) {
    pointer[Key]$0; // ty("generic Deref bound").in_lib("analysis_index_receivers")
}
"#,
        expect![[r#"
            u8 // custom index type
            u8 // index through Deref
            u32 // direct Index before Deref
            u8 // Deref after rejected Index bound
            u16 // index unaffected by rejected receiver
            param T // generic Index bound
            u8 // generic Deref bound
        "#]],
    );
}

#[test]
fn coerces_index_arguments_without_changing_their_source_types() {
    check_analysis_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_index_coercions"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
use std::ops::{Deref, Index};
pub struct TextStore;
impl Index<&str> for TextStore { type Output = u32; }
pub struct SliceStore<T>(T);
impl<T> Index<&[T]> for SliceStore<T> { type Output = T; }
pub struct MutableStore;
impl Index<&mut [u8]> for MutableStore { type Output = u16; }
pub struct Preferred;
impl Index<&String> for Preferred { type Output = u8; }
impl Index<&str> for Preferred { type Output = u32; }
pub fn make<T>() -> T { loop {} }

pub fn use_it(text: TextStore, mut string: String, preferred: Preferred, mutable: MutableStore) {
    let key = &string;
    let from_string = text[key$0]$1; // ty("original string reference").in_lib("analysis_index_coercions"), ty("string index").in_lib("analysis_index_coercions")
    let from_mutable = text[&mut string]$0; // ty("mutable string reborrow").in_lib("analysis_index_coercions")
    let preferred = preferred[&string]$0; // ty("exact argument before coercion").in_lib("analysis_index_coercions")
    let key = &[1_u8, 2];
    let from_array = SliceStore(1_u8)[key$0]$1; // ty("original array reference").in_lib("analysis_index_coercions"), ty("array index").in_lib("analysis_index_coercions")
    let from_mutable_array = mutable[&mut [1_u8, 2]]$0; // ty("mutable array unsizing").in_lib("analysis_index_coercions")
}
pub fn from_bound<P: Deref<Target = str>>(text: TextStore, pointer: P) {
    let value = text[&pointer]$0; // ty("generic Deref argument").in_lib("analysis_index_coercions")
}
pub fn later_output() {
    let store = make::<SliceStore<_>>()$0; // ty("receiver refined by output").in_lib("analysis_index_coercions")
    let key = (&[make(), make()])$0; // ty("array elements refined by output").in_lib("analysis_index_coercions")
    let value: u8 = store[key]$0; // ty("output expectation").in_lib("analysis_index_coercions")
}
pub fn pending_key(text: TextStore, key: Result<&String, ()>) -> Result<(), ()> {
    let value = text[key?$0]$1; // ty("argument supplied by a pending projection").in_lib("analysis_index_coercions"), ty("indexing after the argument projection").in_lib("analysis_index_coercions")
    Result::Ok(())
}
"#,
        expect![[r#"
            &nominal struct alloc[lib]::crate::string::String // original string reference
            u32 // string index
            u32 // mutable string reborrow
            u8 // exact argument before coercion
            &[u8; 2] // original array reference
            u8 // array index
            u16 // mutable array unsizing
            u32 // generic Deref argument
            nominal struct analysis_index_coercions[lib]::crate::SliceStore<u8> // receiver refined by output
            &[u8; 2] // array elements refined by output
            u8 // output expectation
            &nominal struct alloc[lib]::crate::string::String // argument supplied by a pending projection
            u32 // indexing after the argument projection
        "#]],
    );
}

#[test]
fn indexes_arrays_through_slice_impls_of_the_identified_trait() {
    // Only the slice implements the operator here, so this exercises the unsizing alternative
    // independently of core's blanket array impl. The renamed trait checks language identity.
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_index_unsizing"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
#[lang = "index"]
pub trait Project<I> { type Output; }
impl<T> Project<usize> for [T] { type Output = T; }
pub fn use_it(array: [u8; 3]) {
    let value = array[0$0]$1; // ty("index argument"), ty("array through slice implementation")
}
"#,
        expect![[r#"
            usize // index argument
            u8 // array through slice implementation
        "#]],
    );
}

#[test]
fn refines_indexing_from_later_operand_and_output_evidence() {
    check_analysis_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_index_evidence"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
use std::ops::Index;
pub struct User { pub id: u32 }
impl User { pub fn id(&self) -> u32 { self.id } }
pub trait Marker {}
impl Marker for User {}
impl Marker for u8 {}
pub struct Bag<T>(T);
impl<T: Marker> Index<usize> for Bag<T> {
    type Output = T;
    fn index(&self, _: usize) -> &T { &self.0 }
}
pub fn consume<T: Marker>(value: &T) -> u32 { 0 }
pub struct Store;
impl Index<bool> for Store { type Output = User; }
impl Index<usize> for Store { type Output = u8; }
pub struct Stop;
impl Index<usize> for Stop { type Output = !; }
pub fn make<T>() -> T { loop {} }
pub fn require_index(index: bool) {}
pub fn require_base(base: &Store) {}
pub fn require_stop(base: &Stop) {}

pub fn later_index(store: Store) {
    let index = make();
    let user = &store[index]$0; // ty("output after later index evidence").in_lib("analysis_index_evidence")
    let field = user.id$0; // ty("dependent field").in_lib("analysis_index_evidence")
    let method = user.id()$0; // ty("dependent method").in_lib("analysis_index_evidence")
    let consumed = consume(user)$0; // ty("dependent trait obligation").in_lib("analysis_index_evidence")
    require_index(index);
    index$0; // ty("refined index").in_lib("analysis_index_evidence")
}
pub fn later_base() {
    let base = make()$0; // ty("refined base").in_lib("analysis_index_evidence")
    let user = &base[true]$0; // ty("output after later base evidence").in_lib("analysis_index_evidence")
    require_base(&base);
}
pub fn later_output(user: User) {
    let mut array = [Vec::new()]$0; // ty("array initializer refined by output use").in_lib("analysis_index_evidence")
    array[0]$0.push(user); // ty("indexed collection refined by method argument").in_lib("analysis_index_evidence")
    array$0; // ty("refined array binding").in_lib("analysis_index_evidence")
}
pub fn later_never() {
    let base = make();
    let value$0: u8 = base[0]$1; // ty("coerced binding").in_lib("analysis_index_evidence"), ty("diverging output").in_lib("analysis_index_evidence")
    require_stop(&base);
}
pub fn output_drives_bound() {
    let bag$0 = Bag(make()); // ty("receiver refined through pending bound").in_lib("analysis_index_evidence")
    let value: u8 = bag[0$0]$1; // ty("index refined through pending bound").in_lib("analysis_index_evidence"), ty("output refining pending bound").in_lib("analysis_index_evidence")
    let via_method$0 = Bag(make()); // ty("equivalent method receiver").in_lib("analysis_index_evidence")
    let value: &u8 = via_method.index(0)$0; // ty("equivalent method output").in_lib("analysis_index_evidence")
}
"#,
        expect![[r#"
            nominal struct analysis_index_evidence[lib]::crate::User // output after later index evidence
            u32 // dependent field
            u32 // dependent method
            u32 // dependent trait obligation
            bool // refined index
            nominal struct analysis_index_evidence[lib]::crate::Store // refined base
            nominal struct analysis_index_evidence[lib]::crate::User // output after later base evidence
            [nominal struct alloc[lib]::crate::vec::Vec<nominal struct analysis_index_evidence[lib]::crate::User, nominal struct alloc[lib]::crate::vec::Global>; 1] // array initializer refined by output use
            nominal struct alloc[lib]::crate::vec::Vec<nominal struct analysis_index_evidence[lib]::crate::User, nominal struct alloc[lib]::crate::vec::Global> // indexed collection refined by method argument
            [nominal struct alloc[lib]::crate::vec::Vec<nominal struct analysis_index_evidence[lib]::crate::User, nominal struct alloc[lib]::crate::vec::Global>; 1] // refined array binding
            u8 // coerced binding
            ! // diverging output
            nominal struct analysis_index_evidence[lib]::crate::Bag<u8> // receiver refined through pending bound
            usize // index refined through pending bound
            u8 // output refining pending bound
            nominal struct analysis_index_evidence[lib]::crate::Bag<u8> // equivalent method receiver
            &u8 // equivalent method output
        "#]],
    );
}

#[test]
fn propagates_let_annotation_expected_types_through_tuple_expressions() {
    check_analysis_queries(
        r#"
//- /Cargo.toml
[package]
name = "analysis_tuple_expected_type_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it() {
    let default_pair = (1, 1.0)$type_default_pair$;
    let annotated_pair: (u64, f32) = (1$type_pair_int$, 1.0$type_pair_float$)$type_pair$;
    let nested: (u64, (f32, bool)) = (1$type_nested_int$, (1.0$type_nested_float$, true$type_nested_bool$)$type_nested_inner$)$type_nested$;
    let conflict: (bool, f32) = (1$type_conflict_int$, 1.0$type_conflict_float$)$type_conflict_pair$;
}
"#,
        &[
            AnalysisQuery::ty("default tuple expression", "type_default_pair"),
            AnalysisQuery::ty("annotated tuple integer field", "type_pair_int"),
            AnalysisQuery::ty("annotated tuple float field", "type_pair_float"),
            AnalysisQuery::ty("annotated tuple expression", "type_pair"),
            AnalysisQuery::ty("nested tuple integer field", "type_nested_int"),
            AnalysisQuery::ty("nested tuple float field", "type_nested_float"),
            AnalysisQuery::ty("nested tuple bool field", "type_nested_bool"),
            AnalysisQuery::ty("nested inner tuple expression", "type_nested_inner"),
            AnalysisQuery::ty("nested tuple expression", "type_nested"),
            AnalysisQuery::ty("conflicting tuple integer field", "type_conflict_int"),
            AnalysisQuery::ty("conflicting tuple float field", "type_conflict_float"),
            AnalysisQuery::ty("conflicting tuple expression", "type_conflict_pair"),
        ],
        expect![[r#"
            (i32, f64) // default tuple expression
            u64 // annotated tuple integer field
            f32 // annotated tuple float field
            (u64, f32) // annotated tuple expression
            u64 // nested tuple integer field
            f32 // nested tuple float field
            bool // nested tuple bool field
            (f32, bool) // nested inner tuple expression
            (u64, (f32, bool)) // nested tuple expression
            i32 // conflicting tuple integer field
            f32 // conflicting tuple float field
            (i32, f32) // conflicting tuple expression
        "#]],
    );
}

#[test]
fn propagates_expected_types_through_transparent_and_array_expressions() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_shape_expected_type_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn missing<T>() -> T {}

pub fn use_it() {
    let paren: User = (missing()$0)$1; // ty("paren generic call inner"), ty("paren generic call expression")
    let shared: &u64 = &1$0; // ty("reference integer inner")
    let values: [u64; 2] = [1$0, 2$1]$2; // ty("array left integer"), ty("array right integer"), ty("array expression")
    let repeated: [u64; 2] = [1$0; 2]$1; // ty("repeat array integer"), ty("repeat array expression")
    let users: [User; 1] = [missing()$0]$1; // ty("array generic call"), ty("array generic expression")
    let unit = ()$0; // ty("empty tuple expression")
    let deferred = [missing()$0, 1$1]$2; // ty("array call with later evidence"), ty("array integer with later evidence"), ty("array linked through later branch evidence")
    let selected = if true { deferred } else { [2, 3] };
    let _: [u64; 2] = selected;
}
"#,
        expect![[r#"
            nominal struct analysis_shape_expected_type_inference[lib]::crate::User // paren generic call inner
            nominal struct analysis_shape_expected_type_inference[lib]::crate::User // paren generic call expression
            u64 // reference integer inner
            u64 // array left integer
            u64 // array right integer
            [u64; 2] // array expression
            u64 // repeat array integer
            [u64; 2] // repeat array expression
            nominal struct analysis_shape_expected_type_inference[lib]::crate::User // array generic call
            [nominal struct analysis_shape_expected_type_inference[lib]::crate::User; 1] // array generic expression
            () // empty tuple expression
            u64 // array call with later evidence
            u64 // array integer with later evidence
            [u64; 2] // array linked through later branch evidence
        "#]],
    );
}

#[test]
fn propagates_function_return_expected_types() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_return_expected_type_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn int_tail() -> u64 {
    1$0 // ty("integer tail return")
}

pub fn float_tail() -> f32 {
    1.0$0 // ty("float tail return")
}

pub fn tuple_tail() -> (u64, f32) {
    (1$0, 1.0$1)$2 // ty("tuple return integer field"), ty("tuple return float field"), ty("tuple tail return")
}

pub fn explicit_return(flag: bool) -> u64 {
    if flag {
        return 1$0; // ty("explicit return expression")
    }
    2$0 // ty("final tail return")
}
"#,
        expect![[r#"
            u64 // integer tail return
            f32 // float tail return
            u64 // tuple return integer field
            f32 // tuple return float field
            (u64, f32) // tuple tail return
            u64 // explicit return expression
            u64 // final tail return
        "#]],
    );
}

#[test]
fn propagates_function_call_argument_expected_types() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_call_argument_expected_type_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn takes_u64(value: u64) {}

pub fn takes_f32(value: f32) {}

pub fn takes_pair(value: (u64, f32)) {}

pub fn use_it() {
    takes_u64(1$0); // ty("integer call argument")
    takes_f32(1.0$0); // ty("float call argument")
    takes_pair((1$0, 1.0$1)$2); // ty("tuple call integer field"), ty("tuple call float field"), ty("tuple call argument")
}
"#,
        expect![[r#"
            u64 // integer call argument
            f32 // float call argument
            u64 // tuple call integer field
            f32 // tuple call float field
            (u64, f32) // tuple call argument
        "#]],
    );
}

#[test]
fn propagates_method_call_argument_expected_types() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_method_call_argument_expected_type_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Sink;

impl Sink {
    pub fn put_u64(&self, value: u64) {}

    pub fn put_f32(&self, value: f32) {}

    pub fn put_pair(&self, value: (u64, f32)) {}
}

pub struct GenericSink<T> {
    value: T,
}

impl<T> GenericSink<T> {
    pub fn put(&self, value: T) {}

    pub fn put_pair(&self, value: (T, f32)) {}
}

pub fn use_it(sink: Sink, generic_sink: GenericSink<u64>) {
    sink.put_u64(1$0); // ty("integer method argument")
    sink.put_f32(1.0$0); // ty("float method argument")
    sink.put_pair((1$0, 1.0$1)$2); // ty("tuple method integer field"), ty("tuple method float field"), ty("tuple method argument")

    generic_sink.put(1$0); // ty("generic receiver method argument")
    generic_sink.put_pair((1$0, 1.0$1)$2); // ty("generic receiver tuple method integer field"), ty("generic receiver tuple method float field"), ty("generic receiver tuple method argument")
}
"#,
        expect![[r#"
            u64 // integer method argument
            f32 // float method argument
            u64 // tuple method integer field
            f32 // tuple method float field
            (u64, f32) // tuple method argument
            u64 // generic receiver method argument
            u64 // generic receiver tuple method integer field
            f32 // generic receiver tuple method float field
            (u64, f32) // generic receiver tuple method argument
        "#]],
    );
}

#[test]
fn propagates_expected_types_into_generic_call_results() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_generic_call_result_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn id<T>(value: T) -> T {}

pub fn missing<T>() -> T {}

pub fn takes_user(value: User) {}

pub struct Factory;

impl Factory {
    pub fn make<T>() -> T {}
}

pub struct Builder;

impl Builder {
    pub fn build<T>(&self) -> T {}
}

pub fn use_it(builder: Builder) {
    let annotated: User = id(missing())$0; // ty("annotated generic call result")
    takes_user(id(missing())$0); // ty("function argument generic call result")
    let pair: (User, bool) = (id(missing())$0, true$1)$2; // ty("tuple generic call field"), ty("tuple bool field"), ty("tuple containing generic call")
    let associated: User = Factory::make()$0; // ty("associated function generic call result")
    let method: User = builder.build()$0; // ty("method generic call result")
    let unconstrained = id(missing())$0; // ty("unconstrained generic call result")
}
"#,
        expect![[r#"
            nominal struct analysis_generic_call_result_inference[lib]::crate::User // annotated generic call result
            nominal struct analysis_generic_call_result_inference[lib]::crate::User // function argument generic call result
            nominal struct analysis_generic_call_result_inference[lib]::crate::User // tuple generic call field
            bool // tuple bool field
            (nominal struct analysis_generic_call_result_inference[lib]::crate::User, bool) // tuple containing generic call
            nominal struct analysis_generic_call_result_inference[lib]::crate::User // associated function generic call result
            nominal struct analysis_generic_call_result_inference[lib]::crate::User // method generic call result
            <unknown> // unconstrained generic call result
        "#]],
    );
}

#[test]
fn propagates_expected_types_into_generic_call_result_shapes() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_generic_call_result_shape_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct Error;

pub struct Vec<T> {
    value: T,
}

pub struct Option<T> {
    value: T,
}

pub struct Result<T, E> {
    ok: T,
    err: E,
}

pub fn make_vec<T>() -> Vec<T> {}
pub fn make_option<T>() -> Option<T> {}
pub fn make_result<T, E>() -> Result<T, E> {}

pub struct Factory;

impl Factory {
    pub fn make_vec<T>() -> Vec<T> {}
}

pub struct Builder;

impl Builder {
    pub fn build_vec<T>(&self) -> Vec<T> {}
}

pub fn use_it(builder: Builder) {
    let free: Vec<User> = make_vec()$0; // ty("free function generic return shape")
    let associated: Vec<User> = Factory::make_vec()$0; // ty("associated function generic return shape")
    let method: Vec<User> = builder.build_vec()$0; // ty("method generic return shape")
    let option: Option<User> = make_option()$0; // ty("single-param generic return shape")
    let result: Result<User, Error> = make_result()$0; // ty("multi-param generic return shape")
    let unconstrained = make_vec()$0; // ty("unconstrained generic return shape")
}
"#,
        expect![[r#"
            nominal struct analysis_generic_call_result_shape_inference[lib]::crate::Vec<nominal struct analysis_generic_call_result_shape_inference[lib]::crate::User> // free function generic return shape
            nominal struct analysis_generic_call_result_shape_inference[lib]::crate::Vec<nominal struct analysis_generic_call_result_shape_inference[lib]::crate::User> // associated function generic return shape
            nominal struct analysis_generic_call_result_shape_inference[lib]::crate::Vec<nominal struct analysis_generic_call_result_shape_inference[lib]::crate::User> // method generic return shape
            nominal struct analysis_generic_call_result_shape_inference[lib]::crate::Option<nominal struct analysis_generic_call_result_shape_inference[lib]::crate::User> // single-param generic return shape
            nominal struct analysis_generic_call_result_shape_inference[lib]::crate::Result<nominal struct analysis_generic_call_result_shape_inference[lib]::crate::User, nominal struct analysis_generic_call_result_shape_inference[lib]::crate::Error> // multi-param generic return shape
            nominal struct analysis_generic_call_result_shape_inference[lib]::crate::Vec<<unknown>> // unconstrained generic return shape
        "#]],
    );
}

#[test]
fn preserves_live_receiver_self_when_instantiating_method_return() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_method_self_return_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct Name;

pub struct Iter<T> {
    value: T,
}

pub struct Pair<A, B> {
    left: A,
    right: B,
}

impl<T> Iter<T> {
    pub fn pair<U>(self, value: U) -> Pair<Self, U> {}
}

pub fn make_iter<T>(value: T) -> Iter<T> {}

pub fn use_it(user: User, name: Name) {
    let pair = make_iter(user).pair(name)$0; // ty("method Self return")
}
"#,
        expect![[r#"
            nominal struct analysis_method_self_return_inference[lib]::crate::Pair<nominal struct analysis_method_self_return_inference[lib]::crate::Iter<nominal struct analysis_method_self_return_inference[lib]::crate::User>, nominal struct analysis_method_self_return_inference[lib]::crate::Name> // method Self return
        "#]],
    );
}

#[test]
fn infers_collect_destination_through_trait_obligations() {
    check_analysis_with_fake_sysroot(
        r#"
//- /Cargo.toml
[workspace]
members = ["storage", "app"]
resolver = "3"

//- /storage/Cargo.toml
[package]
name = "storage"
version = "0.1.0"
edition = "2024"

//- /storage/src/lib.rs
pub struct ImportData;

pub struct DefMap;

impl DefMap {
    pub fn imports(&self) -> &[ImportData] {
        missing()
    }
}

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

[dependencies]
storage = { path = "../storage" }

//- /app/src/lib.rs
use storage::DefMap;

pub trait UserCollector<S> {}

pub trait UserIteratorExt: core::iter::Iterator {
    fn collect_users<B>(self) -> B
    where
        Self: Sized,
        B: UserCollector<Self>;
}

pub struct User;
pub struct UserStream;

impl core::iter::Iterator for UserStream {
    type Item = User;
}

impl UserIteratorExt for UserStream {}

impl<S> UserCollector<S> for Vec<User>
where
    S: core::iter::Iterator<Item = User>,
{}

pub fn explicit_destination(def_map: &DefMap) {
    let imports = def_map.imports().iter().collect::<Vec<_>>()$0; // ty("explicit collect destination").in_lib("app")
    imports;
}

pub fn expected_destination(def_map: &DefMap) {
    let imports: Vec<_> = def_map.imports().iter().collect()$0; // ty("expected collect destination").in_lib("app")
    imports;
}

pub fn associated_equality_destination(stream: UserStream) {
    let users = stream.collect_users::<Vec<_>>()$0; // ty("associated equality collect destination").in_lib("app")
    users;
}
"#,
        expect![[r#"
            nominal struct alloc[lib]::crate::vec::Vec<&nominal struct storage[lib]::crate::ImportData, nominal struct alloc[lib]::crate::vec::Global> // explicit collect destination
            nominal struct alloc[lib]::crate::vec::Vec<&nominal struct storage[lib]::crate::ImportData, nominal struct alloc[lib]::crate::vec::Global> // expected collect destination
            nominal struct alloc[lib]::crate::vec::Vec<nominal struct app[lib]::crate::User, nominal struct alloc[lib]::crate::vec::Global> // associated equality collect destination
        "#]],
    );
}

#[test]
fn projects_custom_iterator_collect_targets_and_ignores_same_name_methods() {
    check_analysis_with_fake_sysroot(
        r#"
//- /Cargo.toml
[workspace]
members = ["app"]
resolver = "3"

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /app/src/lib.rs
pub struct User;
pub struct Key;
pub struct Value;

pub struct HashMap<K, V> {
    key: K,
    value: V,
}

impl<K, V> core::iter::FromIterator<(K, V)> for HashMap<K, V> {}

impl User {
    pub fn key(&self) -> Key {}
    pub fn value(&self) -> Value {}
}

pub struct SameNameMap;

impl SameNameMap {
    pub fn map<F>(&self, f: F) {}
}

pub struct PairIter;

impl core::iter::Iterator for PairIter {
    type Item = (Key, Value);
}

pub struct OverlappingMap<K, V> {
    key: K,
    value: V,
}

// Deliberately invalid overlap: both candidates still provide the same definite type guidance.
impl<K, V> core::iter::FromIterator<(K, V)> for OverlappingMap<K, V> {}
impl core::iter::FromIterator<(Key, Value)> for OverlappingMap<Key, Value> {}

pub fn pairs() -> PairIter {
    missing()
}

pub fn missing<T>() -> T {}

pub fn use_it(users: &[User], same_name: SameNameMap) {
    let direct_lookup = pairs().collect::<HashMap<_, _>>()$0; // ty("direct HashMap collect result").in_lib("app")
    let lookup = users.iter().map(|user| (user.key(), user.value())).collect::<HashMap<_, _>>()$0; // ty("mapped HashMap collect result").in_lib("app")
    let overlapping = pairs().collect::<OverlappingMap<_, _>>()$0; // ty("overlapping impl shared guidance").in_lib("app")
    same_name.map(|value| value$0); // ty("same-name map closure param").in_lib("app")
}
"#,
        expect![[r#"
            nominal struct app[lib]::crate::HashMap<nominal struct app[lib]::crate::Key, nominal struct app[lib]::crate::Value> // direct HashMap collect result
            nominal struct app[lib]::crate::HashMap<nominal struct app[lib]::crate::Key, nominal struct app[lib]::crate::Value> // mapped HashMap collect result
            nominal struct app[lib]::crate::OverlappingMap<nominal struct app[lib]::crate::Key, nominal struct app[lib]::crate::Value> // overlapping impl shared guidance
            <unknown> // same-name map closure param
        "#]],
    );
}

#[test]
fn projects_realistic_sysroot_iterator_adapters_into_collect_destination() {
    let ty = |title, marker| {
        AnalysisQuery::ty(title, marker).in_lib("analysis_realistic_iterator_adapters")
    };

    check_analysis_queries_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_realistic_iterator_adapters"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct Name;
pub struct Email;
pub struct Row;
pub struct CompletionItem;
pub struct PackageSlot;

pub struct KeywordCandidate {
    pub label: &'static str,
}

pub struct ChangedFile {
    pub package: PackageSlot,
}

impl User {
    pub fn name(&self) -> Name {}
    pub fn email(&self) -> Option<Email> {}
    pub fn is_active(&self) -> bool {}
}

impl KeywordCandidate {
    pub fn completion_item(&self) -> CompletionItem {}
}

pub fn bar(_: usize, _: &User) -> Row {}
pub fn render(user: &User) -> Name {}
pub fn keyword_candidates() -> &'static [KeywordCandidate] {
    missing()
}
pub fn missing<T>() -> T {}

pub fn use_it(users: &[User], changed_files: &[ChangedFile], bytes: &[u8], prefix: &str) {
    let names = users.iter().map(|user| user$type_map_param$.name()$type_map_call$).collect::<Vec<_>>()$type_names$;
    let skipped = users.iter().skip(1).collect::<Vec<_>>()$type_skipped$;
    let rendered = users.iter().map(render).collect::<Vec<_>>()$type_rendered$;
    let render_fn = render;
    let rendered_from_binding = users.iter().map(render_fn).collect::<Vec<_>>()$type_rendered_from_binding$;
    let owned_users: Vec<User> = missing();
    let rendered_from_vec = owned_users.iter().map(render).collect::<Vec<_>>()$type_rendered_from_vec$;
    let byte_refs = bytes.iter().map(|byte| byte$type_identity_map_param$).collect::<Vec<_>>()$type_identity_map$;
    let packages = changed_files.iter().map(|changed_file| changed_file$type_package_map_param$.package$type_package_field$).collect::<Vec<_>>()$type_packages$;
    let emails = users.iter().filter_map(|user| user$type_filter_map_param$.email()).collect::<Vec<_>>()$type_emails$;
    let rows = users.iter().enumerate()$type_enumerate_call$.map(|(id, user)| bar(id$type_bar_id$, user$type_bar_user$)).collect::<Vec<_>>()$type_rows$;
    let filtered_names = users.iter().filter(|user| user$type_filter_param$.is_active())$type_filter_call$.map(|user| user$type_filtered_map_param$.name()$type_filtered_map_call$).collect::<Vec<_>>()$type_filtered_names$;
    let completions = keyword_candidates().iter().filter(|candidate| candidate$type_keyword_filter_param$.label$type_keyword_label$.starts_with(prefix))$type_keyword_filter_call$.map(|candidate| candidate$type_keyword_map_param$.completion_item()$type_keyword_map_call$).collect::<Vec<_>>()$type_completions$;
}
"#,
        &[
            ty("realistic iterator map closure param", "type_map_param"),
            ty("realistic iterator map closure call", "type_map_call"),
            ty("realistic iterator map collect result", "type_names"),
            ty("realistic iterator skip collect result", "type_skipped"),
            ty(
                "realistic iterator named function collect result",
                "type_rendered",
            ),
            ty(
                "realistic iterator function binding collect result",
                "type_rendered_from_binding",
            ),
            ty(
                "realistic iterator named function vec collect result",
                "type_rendered_from_vec",
            ),
            ty(
                "realistic iterator identity map closure param",
                "type_identity_map_param",
            ),
            ty(
                "realistic iterator identity map collect result",
                "type_identity_map",
            ),
            ty(
                "realistic iterator field-copy map closure param",
                "type_package_map_param",
            ),
            ty(
                "realistic iterator field-copy map field",
                "type_package_field",
            ),
            ty(
                "realistic iterator field-copy collect result",
                "type_packages",
            ),
            ty(
                "realistic iterator filter_map closure param",
                "type_filter_map_param",
            ),
            ty(
                "realistic iterator filter_map collect result",
                "type_emails",
            ),
            ty(
                "realistic iterator enumerate call result",
                "type_enumerate_call",
            ),
            ty("realistic iterator enumerate map id", "type_bar_id"),
            ty("realistic iterator enumerate map user", "type_bar_user"),
            ty(
                "realistic iterator enumerate map collect result",
                "type_rows",
            ),
            ty(
                "realistic iterator filter closure param",
                "type_filter_param",
            ),
            ty("realistic iterator filter call result", "type_filter_call"),
            ty(
                "realistic iterator filtered map closure param",
                "type_filtered_map_param",
            ),
            ty(
                "realistic iterator filtered map closure call",
                "type_filtered_map_call",
            ),
            ty(
                "realistic iterator filtered map collect result",
                "type_filtered_names",
            ),
            ty("keyword filter closure param", "type_keyword_filter_param"),
            ty("keyword filter label", "type_keyword_label"),
            ty("keyword filter call result", "type_keyword_filter_call"),
            ty("keyword map closure param", "type_keyword_map_param"),
            ty("keyword map closure call", "type_keyword_map_call"),
            ty("keyword collect result", "type_completions"),
        ],
        expect![[r#"
            &nominal struct analysis_realistic_iterator_adapters[lib]::crate::User // realistic iterator map closure param
            nominal struct analysis_realistic_iterator_adapters[lib]::crate::Name // realistic iterator map closure call
            nominal struct alloc[lib]::crate::vec::Vec<nominal struct analysis_realistic_iterator_adapters[lib]::crate::Name, nominal struct alloc[lib]::crate::vec::Global> // realistic iterator map collect result
            nominal struct alloc[lib]::crate::vec::Vec<&nominal struct analysis_realistic_iterator_adapters[lib]::crate::User, nominal struct alloc[lib]::crate::vec::Global> // realistic iterator skip collect result
            nominal struct alloc[lib]::crate::vec::Vec<nominal struct analysis_realistic_iterator_adapters[lib]::crate::Name, nominal struct alloc[lib]::crate::vec::Global> // realistic iterator named function collect result
            nominal struct alloc[lib]::crate::vec::Vec<nominal struct analysis_realistic_iterator_adapters[lib]::crate::Name, nominal struct alloc[lib]::crate::vec::Global> // realistic iterator function binding collect result
            nominal struct alloc[lib]::crate::vec::Vec<nominal struct analysis_realistic_iterator_adapters[lib]::crate::Name, nominal struct alloc[lib]::crate::vec::Global> // realistic iterator named function vec collect result
            &u8 // realistic iterator identity map closure param
            nominal struct alloc[lib]::crate::vec::Vec<&u8, nominal struct alloc[lib]::crate::vec::Global> // realistic iterator identity map collect result
            &nominal struct analysis_realistic_iterator_adapters[lib]::crate::ChangedFile // realistic iterator field-copy map closure param
            nominal struct analysis_realistic_iterator_adapters[lib]::crate::PackageSlot // realistic iterator field-copy map field
            nominal struct alloc[lib]::crate::vec::Vec<nominal struct analysis_realistic_iterator_adapters[lib]::crate::PackageSlot, nominal struct alloc[lib]::crate::vec::Global> // realistic iterator field-copy collect result
            &nominal struct analysis_realistic_iterator_adapters[lib]::crate::User // realistic iterator filter_map closure param
            nominal struct alloc[lib]::crate::vec::Vec<nominal struct analysis_realistic_iterator_adapters[lib]::crate::Email, nominal struct alloc[lib]::crate::vec::Global> // realistic iterator filter_map collect result
            nominal struct core[lib]::crate::iter::adapters::Enumerate<nominal struct core[lib]::crate::slice::Iter<'_, nominal struct analysis_realistic_iterator_adapters[lib]::crate::User>> // realistic iterator enumerate call result
            usize // realistic iterator enumerate map id
            &nominal struct analysis_realistic_iterator_adapters[lib]::crate::User // realistic iterator enumerate map user
            nominal struct alloc[lib]::crate::vec::Vec<nominal struct analysis_realistic_iterator_adapters[lib]::crate::Row, nominal struct alloc[lib]::crate::vec::Global> // realistic iterator enumerate map collect result
            &&nominal struct analysis_realistic_iterator_adapters[lib]::crate::User // realistic iterator filter closure param
            nominal struct core[lib]::crate::iter::adapters::Filter<nominal struct core[lib]::crate::slice::Iter<'_, nominal struct analysis_realistic_iterator_adapters[lib]::crate::User>, closure #64> // realistic iterator filter call result
            &nominal struct analysis_realistic_iterator_adapters[lib]::crate::User // realistic iterator filtered map closure param
            nominal struct analysis_realistic_iterator_adapters[lib]::crate::Name // realistic iterator filtered map closure call
            nominal struct alloc[lib]::crate::vec::Vec<nominal struct analysis_realistic_iterator_adapters[lib]::crate::Name, nominal struct alloc[lib]::crate::vec::Global> // realistic iterator filtered map collect result
            &&nominal struct analysis_realistic_iterator_adapters[lib]::crate::KeywordCandidate // keyword filter closure param
            &'static str // keyword filter label
            nominal struct core[lib]::crate::iter::adapters::Filter<nominal struct core[lib]::crate::slice::Iter<'_, nominal struct analysis_realistic_iterator_adapters[lib]::crate::KeywordCandidate>, closure #78> // keyword filter call result
            &nominal struct analysis_realistic_iterator_adapters[lib]::crate::KeywordCandidate // keyword map closure param
            nominal struct analysis_realistic_iterator_adapters[lib]::crate::CompletionItem // keyword map closure call
            nominal struct alloc[lib]::crate::vec::Vec<nominal struct analysis_realistic_iterator_adapters[lib]::crate::CompletionItem, nominal struct alloc[lib]::crate::vec::Global> // keyword collect result
        "#]],
    );
}

#[test]
fn explicit_closure_returns_use_their_own_output_types() {
    check_analysis_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "closure_returns"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn output<F: FnOnce() -> R, R>(callback: F) -> R { loop {} }
pub fn make<T>() -> T { loop {} }

pub fn outer(flag: bool) -> u64 {
    let number = output(|| return 42)$0; // ty("inferred closure output").in_lib("closure_returns")
    let unit = output(|| return)$0; // ty("bare closure return").in_lib("closure_returns")
    let mixed = output(|| {
        if flag { return 1$0; } // ty("explicit return follows the tail type").in_lib("closure_returns")
        2u16
    })$0; // ty("mixed closure output").in_lib("closure_returns")
    let nested = output(|| {
        let inner = output(|| return true)$0; // ty("nested closure output").in_lib("closure_returns")
        return 3u8;
    })$0; // ty("enclosing closure output").in_lib("closure_returns")
    let annotated = output(|| -> f32 {
        let value = 1.0;
        return value$0; // ty("annotated closure return").in_lib("closure_returns")
    });
    let contextual: u16 = output(|| return make()$0); // ty("callable bound constrains returned call").in_lib("closure_returns")
    let restored = 4;
    return restored$0; // ty("enclosing function return").in_lib("closure_returns")
}
"#,
        expect![[r#"
            i32 // inferred closure output
            () // bare closure return
            u16 // explicit return follows the tail type
            u16 // mixed closure output
            bool // nested closure output
            u8 // enclosing closure output
            f32 // annotated closure return
            u16 // callable bound constrains returned call
            u64 // enclosing function return
        "#]],
    );
}

#[test]
fn infers_closure_params_and_returns_from_callable_bounds() {
    check_analysis_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_callable_closure_bound_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Attr;

pub struct Id;

pub struct Factory;

pub struct User;
pub struct Name;

impl Factory {
    type Id = Id;

    pub fn with_id(f: impl FnOnce(Self::Id)) {}
}

impl User {
    pub fn name(&self) -> Name {}
}

pub fn with_attrs(f: impl FnOnce(&mut Vec<Attr>)) {}
pub fn with_user(f: impl FnOnce() -> User) {}

pub fn visit<T, F: FnOnce(T)>(value: T, f: F) {}

pub fn visit_all<T, F>(values: &[T], f: F)
where
    F: FnMut(&T),
{}

pub fn apply<T, R, F: FnOnce(T) -> R>(value: T, f: F) -> R {}

pub fn try_apply<T, R, F: FnOnce(T) -> Option<R>>(value: T, f: F) -> Option<R> {}

pub fn id<T>(value: T) -> T {}
pub fn missing<T>() -> T {}
pub fn make_user(id: Id) -> User {}
pub fn make_name(id: Id) -> Name {}

pub fn use_it(flag: bool, attr: Attr, user: User, users: &[User], seed: Id) {
    with_attrs(|attrs| attrs$0.push(attr)$1); // ty("direct callable closure param").in_lib("analysis_callable_closure_bound_inference"), ty("direct callable closure method call").in_lib("analysis_callable_closure_bound_inference")
    Factory::with_id(|id| id$0); // ty("direct closure inherent associated param").in_lib("analysis_callable_closure_bound_inference")
    with_user(|| id(missing())$0); // ty("direct callable closure return body").in_lib("analysis_callable_closure_bound_inference")

    visit(user, |user| {
        user$0.name()$1; // ty("inline generic closure param").in_lib("analysis_callable_closure_bound_inference"), ty("inline generic closure method call").in_lib("analysis_callable_closure_bound_inference")
    });
    visit_all(users, |user| {
        user$0.name()$1; // ty("where generic closure param").in_lib("analysis_callable_closure_bound_inference"), ty("where generic closure method call").in_lib("analysis_callable_closure_bound_inference")
    });

    let applied = apply(seed, |id| make_user(id))$0; // ty("generic closure return result").in_lib("analysis_callable_closure_bound_inference")
    let stored_f = |id| make_user(id);
    let stored = apply(seed, stored_f)$0; // ty("stored generic closure return result").in_lib("analysis_callable_closure_bound_inference")
    let maybe = try_apply(seed, |id| Option::Some(make_user(id)))$0; // ty("nested generic closure return result").in_lib("analysis_callable_closure_bound_inference")
    let conflict = apply(seed, |id| if flag {
        make_user(id)
    } else {
        make_name(id)
    })$0; // ty("conflicting generic closure return result").in_lib("analysis_callable_closure_bound_inference")
}
"#,
        expect![[r#"
            &mut nominal struct alloc[lib]::crate::vec::Vec<nominal struct analysis_callable_closure_bound_inference[lib]::crate::Attr, nominal struct alloc[lib]::crate::vec::Global> // direct callable closure param
            () // direct callable closure method call
            nominal struct analysis_callable_closure_bound_inference[lib]::crate::Id // direct closure inherent associated param
            nominal struct analysis_callable_closure_bound_inference[lib]::crate::User // direct callable closure return body
            nominal struct analysis_callable_closure_bound_inference[lib]::crate::User // inline generic closure param
            nominal struct analysis_callable_closure_bound_inference[lib]::crate::Name // inline generic closure method call
            &nominal struct analysis_callable_closure_bound_inference[lib]::crate::User // where generic closure param
            nominal struct analysis_callable_closure_bound_inference[lib]::crate::Name // where generic closure method call
            nominal struct analysis_callable_closure_bound_inference[lib]::crate::User // generic closure return result
            nominal struct analysis_callable_closure_bound_inference[lib]::crate::User // stored generic closure return result
            nominal enum core[lib]::crate::option::Option<nominal struct analysis_callable_closure_bound_inference[lib]::crate::User> // nested generic closure return result
            nominal struct analysis_callable_closure_bound_inference[lib]::crate::User // conflicting generic closure return result
        "#]],
    );
}

#[test]
fn infers_iterator_closure_params_from_selected_trait_method_bounds() {
    check_analysis_with_fake_sysroot(
        r#"
//- /Cargo.toml
[workspace]
members = ["app"]
resolver = "3"

//- /app/Cargo.toml
[package]
name = "app"
version = "0.1.0"
edition = "2024"

//- /app/src/lib.rs
pub struct User;
pub struct Name;
pub struct Searcher;

pub trait ProduceExt: core::iter::Iterator {
    fn produce<F>(self, f: F)
    where
        Self: Sized,
        F: FnOnce() -> Self::Item;
}

impl<'a, T> ProduceExt for core::slice::Iter<'a, T> {}

impl User {
    pub fn name(&self) -> Name {}

    pub fn is_active(&self) -> bool {}
}

impl Searcher {
    pub fn find<F>(&self, f: F) {}
}

pub fn id<T>(value: T) -> T {}
pub fn missing<T>() -> T {}

pub fn use_it(users: &[User], searcher: Searcher) {
    let mapped = users.iter().map(|user| user$0.name()$1); // ty("iterator map closure param").in_lib("app"), ty("iterator map closure method call").in_lib("app")
    let filtered = users.iter().filter(|user| user$0.is_active()$1); // ty("iterator filter closure param").in_lib("app"), ty("iterator filter closure method call").in_lib("app")
    users.iter().find(|user| user$0.is_active()$1); // ty("iterator find closure param").in_lib("app"), ty("iterator find closure method call").in_lib("app")
    users.iter().produce(|| id(missing())$0); // ty("iterator callable return body").in_lib("app")
    searcher.find(|value| value$0); // ty("same-name find closure param").in_lib("app")
}
"#,
        expect![[r#"
            &nominal struct app[lib]::crate::User // iterator map closure param
            nominal struct app[lib]::crate::Name // iterator map closure method call
            &&nominal struct app[lib]::crate::User // iterator filter closure param
            bool // iterator filter closure method call
            &&nominal struct app[lib]::crate::User // iterator find closure param
            bool // iterator find closure method call
            &nominal struct app[lib]::crate::User // iterator callable return body
            <unknown> // same-name find closure param
        "#]],
    );
}

#[test]
fn projects_associated_type_from_callable_impl_where_clause() {
    check_analysis_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_callable_impl_where_projection"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub trait Produces {
    type Output;

    fn produce(self) -> Self::Output;
}

pub struct Adapter<F> {
    f: F,
}

pub struct Name;
pub struct NotCallable;

impl<F, R> Produces for Adapter<F>
where
    F: FnOnce() -> R,
{
    type Output = R;
}

pub fn adapter<F>(f: F) -> Adapter<F> {}
pub fn make_name() -> Name {}

pub fn use_it(not_callable: NotCallable) {
    let produced = adapter(|| make_name()).produce()$0; // ty("callable impl where projection").in_lib("analysis_callable_impl_where_projection")
    let unknown = adapter(not_callable).produce()$0; // ty("non-callable impl where projection").in_lib("analysis_callable_impl_where_projection")
}
"#,
        expect![[r#"
            nominal struct analysis_callable_impl_where_projection[lib]::crate::Name // callable impl where projection
            <unknown> // non-callable impl where projection
        "#]],
    );
}

#[test]
fn projects_associated_type_from_callable_impl_where_clause_with_generic_assoc_input() {
    check_analysis_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_callable_impl_where_assoc_input_projection"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub trait Stream {
    type Item;

    fn map<B, F>(self, f: F) -> Map<Self, F>
    where
        F: FnMut(Self::Item) -> B;

    fn map_ref<B, F>(self, f: F) -> RefMap<Self, F>
    where
        F: FnMut(&Self::Item) -> B;

    fn map_marked<B, F>(self, f: F) -> MarkedMap<Self, F>
    where
        F: FnMut(Self::Item) -> B;

    fn get(self) -> Self::Item;
}

pub trait Marker {}

pub struct Source<T> {
    value: T,
}

pub struct Map<S, F> {
    stream: S,
    f: F,
}

pub struct RefMap<S, F> {
    stream: S,
    f: F,
}

pub struct MarkedMap<S, F> {
    stream: S,
    f: F,
}

pub struct User;
pub struct Name;

impl User {
    pub fn name(&self) -> Name {}
}

impl<T> Stream for Source<T> {
    type Item = T;
}

impl<S, F, B> Stream for Map<S, F>
where
    S: Stream,
    F: FnMut(S::Item) -> B,
{
    type Item = B;
}

impl<S, F, B> Stream for RefMap<S, F>
where
    S: Stream,
    F: FnMut(&S::Item) -> B,
{
    type Item = B;
}

impl<S, F, B> Stream for MarkedMap<S, F>
where
    S: Stream,
    F: FnMut(S::Item) -> B,
    B: Marker,
{
    type Item = B;
}

pub fn use_it(source: Source<User>) {
    let mapped = source.map(|user| user.name()).get()$0; // ty("callable impl where associated input projection").in_lib("analysis_callable_impl_where_assoc_input_projection")
    let ref_mapped = source.map_ref(|user| user.name()).get()$0; // ty("callable impl where referenced associated input projection").in_lib("analysis_callable_impl_where_assoc_input_projection")
    let marked = source.map_marked(|user| user.name()).get()$0; // ty("unsupported impl where projection").in_lib("analysis_callable_impl_where_assoc_input_projection")
}
"#,
        expect![[r#"
            nominal struct analysis_callable_impl_where_assoc_input_projection[lib]::crate::Name // callable impl where associated input projection
            nominal struct analysis_callable_impl_where_assoc_input_projection[lib]::crate::Name // callable impl where referenced associated input projection
            <unknown> // unsupported impl where projection
        "#]],
    );
}

#[test]
fn propagates_expected_types_through_result_expressions() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_result_expression_expected_type_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub enum Kind {
    A,
    B,
}

pub fn id<T>(value: T) -> T {}

pub fn missing<T>() -> T {}

pub fn use_it(flag: bool, kind: Kind, user: User) {
    let block_user: User = { id(missing())$0 }$1; // ty("block generic call result"), ty("block expression")
    let if_user: User = if flag {
        id(missing())$0 // ty("if then generic call result")
    } else {
        user$0 // ty("if else result")
    }$0; // ty("if expression")
    let match_user: User = match kind {
        Kind::A => id(missing())$0, // ty("match arm generic call result")
        Kind::B => user$0, // ty("match arm known result")
    }$0; // ty("match expression")
}
"#,
        expect![[r#"
            nominal struct analysis_result_expression_expected_type_inference[lib]::crate::User // block generic call result
            nominal struct analysis_result_expression_expected_type_inference[lib]::crate::User // block expression
            nominal struct analysis_result_expression_expected_type_inference[lib]::crate::User // if then generic call result
            nominal struct analysis_result_expression_expected_type_inference[lib]::crate::User // if else result
            nominal struct analysis_result_expression_expected_type_inference[lib]::crate::User // if expression
            nominal struct analysis_result_expression_expected_type_inference[lib]::crate::User // match arm generic call result
            nominal struct analysis_result_expression_expected_type_inference[lib]::crate::User // match arm known result
            nominal struct analysis_result_expression_expected_type_inference[lib]::crate::User // match expression
        "#]],
    );
}

#[test]
fn treats_explicit_call_wildcard_generics_as_inference_variables() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_explicit_call_wildcard_generic_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub struct Vec<T> {
    value: T,
}

impl<T> Vec<T> {
    pub fn push(&mut self, value: T) {}
}

pub fn make<T>() -> T {}

pub fn use_it(user: User) {
    let concrete = make::<Vec<User>>()$0; // ty("explicit concrete generic arg")
    let direct: User = make::<_>()$0; // ty("explicit root wildcard constrained by annotation")
    let annotated: Vec<User> = make::<Vec<_>>()$0; // ty("explicit wildcard constrained by annotation")
    let mut later = make::<Vec<_>>()$0; // ty("explicit wildcard initializer")
    later.push(user);
    later$0; // ty("explicit wildcard read after method evidence")
}
"#,
        expect![[r#"
            nominal struct analysis_explicit_call_wildcard_generic_inference[lib]::crate::Vec<nominal struct analysis_explicit_call_wildcard_generic_inference[lib]::crate::User> // explicit concrete generic arg
            nominal struct analysis_explicit_call_wildcard_generic_inference[lib]::crate::User // explicit root wildcard constrained by annotation
            nominal struct analysis_explicit_call_wildcard_generic_inference[lib]::crate::Vec<nominal struct analysis_explicit_call_wildcard_generic_inference[lib]::crate::User> // explicit wildcard constrained by annotation
            nominal struct analysis_explicit_call_wildcard_generic_inference[lib]::crate::Vec<nominal struct analysis_explicit_call_wildcard_generic_inference[lib]::crate::User> // explicit wildcard initializer
            nominal struct analysis_explicit_call_wildcard_generic_inference[lib]::crate::Vec<nominal struct analysis_explicit_call_wildcard_generic_inference[lib]::crate::User> // explicit wildcard read after method evidence
        "#]],
    );
}

#[test]
fn uses_call_arguments_as_function_generic_evidence() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_call_argument_generic_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub struct Vec<T> {
    value: T,
}

pub fn id<T>(value: T) -> T {}
pub fn wrap<T>(value: T) -> Vec<T> {}
pub fn array<T, const N: usize>(value: [T; N]) -> [T; N] { value }
pub fn make_user() -> User {}
pub fn missing<T>() -> T {}
pub fn takes_vec(value: Vec<User>) {}

pub fn use_it(user: User) {
    let direct = id(user)$0; // ty("direct generic arg")
    let from_call = id(make_user())$0; // ty("generic arg from call result")
    let wrapped = wrap(user)$0; // ty("wrapped generic arg")
    let explicit = wrap::<_>(user)$0; // ty("explicit wildcard generic arg")
    let byte = id(1u8)$0; // ty("independent byte call")
    let flag = id(true)$0; // ty("independent bool call")
    let array = array([true; 3])$0; // ty("inferred array element and length")

    let from_return: User = id(missing()$0)$1; // ty("inner generic call solved from return"), ty("outer generic call solved from return")
    takes_vec(wrap::<_>(missing()$0)$1); // ty("inner generic call solved from arg"), ty("outer generic call solved from arg")
}
"#,
        expect![[r#"
            nominal struct analysis_call_argument_generic_inference[lib]::crate::User // direct generic arg
            nominal struct analysis_call_argument_generic_inference[lib]::crate::User // generic arg from call result
            nominal struct analysis_call_argument_generic_inference[lib]::crate::Vec<nominal struct analysis_call_argument_generic_inference[lib]::crate::User> // wrapped generic arg
            nominal struct analysis_call_argument_generic_inference[lib]::crate::Vec<nominal struct analysis_call_argument_generic_inference[lib]::crate::User> // explicit wildcard generic arg
            u8 // independent byte call
            bool // independent bool call
            [bool; 3] // inferred array element and length
            nominal struct analysis_call_argument_generic_inference[lib]::crate::User // inner generic call solved from return
            nominal struct analysis_call_argument_generic_inference[lib]::crate::User // outer generic call solved from return
            nominal struct analysis_call_argument_generic_inference[lib]::crate::User // inner generic call solved from arg
            nominal struct analysis_call_argument_generic_inference[lib]::crate::Vec<nominal struct analysis_call_argument_generic_inference[lib]::crate::User> // outer generic call solved from arg
        "#]],
    );
}

#[test]
fn alias_placeholders_share_later_evidence_between_components() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "source_type_identity"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
type Pair<T> = (T, T);
fn missing<T>() -> T { loop {} }
fn require(_: u16) {}

pub fn use_it() {
    let $0pair: Pair<_> = missing(); // ty("alias shares its inferred argument")
    let $0right = pair.1; // ty("second component learns from the first")
    require(pair.0);
}
"#,
        expect![[r#"
            (u16, u16) // alias shares its inferred argument
            u16 // second component learns from the first
        "#]],
    );
}

#[test]
fn propagates_associated_function_prefix_generics() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_associated_function_prefix_generic_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub struct Vec<T> {
    value: T,
}

pub struct Borrowed<'value, T> {
    value: &'value T,
}

pub struct Guard<'value> {
    value: &'value User,
}

pub trait Label {
    fn label(self) -> User;
}

impl<T> Vec<T> {
    pub fn new() -> Self {}
    pub fn singleton(value: T) -> Self {}
}

impl<'value, T> Borrowed<'value, T> {
    pub fn new(value: &'value T) -> Self {}
    pub fn into_value(self) -> T {}
}

impl<'value> Guard<'value> {
    pub fn new(value: &'value User) -> Self {}
}

impl<'value> Label for Guard<'value> {
    fn label(self) -> User {}
}

pub fn use_it(user: User) {
    let inferred: Vec<User> = Vec::new()$0; // ty("associated function inferred prefix generic")
    let explicit = Vec::<User>::new()$0; // ty("associated function explicit prefix generic")
    let wildcard = Vec::<_>::new()$0; // ty("associated function wildcard prefix generic")
    let singleton = Vec::singleton(user)$0; // ty("associated function impl generic argument")
    let borrowed: Borrowed<User> = Borrowed::ne$0w(&user)$1; // hover("hover lifetime generic associated constructor"), ty("lifetime generic associated constructor result")
    let explicit_borrowed = Borrowed::<User>::new(&user)$0; // ty("lifetime generic explicit prefix constructor result")
    let chained = Borrowed::new(&user).into_$0value(); // hover("hover method after lifetime generic constructor")
    let guard = Guard::new(&user)$0; // ty("lifetime-only associated constructor result")
    let label = Guard::new(&user).label()$0; // ty("trait method after lifetime-only constructor result")
}
"#,
        expect![[r#"
            nominal struct analysis_associated_function_prefix_generic_inference[lib]::crate::Vec<nominal struct analysis_associated_function_prefix_generic_inference[lib]::crate::User> // associated function inferred prefix generic
            nominal struct analysis_associated_function_prefix_generic_inference[lib]::crate::Vec<nominal struct analysis_associated_function_prefix_generic_inference[lib]::crate::User> // associated function explicit prefix generic
            nominal struct analysis_associated_function_prefix_generic_inference[lib]::crate::Vec<<unknown>> // associated function wildcard prefix generic
            nominal struct analysis_associated_function_prefix_generic_inference[lib]::crate::Vec<nominal struct analysis_associated_function_prefix_generic_inference[lib]::crate::User> // associated function impl generic argument

            // hover lifetime generic associated constructor
            range: 42:46-42:49
            block:
              kind: method
              path: analysis_associated_function_prefix_generic_inference::Borrowed::new
              signature:
                pub fn new(value: &'value T) -> Self

            nominal struct analysis_associated_function_prefix_generic_inference[lib]::crate::Borrowed<'_, nominal struct analysis_associated_function_prefix_generic_inference[lib]::crate::User> // lifetime generic associated constructor result
            nominal struct analysis_associated_function_prefix_generic_inference[lib]::crate::Borrowed<'_, nominal struct analysis_associated_function_prefix_generic_inference[lib]::crate::User> // lifetime generic explicit prefix constructor result

            // hover method after lifetime generic constructor
            range: 44:40-44:50
            block:
              kind: method
              path: analysis_associated_function_prefix_generic_inference::Borrowed::into_value
              signature:
                pub fn into_value(self) -> T

            nominal struct analysis_associated_function_prefix_generic_inference[lib]::crate::Guard<'_> // lifetime-only associated constructor result
            nominal struct analysis_associated_function_prefix_generic_inference[lib]::crate::User // trait method after lifetime-only constructor result
        "#]],
    );
}

#[test]
fn carries_generic_variables_through_simple_bindings() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_simple_binding_generic_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub struct Vec<T> {
    value: T,
}

impl<T> Vec<T> {
    pub fn new() -> Self {}
}

pub fn use_it() {
    let values = Vec::new()$0; // ty("binding initializer generic result")
    let typed: Vec<User> = values$0; // ty("binding read constrained by annotation")
    let alias = values$0; // ty("binding read through alias")
    let later: Vec<User> = alias$0; // ty("alias read constrained by annotation")
    let wrapped = (Vec::new(),)$0; // ty("wrapped binding initializer")
    let wrapped_later: (Vec<User>,) = wrapped$0; // ty("wrapped binding read")
}
"#,
        expect![[r#"
            nominal struct analysis_simple_binding_generic_inference[lib]::crate::Vec<nominal struct analysis_simple_binding_generic_inference[lib]::crate::User> // binding initializer generic result
            nominal struct analysis_simple_binding_generic_inference[lib]::crate::Vec<nominal struct analysis_simple_binding_generic_inference[lib]::crate::User> // binding read constrained by annotation
            nominal struct analysis_simple_binding_generic_inference[lib]::crate::Vec<nominal struct analysis_simple_binding_generic_inference[lib]::crate::User> // binding read through alias
            nominal struct analysis_simple_binding_generic_inference[lib]::crate::Vec<nominal struct analysis_simple_binding_generic_inference[lib]::crate::User> // alias read constrained by annotation
            (nominal struct analysis_simple_binding_generic_inference[lib]::crate::Vec<nominal struct analysis_simple_binding_generic_inference[lib]::crate::User>,) // wrapped binding initializer
            (nominal struct analysis_simple_binding_generic_inference[lib]::crate::Vec<nominal struct analysis_simple_binding_generic_inference[lib]::crate::User>,) // wrapped binding read
        "#]],
    );
}

#[test]
fn constrains_receiver_generic_variables_from_method_arguments() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_method_receiver_generic_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub struct Vec<T> {
    value: T,
}

pub struct Wrapper<T> {
    value: T,
}

impl<T> Vec<T> {
    pub fn new() -> Self {}
    pub fn push(&mut self, value: T) {}
}

impl<T> Wrapper<T> {
    pub fn new() -> Self {}
}

impl Wrapper<User> {
    pub fn touch(&self) {}
}

pub fn user_value(user: User) {
    let mut values = Vec::new()$0; // ty("user receiver initializer")
    values.push(user);
    values$0; // ty("user receiver read")
}

pub fn suffixed_integer() {
    let mut values = Vec::new()$0; // ty("u64 receiver initializer")
    values.push(10u64);
    values$0; // ty("u64 receiver read")
}

pub fn unsuffixed_integer() {
    let mut values = Vec::new()$0; // ty("i32 receiver initializer")
    values.push(10);
    values$0; // ty("i32 receiver read")
}

pub fn conflicting() {
    let mut values = Vec::new()$0; // ty("conflicting receiver initializer")
    values.push(10u64);
    values.push(false$0); // ty("conflicting receiver argument")
    values$0; // ty("conflicting receiver read")
}

pub fn selected_inherent_receiver() {
    let wrapper = Wrapper::new()$0; // ty("selected receiver initializer")
    wrapper.touch();
    wrapper$0; // ty("selected receiver read")
}
"#,
        expect![[r#"
            nominal struct analysis_method_receiver_generic_inference[lib]::crate::Vec<nominal struct analysis_method_receiver_generic_inference[lib]::crate::User> // user receiver initializer
            nominal struct analysis_method_receiver_generic_inference[lib]::crate::Vec<nominal struct analysis_method_receiver_generic_inference[lib]::crate::User> // user receiver read
            nominal struct analysis_method_receiver_generic_inference[lib]::crate::Vec<u64> // u64 receiver initializer
            nominal struct analysis_method_receiver_generic_inference[lib]::crate::Vec<u64> // u64 receiver read
            nominal struct analysis_method_receiver_generic_inference[lib]::crate::Vec<i32> // i32 receiver initializer
            nominal struct analysis_method_receiver_generic_inference[lib]::crate::Vec<i32> // i32 receiver read
            nominal struct analysis_method_receiver_generic_inference[lib]::crate::Vec<u64> // conflicting receiver initializer
            bool // conflicting receiver argument
            nominal struct analysis_method_receiver_generic_inference[lib]::crate::Vec<u64> // conflicting receiver read
            nominal struct analysis_method_receiver_generic_inference[lib]::crate::Wrapper<nominal struct analysis_method_receiver_generic_inference[lib]::crate::User> // selected receiver initializer
            nominal struct analysis_method_receiver_generic_inference[lib]::crate::Wrapper<nominal struct analysis_method_receiver_generic_inference[lib]::crate::User> // selected receiver read
        "#]],
    );
}

#[test]
fn carries_generic_variables_through_member_projections() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_member_projection_generic_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub struct Boxed<T> {
    value: T,
}

impl<T> Boxed<T> {
    pub fn new() -> Self {}
}

pub struct Vec<T> {
    value: T,
}

impl<T> Vec<T> {
    pub fn new() -> Self {}
    pub fn push(&mut self, value: T) {}
}

pub fn declared_field() {
    let boxed = Boxed::new()$0; // ty("declared field owner initializer")
    let _typed: User = boxed.value$0; // ty("declared field projection")
    boxed$0; // ty("declared field owner read")
}

pub fn declared_field_through_ref() {
    let boxed = Boxed::new()$0; // ty("reference field owner initializer")
    let _typed: User = (&boxed).value$0; // ty("reference field projection")
    boxed$0; // ty("reference field owner read")
}

pub fn tuple_field(user: User) {
    let pair = (Vec::new(),)$0; // ty("tuple owner initializer")
    pair.0$0.push(user); // ty("tuple field projection")
    pair$0; // ty("tuple owner read")
}
"#,
        expect![[r#"
            nominal struct analysis_member_projection_generic_inference[lib]::crate::Boxed<nominal struct analysis_member_projection_generic_inference[lib]::crate::User> // declared field owner initializer
            nominal struct analysis_member_projection_generic_inference[lib]::crate::User // declared field projection
            nominal struct analysis_member_projection_generic_inference[lib]::crate::Boxed<nominal struct analysis_member_projection_generic_inference[lib]::crate::User> // declared field owner read
            nominal struct analysis_member_projection_generic_inference[lib]::crate::Boxed<nominal struct analysis_member_projection_generic_inference[lib]::crate::User> // reference field owner initializer
            nominal struct analysis_member_projection_generic_inference[lib]::crate::User // reference field projection
            nominal struct analysis_member_projection_generic_inference[lib]::crate::Boxed<nominal struct analysis_member_projection_generic_inference[lib]::crate::User> // reference field owner read
            (nominal struct analysis_member_projection_generic_inference[lib]::crate::Vec<nominal struct analysis_member_projection_generic_inference[lib]::crate::User>,) // tuple owner initializer
            nominal struct analysis_member_projection_generic_inference[lib]::crate::Vec<nominal struct analysis_member_projection_generic_inference[lib]::crate::User> // tuple field projection
            (nominal struct analysis_member_projection_generic_inference[lib]::crate::Vec<nominal struct analysis_member_projection_generic_inference[lib]::crate::User>,) // tuple owner read
        "#]],
    );
}

#[test]
fn carries_generic_variables_through_structural_patterns() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_structural_pattern_generic_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub struct Vec<T> {
    value: T,
}

impl<T> Vec<T> {
    pub fn new() -> Self {}
    pub fn push(&mut self, value: T) {}
}

pub fn tuple_pattern(user: User) {
    let (values,) = (Vec::new(),)$0; // ty("tuple pattern initializer")
    values.push(user);
    values$0; // ty("tuple pattern binding read")
}

pub fn reference_pattern(user: User) {
    let &(values,) = (&(Vec::new(),))$0; // ty("reference pattern initializer")
    values.push(user);
    values$0; // ty("reference pattern binding read")
}

pub fn slice_pattern(user: User) {
    let [values, ..] = [Vec::new()]$0; // ty("slice pattern initializer")
    values.push(user);
    values$0; // ty("slice pattern binding read")
}
"#,
        expect![[r#"
            (nominal struct analysis_structural_pattern_generic_inference[lib]::crate::Vec<nominal struct analysis_structural_pattern_generic_inference[lib]::crate::User>,) // tuple pattern initializer
            nominal struct analysis_structural_pattern_generic_inference[lib]::crate::Vec<nominal struct analysis_structural_pattern_generic_inference[lib]::crate::User> // tuple pattern binding read
            &(nominal struct analysis_structural_pattern_generic_inference[lib]::crate::Vec<nominal struct analysis_structural_pattern_generic_inference[lib]::crate::User>,) // reference pattern initializer
            nominal struct analysis_structural_pattern_generic_inference[lib]::crate::Vec<nominal struct analysis_structural_pattern_generic_inference[lib]::crate::User> // reference pattern binding read
            [nominal struct analysis_structural_pattern_generic_inference[lib]::crate::Vec<nominal struct analysis_structural_pattern_generic_inference[lib]::crate::User>; 1] // slice pattern initializer
            nominal struct analysis_structural_pattern_generic_inference[lib]::crate::Vec<nominal struct analysis_structural_pattern_generic_inference[lib]::crate::User> // slice pattern binding read
        "#]],
    );
}

#[test]
fn projects_borrowed_patterns_through_live_type_variables() {
    check_analysis_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_borrowed_patterns"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User { pub count: u64 }
pub struct Account<T> { pub user: T }
pub struct Holder { pub account: Account<User>, pub optional: Option<User> }

pub fn borrowed(holder: &Holder) {
    let Account { user } = &&holder.account;
    user$0; // ty("borrowed record binding").in_lib("analysis_borrowed_patterns")
    user.count$0; // ty("borrowed record field").in_lib("analysis_borrowed_patterns")
    if let Some(user) = &holder.optional {
        user$0; // ty("borrowed variant binding").in_lib("analysis_borrowed_patterns")
        user.count$0; // ty("borrowed variant field").in_lib("analysis_borrowed_patterns")
    }
}

pub fn apply<F: FnOnce(Holder)>(f: F) {}
pub fn deferred() {
    apply((|holder| {
        let Account { user } = &holder.account;
        user$0; // ty("deferred borrowed record binding").in_lib("analysis_borrowed_patterns")
        user.count$0; // ty("deferred borrowed record field").in_lib("analysis_borrowed_patterns")
        if let Some(user) = &holder.optional {
            user$0; // ty("deferred borrowed variant binding").in_lib("analysis_borrowed_patterns")
            user.count$0; // ty("deferred borrowed variant field").in_lib("analysis_borrowed_patterns")
        }
    }));
}

pub fn account<T>() -> Account<T> { loop {} }
pub fn accept(user: &User) {}
pub fn inferred_field() {
    let Account { user } = (&account())$0; // ty("inferred borrowed initializer").in_lib("analysis_borrowed_patterns")
    accept(user);
    user.count$0; // ty("inferred borrowed field").in_lib("analysis_borrowed_patterns")
}

pub fn borrowed_mut(holder: &mut Holder) {
    let Account { user } = &mut holder.account;
    user$0; // ty("mutable borrowed binding").in_lib("analysis_borrowed_patterns")
    let Account { user } = &&mut holder.account;
    user$0; // ty("shared borrow of mutable reference").in_lib("analysis_borrowed_patterns")
}
"#,
        expect![[r#"
            &nominal struct analysis_borrowed_patterns[lib]::crate::User // borrowed record binding
            u64 // borrowed record field
            &nominal struct analysis_borrowed_patterns[lib]::crate::User // borrowed variant binding
            u64 // borrowed variant field
            &nominal struct analysis_borrowed_patterns[lib]::crate::User // deferred borrowed record binding
            u64 // deferred borrowed record field
            &nominal struct analysis_borrowed_patterns[lib]::crate::User // deferred borrowed variant binding
            u64 // deferred borrowed variant field
            &nominal struct analysis_borrowed_patterns[lib]::crate::Account<nominal struct analysis_borrowed_patterns[lib]::crate::User> // inferred borrowed initializer
            u64 // inferred borrowed field
            &mut nominal struct analysis_borrowed_patterns[lib]::crate::User // mutable borrowed binding
            &nominal struct analysis_borrowed_patterns[lib]::crate::User // shared borrow of mutable reference
        "#]],
    );
}

#[test]
fn uses_simple_assignments_as_equality_evidence() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_assignment_generic_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn missing<T>() -> T {}
pub fn id<T>(value: T) -> T {}

pub fn use_it(user: User) {
    let mut value = id(missing()$0)$1; // ty("assignment missing argument"), ty("assignment initializer")
    value = user;
    value$0; // ty("assignment binding read")
}
"#,
        expect![[r#"
            nominal struct analysis_assignment_generic_inference[lib]::crate::User // assignment missing argument
            nominal struct analysis_assignment_generic_inference[lib]::crate::User // assignment initializer
            nominal struct analysis_assignment_generic_inference[lib]::crate::User // assignment binding read
        "#]],
    );
}

#[test]
fn infers_enum_variant_payload_generics_bidirectionally() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_enum_payload_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct Error;

pub enum Slot<T> {
    Put(T),
}

pub enum Option<T> {
    Some(T),
    None,
}

pub enum Result<T, E> {
    Ok(T),
    Err(E),
}

pub enum Action {
    Set(u64, f32),
    Pair((u64, f32)),
}

pub enum Borrowed<'a, T, const N: usize> {
    Array(&'a [T; N]),
}
use Borrowed::Array;

pub fn use_it(user: User, error: Error) {
    let slot = Slot::<u64>::Put(1$0)$1; // ty("enum variant explicit generic payload"), ty("enum variant explicit generic result")

    let maybe = Option::Some(user)$0; // ty("option generic variant result")
    let result = Result::Ok(error)$0; // ty("result generic variant result")

    let _action = Action::Set(1$0, 1.0$1); // ty("enum variant integer payload"), ty("enum variant float payload")
    let _pair = Action::Pair((1$0, 1.0$1)$2); // ty("enum variant tuple integer field"), ty("enum variant tuple float field"), ty("enum variant tuple payload")
    let borrowed = Array(&[user; 3])$0; // ty("imported variant retains all generic kinds")
}
"#,
        expect![[r#"
            u64 // enum variant explicit generic payload
            nominal enum analysis_enum_payload_inference[lib]::crate::Slot<u64> // enum variant explicit generic result
            nominal enum analysis_enum_payload_inference[lib]::crate::Option<nominal struct analysis_enum_payload_inference[lib]::crate::User> // option generic variant result
            nominal enum analysis_enum_payload_inference[lib]::crate::Result<nominal struct analysis_enum_payload_inference[lib]::crate::Error, <unknown>> // result generic variant result
            u64 // enum variant integer payload
            f32 // enum variant float payload
            u64 // enum variant tuple integer field
            f32 // enum variant tuple float field
            (u64, f32) // enum variant tuple payload
            nominal enum analysis_enum_payload_inference[lib]::crate::Borrowed<'_, nominal struct analysis_enum_payload_inference[lib]::crate::User, 3> // imported variant retains all generic kinds
        "#]],
    );
}

#[test]
fn infers_record_fields_bidirectionally() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_bidirectional_record_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct Error;

pub struct PrimitiveFields {
    left: u64,
    right: f32,
    nested: (u64, f32),
}

pub struct UserPair {
    left: User,
}

pub struct Pair<T> {
    left: T,
}

pub struct Pair2<T, E> {
    left: T,
    right: E,
}

pub struct Same<T> {
    left: T,
    right: T,
}

pub fn id<T>(value: T) -> T {}

pub fn use_it(user: User, error: Error) {
    let _primitive = PrimitiveFields {
        left: 1$0, // ty("record integer field initializer")
        right: 1.0$0, // ty("record float field initializer")
        nested: (1$0, 1.0$1)$2, // ty("record tuple integer field"), ty("record tuple float field"), ty("record tuple field initializer")
    };
    let _user_pair = UserPair {
        left: id(user)$0, // ty("record generic call field initializer")
    };

    let pair = Pair { left: user }$0; // ty("record generic field result")
    let pair2 = Pair2 { left: user, right: error }$0; // ty("record two generic field result")
    let explicit = Pair::<_> { left: error }$0; // ty("record wildcard generic field result")
    let conflict = Same { left: user, right: error }$0; // ty("record conflicting generic field result")
}
"#,
        expect![[r#"
            u64 // record integer field initializer
            f32 // record float field initializer
            u64 // record tuple integer field
            f32 // record tuple float field
            (u64, f32) // record tuple field initializer
            nominal struct analysis_bidirectional_record_inference[lib]::crate::User // record generic call field initializer
            nominal struct analysis_bidirectional_record_inference[lib]::crate::Pair<nominal struct analysis_bidirectional_record_inference[lib]::crate::User> // record generic field result
            nominal struct analysis_bidirectional_record_inference[lib]::crate::Pair2<nominal struct analysis_bidirectional_record_inference[lib]::crate::User, nominal struct analysis_bidirectional_record_inference[lib]::crate::Error> // record two generic field result
            nominal struct analysis_bidirectional_record_inference[lib]::crate::Pair<nominal struct analysis_bidirectional_record_inference[lib]::crate::Error> // record wildcard generic field result
            nominal struct analysis_bidirectional_record_inference[lib]::crate::Same<nominal struct analysis_bidirectional_record_inference[lib]::crate::User> // record conflicting generic field result
        "#]],
    );
}

#[test]
fn late_associated_type_evidence_reaches_destructured_bindings_and_members() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_late_projection"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User { pub name: Name }
pub struct Name;
pub struct UserSource;
pub trait Source { type Item; }
impl Source for UserSource { type Item = User; }
pub fn missing<T>() -> T {}
pub fn wrap<S: Source>(source: S) -> (S::Item,) {}
pub fn require_source(source: UserSource) {}
impl User { pub fn name(&self) -> Name {} }

pub fn use_it() {
    let source = missing();
    let (user,) = wrap(source)$0; // ty("associated result")
    let field = user.name$0; // ty("field after pending projection")
    let method = user.name()$0; // ty("method after pending projection")
    require_source(source);
    user$0; // ty("destructured binding")
}
"#,
        expect![[r#"
            (nominal struct analysis_late_projection[lib]::crate::User,) // associated result
            nominal struct analysis_late_projection[lib]::crate::Name // field after pending projection
            nominal struct analysis_late_projection[lib]::crate::Name // method after pending projection
            nominal struct analysis_late_projection[lib]::crate::User // destructured binding
        "#]],
    );
}

#[test]
fn infers_try_output_with_a_residual_default_in_its_supertrait() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_try_supertrait"
version = "0.1.0"
edition = "2024"
//- /src/lib.rs
#[lang = "Try"]
pub trait Extract: FromResidual {
    type Output;
    type Residual;
}
pub trait FromResidual<R = <Self as Extract>::Residual> {}
pub struct Fallible<T>(T);
impl<T> Extract for Fallible<T> {
    type Output = T;
    type Residual = ();
}
impl<T> FromResidual<()> for Fallible<T> {}

pub fn use_it(input: Fallible<u16>) -> Fallible<()> {
    let output = input?$0; // ty("output through residual supertrait")
    Fallible(())
}
"#,
        expect![[r#"
            u16 // output through residual supertrait
        "#]],
    );
}

#[test]
fn infers_try_outputs_from_trait_implementations_and_bounds() {
    check_analysis_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_try_outputs"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Error;
pub struct Packet<E, T>(E, T);
impl<E, T> core::ops::Try for Packet<E, T> { type Output = T; }
pub type Alias<T> = Result<T, Error>;
pub fn result<T>() -> Result<T, Error> { loop {} }
pub fn option<T>() -> Option<T> { loop {} }
pub fn alias<T>() -> Alias<T> { loop {} }
pub fn packet<T>() -> Packet<Error, T> { loop {} }

pub fn use_result() -> Result<(), Error> {
    let value: u16 = result()$0?$1; // ty("Result operand").in_lib("analysis_try_outputs"), ty("Result output").in_lib("analysis_try_outputs")
    let aliased: u16 = alias()$0?$1; // ty("alias operand").in_lib("analysis_try_outputs"), ty("alias output").in_lib("analysis_try_outputs")
    let wildcard: u16 = alias::<_>()$0?$1; // ty("explicit wildcard alias operand").in_lib("analysis_try_outputs"), ty("explicit wildcard alias output").in_lib("analysis_try_outputs")
    Result::Ok(())
}

pub fn use_option() -> Option<()> {
    let value: u16 = option()$0?$1; // ty("Option operand").in_lib("analysis_try_outputs"), ty("Option output").in_lib("analysis_try_outputs")
    Option::Some(())
}

pub fn use_custom() -> Packet<Error, ()> {
    let value: u16 = packet()$0?$1; // ty("custom operand").in_lib("analysis_try_outputs"), ty("custom output").in_lib("analysis_try_outputs")
    Packet(Error, ())
}

pub fn generic<T: core::ops::Try<Output = U>, U>(value: T) -> T {
    let output = value?$0; // ty("output under generic bound").in_lib("analysis_try_outputs")
    loop {}
}
"#,
        expect![[r#"
            nominal enum core[lib]::crate::result::Result<u16, nominal struct analysis_try_outputs[lib]::crate::Error> // Result operand
            u16 // Result output
            nominal enum core[lib]::crate::result::Result<u16, nominal struct analysis_try_outputs[lib]::crate::Error> // alias operand
            u16 // alias output
            nominal enum core[lib]::crate::result::Result<u16, nominal struct analysis_try_outputs[lib]::crate::Error> // explicit wildcard alias operand
            u16 // explicit wildcard alias output
            nominal enum core[lib]::crate::option::Option<u16> // Option operand
            u16 // Option output
            nominal struct analysis_try_outputs[lib]::crate::Packet<nominal struct analysis_try_outputs[lib]::crate::Error, u16> // custom operand
            u16 // custom output
            param U // output under generic bound
        "#]],
    );
}

#[test]
fn late_try_operand_evidence_reaches_output_members_and_obligations() {
    check_analysis_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_late_try_operand"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User { pub id: u32 }
impl User { pub fn id(&self) -> u32 { self.id } }
pub trait Marker {}
impl Marker for User {}
pub trait Source { type Item; }
pub struct UserSource;
impl Source for UserSource { type Item = User; }
pub struct Carrier<S>(S);
impl<S: Source> core::ops::Try for Carrier<S> { type Output = S::Item; }
pub fn make<T>() -> T { loop {} }
pub fn require_source(value: Carrier<UserSource>) {}
pub fn consume<T: Marker>(value: T) -> T { value }

pub fn use_it() -> Carrier<UserSource> {
    let source = make();
    let user = source?$0; // ty("output after later operand evidence").in_lib("analysis_late_try_operand")
    let field = user.id$0; // ty("field on pending output").in_lib("analysis_late_try_operand")
    let method = user.id()$0; // ty("method on pending output").in_lib("analysis_late_try_operand")
    let consumed = consume(user)$0; // ty("dependent trait obligation").in_lib("analysis_late_try_operand")
    require_source(source);
    source$0; // ty("refined operand").in_lib("analysis_late_try_operand")
    loop {}
}
"#,
        expect![[r#"
            nominal struct analysis_late_try_operand[lib]::crate::User // output after later operand evidence
            u32 // field on pending output
            u32 // method on pending output
            nominal struct analysis_late_try_operand[lib]::crate::User // dependent trait obligation
            nominal struct analysis_late_try_operand[lib]::crate::Carrier<nominal struct analysis_late_try_operand[lib]::crate::UserSource> // refined operand
        "#]],
    );
}

#[test]
fn coerces_try_output_after_later_evidence_reveals_never() {
    check_analysis_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_try_never"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Stop;
impl core::ops::Try for Stop { type Output = !; }
pub fn make<T>() -> T { loop {} }
pub fn require_stop(value: Stop) {}

pub fn use_it() -> Stop {
    let source = make();
    let value: u8 = source?$0; // ty("diverging try output").in_lib("analysis_try_never")
    require_stop(source);
    value$0; // ty("coerced binding").in_lib("analysis_try_never")
    source$0; // ty("refined operand").in_lib("analysis_try_never")
    loop {}
}
"#,
        expect![[r#"
            ! // diverging try output
            u8 // coerced binding
            nominal struct analysis_try_never[lib]::crate::Stop // refined operand
        "#]],
    );
}

#[test]
fn retains_late_expectations_for_ambiguous_conversion_results() {
    check_analysis_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_conversion_expectation"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Conversion<T> { pub value: T }
impl<T> Conversion<T> {
    pub fn ok(self) -> Option<T> { Option::Some(self.value) }
}

pub trait Convert<T> {
    fn convert(self) -> Conversion<T> { loop {} }
}
pub trait FromNumber {}
impl FromNumber for u32 {}
impl FromNumber for u64 {}
impl<T: FromNumber> Convert<T> for usize {}

pub fn convert_later(number: usize) -> Option<u32> {
    let size = number.convert().ok()?$0; // ty("conversion initializer").in_lib("analysis_conversion_expectation")
    Option::Some(size$0) // ty("conversion binding read").in_lib("analysis_conversion_expectation")
}
"#,
        expect![[r#"
            u32 // conversion initializer
            u32 // conversion binding read
        "#]],
    );
}

#[test]
fn coerces_deferred_branch_results() {
    check_analysis_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_deferred_never_branches"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
impl User {
    pub fn abort(self) -> ! { loop {} }
    pub fn value<T>(self) -> T { loop {} }
}
pub fn apply<R, F: FnOnce(User) -> R>(f: F) -> R { f(User) }
pub fn make<T>() -> T { loop {} }
pub trait Source { type Item; }
impl Source for User { type Item = !; }
pub fn project<S: Source>(source: S) -> S::Item { loop {} }

pub fn use_it(flag: bool) {
    let numeric = apply((|user| if flag {
        user.abort()$0 // ty("deferred diverging branch").in_lib("analysis_deferred_never_branches")
    } else {
        1$0 // ty("numeric alternate branch").in_lib("analysis_deferred_never_branches")
    }))$0; // ty("numeric call result").in_lib("analysis_deferred_never_branches")
    let generic = apply((|user| if flag {
        user.abort()
    } else {
        make()$0 // ty("unconstrained generic branch").in_lib("analysis_deferred_never_branches")
    }))$0; // ty("unconstrained generic call result").in_lib("analysis_deferred_never_branches")
    let annotated: u64 = apply((|user| if flag {
        user.abort()$0 // ty("annotated diverging branch").in_lib("analysis_deferred_never_branches")
    } else {
        make()$0 // ty("annotated generic branch").in_lib("analysis_deferred_never_branches")
    }))$0; // ty("annotated call result").in_lib("analysis_deferred_never_branches")
    let matched = apply((|user| match flag {
        true => user.abort()$0, // ty("deferred diverging match arm").in_lib("analysis_deferred_never_branches")
        false => 1$0, // ty("numeric alternate match arm").in_lib("analysis_deferred_never_branches")
    }))$0; // ty("match call result").in_lib("analysis_deferred_never_branches")
    let selected = apply((|user| if flag {
        1
    } else {
        user.value()$0 // ty("deferred generic branch").in_lib("analysis_deferred_never_branches")
    }))$0; // ty("deferred generic call result").in_lib("analysis_deferred_never_branches")
    let projected = apply((|user| if flag {
        1
    } else {
        project(user)$0 // ty("deferred diverging projection").in_lib("analysis_deferred_never_branches")
    }))$0; // ty("projected call result").in_lib("analysis_deferred_never_branches")
}
"#,
        expect![[r#"
            ! // deferred diverging branch
            i32 // numeric alternate branch
            i32 // numeric call result
            <unknown> // unconstrained generic branch
            <unknown> // unconstrained generic call result
            ! // annotated diverging branch
            u64 // annotated generic branch
            u64 // annotated call result
            ! // deferred diverging match arm
            i32 // numeric alternate match arm
            i32 // match call result
            i32 // deferred generic branch
            i32 // deferred generic call result
            ! // deferred diverging projection
            i32 // projected call result
        "#]],
    );
}

#[test]
fn generic_body_projection_feeds_a_dependent_call_obligation() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_shared_solver_environment"
version = "0.1.0"
edition = "2024"
//- /src/lib.rs
pub trait Produce {
    type Item;
    fn get(self) -> Self::Item;
}

pub trait Marker {}
pub fn consume<T: Marker>(value: T) -> T { value }
pub fn relay<I: Produce<Item = T>, T: Marker>(input: I) -> T {
    let item = input.get()$0; // ty("projection under generic assumptions")
    consume(item)$0 // ty("dependent obligation under generic assumptions")
}
"#,
        expect![[r#"
            param T // projection under generic assumptions
            param T // dependent obligation under generic assumptions
        "#]],
    );
}

#[test]
fn infers_iterator_items_with_unrelated_reference_impls() {
    check_analysis_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_iterator_impl_shapes"
version = "0.1.0"
edition = "2024"
//- /src/lib.rs
pub trait Reflect {}
impl<'a> IntoIterator for &'a dyn Reflect {
    type Item = u32;
    type IntoIter = Items<u32>;
    fn into_iter(self) -> Self::IntoIter { loop {} }
}
pub struct Items<T> { value: T }
impl<T> Iterator for Items<T> { type Item = T; }
pub struct User { pub id: u64 }
pub fn run() {
    let items = Items { value: User { id: 7 } };
    for item in items.into_iter() {
        item.id$0; // ty("iterator item field").in_lib("analysis_iterator_impl_shapes")
    }
}
"#,
        expect![[r#"
            u64 // iterator item field
        "#]],
    );
}

#[test]
fn infers_trait_arguments_from_a_unique_methods_signature() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_trait_call_arguments"
version = "0.1.0"
edition = "2024"
//- /src/lib.rs
pub trait Marker {}
pub struct User { pub id: u64 }
impl Marker for User {}
pub struct Handler;
pub trait Convert<T> { fn convert(&self, value: T) -> T; }
impl<T: Marker> Convert<T> for Handler {
    fn convert(&self, value: T) -> T { value }
}

pub fn run() {
    let user = Handler.convert(User { id: 7 });
    user.id$0; // ty("converted field")
}
"#,
        expect![[r#"
            u64 // converted field
        "#]],
    );
}

#[test]
fn infers_projection_after_numeric_receiver_becomes_known() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_deferred_numeric_projection"
version = "0.1.0"
edition = "2024"
//- /src/lib.rs
pub struct User { pub id: u64 }
pub trait Produce { type Item; }
pub trait Reflect {}
impl Produce for &dyn Reflect { type Item = bool; }
impl Produce for u32 { type Item = User; }
pub fn produce<T: Produce>(value: T) -> T::Item { loop {} }
pub fn run() {
    let number = 0;
    let item = produce(number)$0; // ty("projection after numeric evidence")
    let other = User { id: 1 };
    other.id$0; // ty("independent field")
    let _: u32 = number;
    item.id$0; // ty("projected field")
}
"#,
        expect![[r#"
            nominal struct analysis_deferred_numeric_projection[lib]::crate::User // projection after numeric evidence
            u64 // independent field
            u64 // projected field
        "#]],
    );
}

#[test]
fn resolves_impl_bound_aliases_before_checking_applicability() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_impl_bound_aliases"
version = "0.1.0"
edition = "2024"
//- /src/lib.rs
pub trait Marker {}
pub struct Good;
pub struct Other;
impl Marker for Good {}

pub struct Factory<T> { pub value: T }
impl<T> Factory<T> where Self::Item: Marker {
    type Item = T;
    pub fn get(self) -> Self::Item { self.value }
}

pub fn saved() {
    let good = Factory { value: Good }.get()$0; // ty("saved alias with satisfied bound")
    let other = Factory { value: Other }.get()$0; // ty("saved alias with unsatisfied bound")
}

pub fn lexical() {
    struct Local;
    impl Local where Local::Item: Marker {
        type Item = Good;
        fn get(self) -> Self::Item { Good }
    }
    let value = Local.get()$0; // ty("body-local alias in its own bound")
}

pub struct Guard;
impl Guard where Other: Marker {
    pub fn requires_parent_bound(self) -> bool { true }
}
pub fn check_parent() {
    let result = Guard.requires_parent_bound()$0; // ty("nongeneric enclosing impl requirement")
}
"#,
        expect![[r#"
            nominal struct analysis_impl_bound_aliases[lib]::crate::Good // saved alias with satisfied bound
            <unknown> // saved alias with unsatisfied bound
            nominal struct analysis_impl_bound_aliases[lib]::crate::Good // body-local alias in its own bound
            <unknown> // nongeneric enclosing impl requirement
        "#]],
    );
}

#[test]
fn infers_inherited_associated_equalities_and_displays_their_bounds() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_inherited_equalities"
version = "0.1.0"
edition = "2024"
//- /src/lib.rs
pub struct Vec<T> { pub value: T }
pub struct Option<T> { pub value: T }
pub trait Base<T> {
    type Item;
    fn item(&self) -> Self::Item;
}
pub trait Derived<T>: Base<Vec<T>> {}
pub trait Further<T>: Derived<Option<T>> {}
pub trait Other<T> { type Item; }

pub fn direct<V: Derived<u8, Item = u64>>(value: V) {
    let result = value.item()$0; // ty("inherited equality")
}
pub fn further<V: Further<u8, Item = bool>>(value: V) {
    let result = value.item()$0; // ty("two supertrait substitutions")
}
pub fn display(value: impl Derived<u8, Item = u64> + Other<u8, Item = bool>) {
    let saved = value$0; // ty("bindings stay with their trait")
    let result = saved.item()$0; // ty("argument impl Trait equality")
}
"#,
        expect![[r#"
            u64 // inherited equality
            bool // two supertrait substitutions
            impl trait analysis_inherited_equalities[lib]::crate::Derived<u8, Item = u64> + trait analysis_inherited_equalities[lib]::crate::Other<u8, Item = bool> // bindings stay with their trait
            u64 // argument impl Trait equality
        "#]],
    );
}

#[test]
fn infers_labeled_block_exit_values() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_labeled_block_exits"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it(flag: bool) {
    let semicolon = 'done: { break 'done 1u64; }$0; // ty("break statement")
    let tail = 'done: { break 'done 2u64 }$0; // ty("break tail")
    let nested = 'done: { { break 'done 3u64; } }$0; // ty("break through ordinary block")
    let initializer = 'done: { let _ = break 'done 3u64; }$0; // ty("break in let initializer")
    let branches = 'done: {
        if flag { break 'done 4u64; } else { break 'done 5u64; }
    }$0; // ty("both branches break")
    let fallthrough = 'done: {
        if flag { break 'done 6u64; }
        7u64
    }$0; // ty("break and normal tail")
    let return_branch = 'done: {
        if flag { return; }
        break 'done 8u64;
    }$0; // ty("return and break")
    let unit_break = 'done: { break 'done; }$0; // ty("unit break")
    let unit_fallthrough = 'done: { if flag { break 'done; } }$0; // ty("unit fallthrough")
    let empty = 'done: {}$0; // ty("empty labeled block")
}
"#,
        expect![[r#"
            u64 // break statement
            u64 // break tail
            u64 // break through ordinary block
            u64 // break in let initializer
            u64 // both branches break
            u64 // break and normal tail
            u64 // return and break
            () // unit break
            () // unit fallthrough
            () // empty labeled block
        "#]],
    );
}

#[test]
fn routes_labeled_block_breaks_to_their_lexical_targets() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_labeled_block_targets"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it(flag: bool) {
    let outer = 'done: {
        let inner = 'done: { break 'done true; }$0; // ty("shadowed block label")
        let inner_loop = 'done: loop { break 'done 1u8; };
        let loop_value = loop { break 2u16; };
        while flag { break; }
        break 'done 3u64;
    }$0; // ty("outer block result")
    let from_loop = 'done: { loop { break 'done 4u64; } }$0; // ty("block exit through loop")
    let from_nested_label = 'outer: {
        'inner: { break 'outer 5u64; }
    }$0; // ty("block exit through another label")
    let closure = 'done: {
        let f = || 'done: { break 'done true; }$0; // ty("closure block result")
        break 'done 6u64;
    }$0; // ty("block containing closure")
    let asynchronous = 'done: {
        let f = async { 'done: { break 'done true; }$0 }; // ty("async inner block result")
        break 'done 7u64;
    }$0; // ty("block containing async block")
    let raw = 'r#break: { break 'r#break 8u64; }$0; // ty("raw label")
}
"#,
        expect![[r#"
            bool // shadowed block label
            u64 // outer block result
            u64 // block exit through loop
            u64 // block exit through another label
            bool // closure block result
            u64 // block containing closure
            bool // async inner block result
            u64 // block containing async block
            u64 // raw label
        "#]],
    );
}

#[test]
fn propagates_labeled_block_expectations_and_deferred_results() {
    check_analysis_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "analysis_labeled_block_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
impl User {
    pub fn abort(self) -> ! { loop {} }
    pub fn value(self) -> u64 { 1 }
}
pub fn apply<R, F: FnOnce(User) -> R>(f: F) -> R { f(User) }
pub fn consume(value: u64) {}
pub fn make<T>() -> T { loop {} }

pub fn use_it(flag: bool) {
    let annotated: u64 = 'done: { break 'done 1$0; }$1; // ty("expected break literal").in_lib("analysis_labeled_block_inference"), ty("annotated block").in_lib("analysis_labeled_block_inference")
    let later = 'done: { break 'done make()$0; }$1; // ty("generic break payload").in_lib("analysis_labeled_block_inference"), ty("block constrained by later use").in_lib("analysis_labeled_block_inference")
    consume(later);
    let payload = apply((|user| 'done: {
        break 'done user.value()$0; // ty("deferred break payload").in_lib("analysis_labeled_block_inference")
    }))$0; // ty("deferred payload result").in_lib("analysis_labeled_block_inference")
    let diverging_payload = apply((|user| 'done: {
        if flag { break 'done user.abort()$0; } // ty("diverging break payload").in_lib("analysis_labeled_block_inference")
        break 'done 2u64;
    }))$0; // ty("surviving break result").in_lib("analysis_labeled_block_inference")
    let diverging_statement = apply((|user| 'done: {
        if flag { break 'done 3u64; }
        user.abort()$0; // ty("diverging final statement").in_lib("analysis_labeled_block_inference")
    }))$0; // ty("deferred statement result").in_lib("analysis_labeled_block_inference")
    let ordinary_statement = apply((|user| 'done: {
        user.value();
        4u64
    }))$0; // ty("deferred statement with normal tail").in_lib("analysis_labeled_block_inference")
}
"#,
        expect![[r#"
            u64 // expected break literal
            u64 // annotated block
            u64 // generic break payload
            u64 // block constrained by later use
            u64 // deferred break payload
            u64 // deferred payload result
            ! // diverging break payload
            u64 // surviving break result
            ! // diverging final statement
            u64 // deferred statement result
            u64 // deferred statement with normal tail
        "#]],
    );
}

#[test]
fn infers_loop_exit_values() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_loop_exits"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it(flag: bool) {
    let bare = loop { break; };
    bare$0; // ty("bare break")
    let explicit_unit = loop { break (); };
    explicit_unit$0; // ty("explicit unit payload")
    let single = loop { break 1u64; };
    single$0; // ty("single value break")
    let branches = loop {
        if flag { break 2$0; } // ty("literal constrained by another break")
        break 3u64;
    };
    branches$0; // ty("multiple break values")
    let outer = 'done: loop {
        let inner = loop { break true; };
        inner$0; // ty("inner loop result")
        let shadowed_block = 'done: { break 'done 4u8; };
        shadowed_block$0; // ty("block shadowing a loop label")
        let shadowed_loop = 'done: loop { break 'done 5u16; };
        shadowed_loop$0; // ty("loop shadowing a loop label")
        loop {
            if flag { break 'done 6u64; }
            break;
        }
        break 'done 7u64;
    };
    outer$0; // ty("outer loop result")
    let nested_payload = loop { break loop { break 8u32; }; };
    nested_payload$0; // ty("loop used as break payload")
}

pub fn exit_to_block() {
    let value = 'done: {
        let inner = loop { break 'done 9u64; };
        inner$0; // ty("loop exiting to outer block")
    };
    value$0; // ty("block receiving loop exit")
}

pub fn spin() {
    let value = loop {};
    value$0; // ty("loop without exits")
}
"#,
        expect![[r#"
            () // bare break
            () // explicit unit payload
            u64 // single value break
            u64 // literal constrained by another break
            u64 // multiple break values
            bool // inner loop result
            u8 // block shadowing a loop label
            u16 // loop shadowing a loop label
            u64 // outer loop result
            u32 // loop used as break payload
            ! // loop exiting to outer block
            u64 // block receiving loop exit
            ! // loop without exits
        "#]],
    );
}

#[test]
fn propagates_loop_expectations_and_deferred_results() {
    check_analysis(
        r#"
//- /Cargo.toml
[package]
name = "analysis_loop_inference"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Source;
impl Source {
    pub fn value(&self) -> u64 { 1 }
    pub fn abort(&self) -> ! { loop {} }
}
pub fn make<T>() -> T { loop {} }
pub fn consume(_: u64) {}
pub fn require_source(_: &Source) {}

pub fn use_it(flag: bool) {
    let annotated: u64 = loop { break 1$0; }; // ty("expected break literal")
    let later = loop { break make()$0; }; // ty("generic break payload")
    consume(later);
    later$0; // ty("loop constrained by later use")

    let source = make();
    let result = loop {
        if flag { break source.abort()$0; } // ty("deferred diverging payload")
        break source.value()$0; // ty("deferred value payload")
    };
    require_source(&source);
    result$0; // ty("deferred loop result")
}

pub fn all_diverging() {
    let source = make();
    let result = loop { break source.abort()$0; }; // ty("only payload diverges")
    require_source(&source);
    result$0; // ty("all break payloads diverge")
}
"#,
        expect![[r#"
            u64 // expected break literal
            u64 // generic break payload
            u64 // loop constrained by later use
            ! // deferred diverging payload
            u64 // deferred value payload
            u64 // deferred loop result
            ! // only payload diverges
            ! // all break payloads diverge
        "#]],
    );
}
