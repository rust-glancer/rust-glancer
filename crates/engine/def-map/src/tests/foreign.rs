use expect_test::expect;
use rg_workspace::TargetKind;

use super::utils::{self, PathResolutionQuery};
use crate::{ItemSourceKind, testonly::DefMapFixture};

const SOURCE_FOREIGN_FIXTURE: &str = r#"
//- /Cargo.toml
[package]
name = "foreign_items"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! nested_foreign_macro {
    () => {
        pub struct MustNotExpand;
    };
}

pub mod ffi {
    unsafe extern "C" {
        pub fn foreign_fn(input: u32) -> u64;
        pub static FOREIGN_STATIC: u32;
        pub type Opaque;

        #[cfg(disabled)]
        pub fn child_disabled();

        nested_foreign_macro!();
    }

    #[cfg(disabled)]
    extern "C" {
        pub fn block_disabled();
    }
}

pub use ffi::foreign_fn as reexported_fn;
pub use ffi::FOREIGN_STATIC as REEXPORTED_STATIC;
pub use ffi::Opaque as ReexportedOpaque;
"#;

#[test]
fn foreign_declarations_resolve_import_and_reexport_like_named_items() {
    utils::check_project_path_resolution(
        SOURCE_FOREIGN_FIXTURE,
        &[
            PathResolutionQuery::lib("foreign_items", "crate", "ffi::foreign_fn").values(),
            PathResolutionQuery::lib("foreign_items", "crate", "reexported_fn").values(),
            PathResolutionQuery::lib("foreign_items", "crate", "ffi::FOREIGN_STATIC").values(),
            PathResolutionQuery::lib("foreign_items", "crate", "REEXPORTED_STATIC").values(),
            PathResolutionQuery::lib("foreign_items", "crate", "ffi::Opaque").types(),
            PathResolutionQuery::lib("foreign_items", "crate", "ReexportedOpaque").types(),
            PathResolutionQuery::lib("foreign_items", "crate", "ffi::child_disabled"),
            PathResolutionQuery::lib("foreign_items", "crate", "ffi::block_disabled"),
            PathResolutionQuery::lib("foreign_items", "crate", "ffi::MustNotExpand"),
        ],
        expect![[r#"
            foreign_items [lib] crate resolves ffi::foreign_fn [values] -> fn foreign_items[lib]::crate::ffi::foreign_fn
            foreign_items [lib] crate resolves reexported_fn [values] -> fn foreign_items[lib]::crate::ffi::foreign_fn
            foreign_items [lib] crate resolves ffi::FOREIGN_STATIC [values] -> static foreign_items[lib]::crate::ffi::FOREIGN_STATIC
            foreign_items [lib] crate resolves REEXPORTED_STATIC [values] -> static foreign_items[lib]::crate::ffi::FOREIGN_STATIC
            foreign_items [lib] crate resolves ffi::Opaque [types] -> type_alias foreign_items[lib]::crate::ffi::Opaque
            foreign_items [lib] crate resolves ReexportedOpaque [types] -> type_alias foreign_items[lib]::crate::ffi::Opaque
            foreign_items [lib] crate resolves ffi::child_disabled -> <none> (unresolved at segment #1)
            foreign_items [lib] crate resolves ffi::block_disabled -> <none> (unresolved at segment #1)
            foreign_items [lib] crate resolves ffi::MustNotExpand -> <none> (unresolved at segment #1)
        "#]],
    );

    let fixture = DefMapFixture::build(SOURCE_FOREIGN_FIXTURE);
    let crate_ref = fixture.crate_ref("foreign_items", TargetKind::Lib);
    let def_map = fixture
        .resident_def_map(crate_ref)
        .expect("foreign fixture def map should exist");
    let foreign_defs = def_map
        .local_def_refs()
        .filter_map(|local_def| {
            def_map
                .foreign_block(local_def.local_def)
                .map(|block| (local_def, block))
        })
        .collect::<Vec<_>>();
    assert_eq!(foreign_defs.len(), 3);
    for (local_def, block) in foreign_defs {
        assert!(matches!(block.kind, ItemSourceKind::ItemTree(_)));
        assert!(
            def_map.local_def(local_def.local_def).is_some(),
            "foreign ownership should point at a collected local definition",
        );
    }
}

#[test]
fn macro_generated_extern_blocks_collect_the_same_declaration_kinds() {
    let fixture = r#"
//- /Cargo.toml
[package]
name = "generated_foreign_items"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
macro_rules! make_foreign {
    () => {
        extern "C" {
            pub fn generated_fn(input: u8) -> u16;
            pub static GENERATED_STATIC: u8;
            pub type GeneratedOpaque;
        }
    };
}

make_foreign!();
"#;
    utils::check_project_path_resolution(
        fixture,
        &[
            PathResolutionQuery::lib("generated_foreign_items", "crate", "generated_fn").values(),
            PathResolutionQuery::lib("generated_foreign_items", "crate", "GENERATED_STATIC")
                .values(),
            PathResolutionQuery::lib("generated_foreign_items", "crate", "GeneratedOpaque").types(),
        ],
        expect![[r#"
            generated_foreign_items [lib] crate resolves generated_fn [values] -> fn generated_foreign_items[lib]::crate::generated_fn
            generated_foreign_items [lib] crate resolves GENERATED_STATIC [values] -> static generated_foreign_items[lib]::crate::GENERATED_STATIC
            generated_foreign_items [lib] crate resolves GeneratedOpaque [types] -> type_alias generated_foreign_items[lib]::crate::GeneratedOpaque
        "#]],
    );

    let fixture = DefMapFixture::build(fixture);
    let crate_ref = fixture.crate_ref("generated_foreign_items", TargetKind::Lib);
    let def_map = fixture
        .resident_def_map(crate_ref)
        .expect("generated foreign fixture def map should exist");
    let foreign_blocks = def_map
        .local_def_refs()
        .filter_map(|local_def| def_map.foreign_block(local_def.local_def))
        .collect::<Vec<_>>();
    assert_eq!(foreign_blocks.len(), 3);
    assert!(
        foreign_blocks
            .iter()
            .all(|source| matches!(source.kind, ItemSourceKind::Generated(_))),
        "generated foreign declarations should retain generated block ownership",
    );
}
