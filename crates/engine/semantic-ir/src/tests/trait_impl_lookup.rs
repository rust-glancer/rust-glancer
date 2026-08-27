use rg_ir_model::{Mutability, PrimitiveTy, UnsignedIntTy};

use crate::TraitImplSelfHead;

#[test]
fn trait_impl_lookup_uses_direct_self_heads_and_conservative_fallbacks() {
    let fixture = crate::testonly::SemanticIrFixture::build(
        r#"
//- /Cargo.toml
[package]
name = "trait_head_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub trait Marker {}

pub struct User;
pub type Alias = u32;

pub mod shadowed {
    #[allow(non_camel_case_types)]
    pub struct u8;

    impl super::Marker for u8 {}
}

impl Marker for u16 {}
impl Marker for u8 {}
impl Marker for User {}
impl Marker for () {}
impl Marker for ! {}
impl Marker for (u8, u16) {}
impl Marker for [u8; 4] {}
impl Marker for [u8] {}
impl Marker for &u8 {}
impl Marker for &mut u8 {}
impl Marker for *const u8 {}
impl Marker for *mut u8 {}
impl Marker for fn(u8) {}
impl Marker for fn(u8, u16) {}
impl Marker for Alias {}
impl<T> Marker for T {}
"#,
    );
    let crate_ref = fixture
        .def_map_fixture()
        .crate_ref("trait_head_fixture", rg_workspace::TargetKind::Lib);
    let store = fixture
        .resident_crate_ir(crate_ref)
        .expect("fixture semantic store should exist");
    let marker = store
        .traits_with_refs()
        .find(|(_, data)| data.name.as_str() == "Marker")
        .expect("fixture should contain Marker")
        .0;
    let user = store
        .semantic_items()
        .find_map(|item| {
            (item.name()?.as_str() == "User")
                .then(|| item.type_def())
                .flatten()
        })
        .expect("fixture should contain User");
    let shadowed_u8 = store
        .semantic_items()
        .find_map(|item| {
            (item.name()?.as_str() == "u8")
                .then(|| item.type_def())
                .flatten()
        })
        .expect("fixture should contain the type named like a primitive");

    let def_maps = fixture
        .def_map_db()
        .read_txn(rg_def_map::DefMapLoader::resident_only(
            "resident trait-head fixture",
        ));
    let items = fixture
        .semantic_ir_db()
        .read_txn(crate::SemanticIrLoader::resident_only(
            "resident trait-head fixture",
        ));
    let lookup = crate::ItemLookupQuery::build_from(&crate::CrateItemQuery::new(
        &def_maps, &items, crate_ref,
    ))
    .expect("trait-head lookup query should build");

    let self_types = |head| {
        lookup
            .trait_impl_candidates_for_self_head(marker, head)
            .expect("Marker should be visible")
            .iter()
            .map(|candidate| {
                store
                    .impl_data(candidate.impl_ref.id)
                    .expect("candidate impl should exist")
                    .self_ty
                    .to_string()
            })
            .collect::<Vec<_>>()
    };

    let cases: [(Option<TraitImplSelfHead>, &[&str], &str); 16] = [
        (
            Some(TraitImplSelfHead::Primitive(PrimitiveTy::UnsignedInt(
                UnsignedIntTy::U8,
            ))),
            &["u8", "Alias", "T"],
            "a primitive u8 lookup should include its direct impl and conservative fallbacks",
        ),
        (
            Some(TraitImplSelfHead::Primitive(PrimitiveTy::UnsignedInt(
                UnsignedIntTy::U16,
            ))),
            &["u16", "Alias", "T"],
            "a primitive u16 lookup should not reuse the u8 lane",
        ),
        (
            Some(TraitImplSelfHead::Adt(user)),
            &["User", "Alias", "T"],
            "a nominal lookup should include the matching ADT lane",
        ),
        (
            Some(TraitImplSelfHead::Adt(shadowed_u8)),
            &["u8", "Alias", "T"],
            "an ADT named like a primitive should remain in the nominal lane",
        ),
        (
            Some(TraitImplSelfHead::Tuple(2)),
            &["(u8, u16)", "Alias", "T"],
            "a tuple lookup should use its arity",
        ),
        (
            Some(TraitImplSelfHead::Unit),
            &["()", "Alias", "T"],
            "unit should have a distinct receiver head",
        ),
        (
            Some(TraitImplSelfHead::Never),
            &["!", "Alias", "T"],
            "never should have a distinct receiver head",
        ),
        (
            Some(TraitImplSelfHead::Array),
            &["[u8; 4]", "Alias", "T"],
            "arrays should use the array receiver lane",
        ),
        (
            Some(TraitImplSelfHead::Slice),
            &["[u8]", "Alias", "T"],
            "slices should use the slice receiver lane",
        ),
        (
            Some(TraitImplSelfHead::Reference(Mutability::Shared)),
            &["&u8", "Alias", "T"],
            "shared references should retain their mutability in the key",
        ),
        (
            Some(TraitImplSelfHead::Reference(Mutability::Mutable)),
            &["&mut u8", "Alias", "T"],
            "mutable references should retain their mutability in the key",
        ),
        (
            Some(TraitImplSelfHead::RawPointer(Mutability::Shared)),
            &["*const u8", "Alias", "T"],
            "const raw pointers should retain their mutability in the key",
        ),
        (
            Some(TraitImplSelfHead::RawPointer(Mutability::Mutable)),
            &["*mut u8", "Alias", "T"],
            "mutable raw pointers should retain their mutability in the key",
        ),
        (
            Some(TraitImplSelfHead::FnPointer(1)),
            &["fn(u8)", "Alias", "T"],
            "function pointers should use their parameter arity",
        ),
        (
            Some(TraitImplSelfHead::FnPointer(2)),
            &["fn(u8, u16)", "Alias", "T"],
            "different function-pointer arities should not share a direct lane",
        ),
        (
            None,
            &["Alias", "T"],
            "an absent receiver head should return only conservative fallbacks",
        ),
    ];

    for (head, expected, error_message) in cases {
        let actual = self_types(head);
        let actual = actual.iter().map(String::as_str).collect::<Vec<_>>();
        assert_eq!(actual, expected, "{error_message}");
    }
}
