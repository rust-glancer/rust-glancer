use std::fmt::Write as _;

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

    let items = fixture
        .semantic_ir_db()
        .read_txn(crate::SemanticIrLoader::resident_only(
            "resident trait-head fixture",
        ));
    let lookup = crate::ItemLookupQuery::build_from(
        &crate::ItemStoreQuery::new(&items),
        crate_ref,
        &rg_std::CancellationToken::new(),
    )
    .expect("trait-head lookup query should build");

    let self_types = |head| {
        lookup
            .trait_impl_candidates_for_self_head(marker, head)
            .expect("candidate lookup succeeds")
            .expect("Marker should be visible")
            .iter()
            .map(|candidate| {
                let data = store
                    .impl_data(candidate.impl_ref.id)
                    .expect("candidate impl should exist");
                // The primitive and shadowing struct have the same source spelling. Qualify the
                // struct so routing to the wrong impl cannot produce an identical snapshot row.
                if data.resolved_self_ty.as_option() == Some(&shadowed_u8) {
                    format!("shadowed::{}", data.self_ty)
                } else {
                    data.self_ty.to_string()
                }
            })
            .collect::<Vec<_>>()
    };

    // Query names describe routing keys: structural heads do not carry element types or lengths.
    let heads = [
        (
            "primitive u8",
            Some(TraitImplSelfHead::Primitive(PrimitiveTy::UnsignedInt(
                UnsignedIntTy::U8,
            ))),
        ),
        (
            "primitive u16",
            Some(TraitImplSelfHead::Primitive(PrimitiveTy::UnsignedInt(
                UnsignedIntTy::U16,
            ))),
        ),
        ("nominal User", Some(TraitImplSelfHead::Adt(user))),
        (
            "nominal shadowed::u8",
            Some(TraitImplSelfHead::Adt(shadowed_u8)),
        ),
        ("tuple arity 2", Some(TraitImplSelfHead::Tuple(2))),
        ("unit", Some(TraitImplSelfHead::Unit)),
        ("never", Some(TraitImplSelfHead::Never)),
        ("array", Some(TraitImplSelfHead::Array)),
        ("slice", Some(TraitImplSelfHead::Slice)),
        (
            "shared reference",
            Some(TraitImplSelfHead::Reference(Mutability::Shared)),
        ),
        (
            "mutable reference",
            Some(TraitImplSelfHead::Reference(Mutability::Mutable)),
        ),
        (
            "const pointer",
            Some(TraitImplSelfHead::RawPointer(Mutability::Shared)),
        ),
        (
            "mutable pointer",
            Some(TraitImplSelfHead::RawPointer(Mutability::Mutable)),
        ),
        ("fn arity 1", Some(TraitImplSelfHead::FnPointer(1))),
        ("fn arity 2", Some(TraitImplSelfHead::FnPointer(2))),
        ("no head", None),
    ];

    // Keep candidate order visible, including the alias and blanket impl fallbacks after each
    // direct match. A missing head should return only those fallbacks.
    let mut dump = String::new();
    for (name, head) in heads {
        writeln!(dump, "{name} -> [{}]", self_types(head).join(", "))
            .expect("string writes should not fail");
    }
    expect_test::expect![[r#"
        primitive u8 -> [u8, Alias, T]
        primitive u16 -> [u16, Alias, T]
        nominal User -> [User, Alias, T]
        nominal shadowed::u8 -> [shadowed::u8, Alias, T]
        tuple arity 2 -> [(u8, u16), Alias, T]
        unit -> [(), Alias, T]
        never -> [!, Alias, T]
        array -> [[u8; 4], Alias, T]
        slice -> [[u8], Alias, T]
        shared reference -> [&u8, Alias, T]
        mutable reference -> [&mut u8, Alias, T]
        const pointer -> [*const u8, Alias, T]
        mutable pointer -> [*mut u8, Alias, T]
        fn arity 1 -> [fn(u8), Alias, T]
        fn arity 2 -> [fn(u8, u16), Alias, T]
        no head -> [Alias, T]
    "#]]
    .assert_eq(&dump);
}
