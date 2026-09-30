use expect_test::expect;

#[test]
fn builtin_derives_preserve_generics_and_projection_requirements() {
    let fixture = crate::testonly::SemanticIrFixture::build_with_fake_sysroot(
        r#"
//- /Cargo.toml
[package]
name = "derive_headers"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub trait Family { type Item; type Other; }

#[derive(Clone)]
pub struct Projected<T: Family> {
    item: T::Item,
    nested: Option<<T as Family>::Item>,
    #[cfg(any())]
    omitted: T::Other,
}

#[derive(::core::clone::Clone)]
pub struct Borrowed<'a, T: ?Sized, const N: usize = 4>
where T: 'a {
    value: &'a T,
    bytes: [u8; N],
}

#[derive(Default)]
pub enum Choice<T> {
    #[cfg(any())]
    #[default]
    Omitted,
    #[default]
    Empty,
    Some(T),
}

mod core {}
#[derive(std::clone::Clone)]
pub struct AbsoluteCore;

use std::clone::Clone as Duplicate;
#[cfg_attr(all(), cfg_attr(all(), derive(Duplicate)))]
pub struct Alias<T = ()>(T);
"#,
    );
    let crate_ref = fixture
        .def_map_fixture()
        .crate_ref("derive_headers", rg_workspace::TargetKind::Lib);
    let store = fixture
        .resident_crate_ir(crate_ref)
        .expect("fixture item store exists");
    let mut headers = Vec::new();
    for (_, data) in store.impls_with_refs() {
        assert!(
            data.resolved_trait_ref.as_option().is_some(),
            "derived trait should resolve through core"
        );
        headers.push(
            format!(
                "{}: {}\n  generics: {}",
                data.self_ty,
                data.trait_ref.as_ref().expect("derived impl has a trait"),
                data.generics,
            )
            .trim_end()
            .to_owned(),
        );
    }
    expect![[r#"
        Projected<T>: ::core::clone::Clone
          generics: <T: Family + ::core::clone::Clone> where T::Item: ::core::clone::Clone
        Borrowed<'a, T, N>: ::core::clone::Clone
          generics: <'a, T: ?Sized + ::core::clone::Clone, const N: usize> where T: 'a
        Choice<T>: ::core::default::Default
          generics: <T>
        AbsoluteCore: ::core::clone::Clone
          generics:
        Alias<T>: ::core::clone::Clone
          generics: <T: ::core::clone::Clone>
    "#]]
    .assert_eq(&format!("{}\n", headers.join("\n")));
}
