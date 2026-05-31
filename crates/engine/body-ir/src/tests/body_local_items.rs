use expect_test::expect;
use rg_def_map::{
    ImportBinding, ImportKind, LocalDefKind, ModuleOrigin, Path, PathSegment, ScopeBindingOrigin,
};
use rg_ir_model::{
    DefId, DefMapRef, LocalDefId, ModuleId, ModuleRef, SemanticItemKind,
    hir::source::ItemSourceKind,
};
use rg_text::Name;

use crate::resolution::def_map_lookup::BodyDefMapLookup;

use super::utils::{check_first_body_def_map, check_first_body_item_store, check_project_body_ir};

#[test]
fn resolves_body_local_structs_before_module_structs() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_local_item_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;

pub fn use_it() {
    struct User;
    let local: User = User;
    {
        struct User;
        let nested: User = User;
    }
    let again: User = User;
}
"#,
        expect![[r#"
            package body_local_item_fixture

            body_local_item_fixture [lib]
            body b0 fn body_local_item_fixture[lib]::crate::use_it @ 3:1-11:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v2; items i0
            - s2 parent s1: v1; items i1
            items
            - i0 struct User @ 4:5-4:17
            - i1 struct User @ 7:9-7:21
            bindings
            - v0 let local `local`: User => local nominal struct fn body_local_item_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 5:9-5:14
            - v1 let nested `nested`: User => local nominal struct fn body_local_item_fixture[lib]::crate::use_it::User @ 7:9-7:21 @ 8:13-8:19
            - v2 let again `again`: User => local nominal struct fn body_local_item_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 10:9-10:14
            body
            expr e4 block s1 => () @ 3:17-11:2
              stmt s0 item i0 @ 4:5-4:17
              stmt s1 let v0: User @ 5:5-5:28
                initializer
                  expr e0 path User -> local item struct fn body_local_item_fixture[lib]::crate::use_it::User @ 4:5-4:17 => local nominal struct fn body_local_item_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 5:23-5:27
              stmt s4 expr @ 6:5-9:6
                expr e2 block s2 => () @ 6:5-9:6
                  stmt s2 item i1 @ 7:9-7:21
                  stmt s3 let v1: User @ 8:9-8:33
                    initializer
                      expr e1 path User -> local item struct fn body_local_item_fixture[lib]::crate::use_it::User @ 7:9-7:21 => local nominal struct fn body_local_item_fixture[lib]::crate::use_it::User @ 7:9-7:21 @ 8:28-8:32
              stmt s5 let v2: User @ 10:5-10:28
                initializer
                  expr e3 path User -> local item struct fn body_local_item_fixture[lib]::crate::use_it::User @ 4:5-4:17 => local nominal struct fn body_local_item_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 10:23-10:27
        "#]],
    );
}

// TODO: Temporary test until the codebase is migrated to use defmaps for everything.
#[test]
fn collects_body_local_def_map_items_impls_and_imports() {
    check_first_body_def_map(
        r#"
//- /Cargo.toml
[package]
name = "body_local_def_map_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Root;

pub fn use_it() {
    use crate::Root as Alias;
    struct User;
    impl User {}
    {
        enum Nested {
            Value,
        }
    }
}
"#,
        |def_map| {
            assert!(matches!(def_map.own_ref(), DefMapRef::Body(_)));

            let modules = def_map.modules();
            assert_eq!(modules.len(), 3);
            assert_eq!(modules[0].parent, None);
            assert_eq!(modules[1].parent, Some(ModuleId(0)));
            assert_eq!(modules[2].parent, Some(ModuleId(1)));
            assert!(matches!(modules[0].origin, ModuleOrigin::Synthetic { .. }));
            assert!(matches!(modules[1].origin, ModuleOrigin::Synthetic { .. }));
            assert!(matches!(modules[2].origin, ModuleOrigin::Synthetic { .. }));
            assert_eq!(modules[1].local_defs.len(), 1);
            assert_eq!(modules[1].imports.len(), 1);
            assert_eq!(modules[1].impls.len(), 1);
            assert_eq!(modules[2].local_defs.len(), 1);
            assert!(
                modules
                    .iter()
                    .all(|module| module.unresolved_imports.is_empty())
            );

            let defs = def_map
                .local_defs()
                .iter()
                .map(|def| (def.name.as_str(), def.kind))
                .collect::<Vec<_>>();
            assert_eq!(
                defs,
                vec![
                    ("User", LocalDefKind::Struct),
                    ("Nested", LocalDefKind::Enum)
                ]
            );
            assert!(
                def_map
                    .local_defs()
                    .iter()
                    .all(|def| { matches!(def.source.kind, ItemSourceKind::Body(_)) })
            );
            assert_eq!(def_map.local_impls().len(), 1);
            assert!(matches!(
                def_map.local_impls()[0].source.kind,
                ItemSourceKind::Body(_)
            ));

            let imports = def_map.imports();
            assert_eq!(imports.len(), 1);
            let import = &imports[0];
            assert_eq!(import.kind, ImportKind::Named);
            assert!(matches!(&import.binding, ImportBinding::Explicit(name) if name == "Alias"));
            assert!(matches!(import.source.kind, ItemSourceKind::Body(_)));
            assert_eq!(
                import
                    .path
                    .segments
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
                vec!["crate", "Root"]
            );
        },
    );
}

// TODO: Temporary test until the codebase is migrated to use defmaps for everything.
#[test]
fn body_def_map_scopes_contain_direct_bindings() {
    check_first_body_def_map(
        r#"
//- /Cargo.toml
[package]
name = "body_local_scope_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Root;

pub fn use_it() {
    use crate::Root as Alias;
    struct User;
    fn local_fn() {}
    const LOCAL: usize = 0;
    mod nested {
        pub struct Inside;
    }
}
"#,
        |def_map| {
            let body_origin = def_map.own_ref();
            let modules = def_map.modules();
            assert_eq!(modules.len(), 3);

            let function_module = &modules[1];
            let nested_module = &modules[2];
            assert!(matches!(
                function_module.origin,
                ModuleOrigin::Synthetic { .. }
            ));
            assert!(matches!(nested_module.origin, ModuleOrigin::Inline { .. }));

            let user_entry = function_module
                .scope
                .entry("User")
                .expect("body-local struct should be visible in module scope");
            assert_eq!(user_entry.types().len(), 1);
            assert_eq!(user_entry.types()[0].origin, ScopeBindingOrigin::Direct);
            let DefId::Local(user_def) = user_entry.types()[0].def else {
                panic!("body-local struct should resolve to a local definition");
            };
            assert_eq!(user_def.origin, body_origin);
            assert_eq!(
                def_map
                    .local_def(user_def.local_def)
                    .expect("body-local struct def should exist")
                    .kind,
                LocalDefKind::Struct
            );

            let fn_entry = function_module
                .scope
                .entry("local_fn")
                .expect("body-local function should be visible in module scope");
            assert!(fn_entry.types().is_empty());
            assert_eq!(fn_entry.values().len(), 1);
            assert_eq!(fn_entry.values()[0].origin, ScopeBindingOrigin::Direct);

            let const_entry = function_module
                .scope
                .entry("LOCAL")
                .expect("body-local const should be visible in module scope");
            assert!(const_entry.types().is_empty());
            assert_eq!(const_entry.values().len(), 1);
            assert_eq!(const_entry.values()[0].origin, ScopeBindingOrigin::Direct);

            let nested_entry = function_module
                .scope
                .entry("nested")
                .expect("body-local module should be visible in parent module scope");
            assert_eq!(nested_entry.types().len(), 1);
            assert_eq!(nested_entry.types()[0].origin, ScopeBindingOrigin::Direct);
            assert_eq!(
                nested_entry.types()[0].def,
                DefId::Module(rg_ir_model::ModuleRef {
                    origin: body_origin,
                    module: ModuleId(2),
                })
            );

            let inside_entry = nested_module
                .scope
                .entry("Inside")
                .expect("inline body-local module items should be visible in child module scope");
            assert_eq!(inside_entry.types().len(), 1);
            let DefId::Local(inside_def) = inside_entry.types()[0].def else {
                panic!("nested body-local struct should resolve to a local definition");
            };
            assert_eq!(inside_def.origin, body_origin);

            assert!(
                function_module.scope.entry("Alias").is_none(),
                "body imports are recorded but not resolved into scopes yet"
            );
        },
    );
}

// TODO: Either remove or rework this test after resolution is migrated.
// If kept, should be snapshot-driven.
#[test]
fn body_def_map_lookup_resolves_lexical_scope_names() {
    check_first_body_def_map(
        r#"
//- /Cargo.toml
[package]
name = "body_lookup_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Root;

pub fn use_it() {
    use crate::Root as Alias;
    struct ParentOnly;
    struct Shadow;
    fn value_name() {}
    {
        struct Inner;
        struct Shadow;
    }
}
"#,
        |def_map| {
            let lookup = BodyDefMapLookup::new(def_map);
            let origin = def_map.own_ref();
            let outer = ModuleRef {
                origin,
                module: ModuleId(1),
            };
            let inner = ModuleRef {
                origin,
                module: ModuleId(2),
            };

            let inner_result =
                lookup.resolve_path_in_type_namespace(inner, &relative_path(&["Inner"]));
            assert_eq!(inner_result.unresolved_at, None);
            assert_eq!(
                local_def_module(def_map, inner_result.resolved[0]),
                ModuleId(2)
            );

            let parent_result =
                lookup.resolve_path_in_type_namespace(inner, &relative_path(&["ParentOnly"]));
            assert_eq!(parent_result.unresolved_at, None);
            assert_eq!(
                local_def_name(def_map, parent_result.resolved[0]),
                "ParentOnly"
            );
            assert_eq!(
                local_def_module(def_map, parent_result.resolved[0]),
                ModuleId(1)
            );

            let shadow_result =
                lookup.resolve_path_in_type_namespace(inner, &relative_path(&["Shadow"]));
            assert_eq!(shadow_result.unresolved_at, None);
            assert_eq!(
                local_def_module(def_map, shadow_result.resolved[0]),
                ModuleId(2)
            );

            let type_value_result =
                lookup.resolve_path_in_type_namespace(inner, &relative_path(&["value_name"]));
            assert_eq!(type_value_result.unresolved_at, Some(0));
            assert!(type_value_result.resolved.is_empty());

            let value_result = lookup.resolve_path(inner, &relative_path(&["value_name"]));
            assert_eq!(value_result.unresolved_at, None);
            assert_eq!(
                local_def_name(def_map, value_result.resolved[0]),
                "value_name"
            );
            assert_eq!(
                local_def_module(def_map, value_result.resolved[0]),
                ModuleId(1)
            );

            let import_result =
                lookup.resolve_path_in_type_namespace(outer, &relative_path(&["Alias"]));
            assert_eq!(import_result.unresolved_at, Some(0));
            assert!(import_result.resolved.is_empty());

            let mut absolute_path = relative_path(&["Root"]);
            absolute_path.absolute = true;
            let absolute_result = lookup.resolve_path_in_type_namespace(inner, &absolute_path);
            assert_eq!(absolute_result.unresolved_at, Some(0));
            assert!(absolute_result.resolved.is_empty());
        },
    );
}

// TODO: Either remove or rework this test after resolution is migrated.
// If kept, should be snapshot-driven.
#[test]
fn body_def_map_lookup_resolves_named_body_modules() {
    check_first_body_def_map(
        r#"
//- /Cargo.toml
[package]
name = "body_module_lookup_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it() {
    mod nested {
        pub struct Inside;
        struct Private;
    }
}
"#,
        |def_map| {
            let lookup = BodyDefMapLookup::new(def_map);
            let origin = def_map.own_ref();
            let function_module = ModuleRef {
                origin,
                module: ModuleId(1),
            };

            let module_result =
                lookup.resolve_path_in_type_namespace(function_module, &relative_path(&["nested"]));
            assert_eq!(module_result.unresolved_at, None);
            assert_eq!(
                module_result.resolved,
                vec![DefId::Module(ModuleRef {
                    origin,
                    module: ModuleId(2),
                })],
            );

            let inside_result = lookup.resolve_path_in_type_namespace(
                function_module,
                &relative_path(&["nested", "Inside"]),
            );
            assert_eq!(inside_result.unresolved_at, None);
            assert_eq!(local_def_name(def_map, inside_result.resolved[0]), "Inside");
            assert_eq!(
                local_def_module(def_map, inside_result.resolved[0]),
                ModuleId(2)
            );

            let private_result = lookup.resolve_path_in_type_namespace(
                function_module,
                &relative_path(&["nested", "Private"]),
            );
            assert_eq!(private_result.unresolved_at, Some(1));
            assert!(private_result.resolved.is_empty());
        },
    );
}

fn relative_path(segments: &[&str]) -> Path {
    Path {
        absolute: false,
        segments: segments
            .iter()
            .map(|segment| PathSegment::Name(Name::new(segment)))
            .collect(),
    }
}

fn local_def_name(def_map: &rg_def_map::DefMap, def: DefId) -> &str {
    let DefId::Local(local_def) = def else {
        panic!("resolved def should be a local definition");
    };
    def_map
        .local_def(local_def.local_def)
        .expect("resolved local definition should exist")
        .name
        .as_str()
}

fn local_def_module(def_map: &rg_def_map::DefMap, def: DefId) -> ModuleId {
    let DefId::Local(local_def) = def else {
        panic!("resolved def should be a local definition");
    };
    def_map
        .local_def(local_def.local_def)
        .expect("resolved local definition should exist")
        .module
}

// TODO: Temporary test until the codebase is migrated to use item stores for body items.
#[test]
fn collects_body_local_item_store_items_impls_and_assoc_items() {
    check_first_body_item_store(
        r#"
//- /Cargo.toml
[package]
name = "body_local_item_store_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub fn use_it() {
    struct User {
        id: usize,
    }
    enum Choice {
        Start,
    }
    type Alias = User;
    const COUNT: usize = 1;
    static VALUE: usize = 2;
    fn helper() {}

    trait LocalTrait {
        type Assoc;
        const FLAG: bool;
        fn run(&self);
    }

    impl User {
        type Output = User;
        const ZERO: usize = 0;
        fn new() -> Self {
            User { id: 0 }
        }
    }
}
"#,
        |items| {
            assert!(matches!(items.origin(), DefMapRef::Body(_)));
            assert_eq!(items.structs().len(), 1);
            assert_eq!(items.unions().len(), 0);
            assert_eq!(items.enums().len(), 1);
            assert_eq!(items.traits().len(), 1);
            assert_eq!(items.impls().len(), 1);
            assert_eq!(items.functions().len(), 3);
            assert_eq!(items.type_aliases().len(), 3);
            assert_eq!(items.consts().len(), 3);
            assert_eq!(items.statics().len(), 1);

            for local_def_idx in 0..7 {
                assert!(
                    items
                        .item_for_local_def(LocalDefId(local_def_idx))
                        .is_some(),
                    "body local def {local_def_idx} should map to a semantic item",
                );
            }

            let kinds = items
                .semantic_items()
                .map(|item| item.kind())
                .collect::<Vec<_>>();
            assert_eq!(
                kinds,
                vec![
                    SemanticItemKind::Struct,
                    SemanticItemKind::Enum,
                    SemanticItemKind::Trait,
                    SemanticItemKind::Impl,
                    SemanticItemKind::Function,
                    SemanticItemKind::Function,
                    SemanticItemKind::Function,
                    SemanticItemKind::TypeAlias,
                    SemanticItemKind::TypeAlias,
                    SemanticItemKind::TypeAlias,
                    SemanticItemKind::Const,
                    SemanticItemKind::Const,
                    SemanticItemKind::Const,
                    SemanticItemKind::Static,
                ]
            );
            assert!(
                items
                    .semantic_items()
                    .all(|item| matches!(item.source().kind, ItemSourceKind::Body(_)))
            );

            let trait_data = items
                .traits()
                .iter()
                .next()
                .expect("fixture should lower one body-local trait");
            assert_eq!(trait_data.items.len(), 3);

            let impl_data = items
                .impls()
                .iter()
                .next()
                .expect("fixture should lower one body-local impl");
            assert_eq!(impl_data.items.len(), 3);
            assert!(impl_data.resolved_self_tys.is_empty());
            assert!(impl_data.resolved_trait_refs.is_empty());
        },
    );
}

#[test]
fn resolves_body_local_struct_fields() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_local_field_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    struct User {
        id: GlobalId,
        pair: Pair,
    }
    struct Pair(GlobalId, GlobalId);

    let user: User;
    let id = user.id;
    let right = user.pair.1;
}
"#,
        expect![[r#"
            package body_local_field_fixture

            body_local_field_fixture [lib]
            body b0 fn body_local_field_fixture[lib]::crate::use_it @ 3:1-13:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v1, v2; items i0, i1
            items
            - i0 struct User @ 4:5-7:6
            - i1 struct Pair @ 8:5-8:37
            bindings
            - v0 let user `user`: User => local nominal struct fn body_local_field_fixture[lib]::crate::use_it::User @ 4:5-7:6 @ 10:9-10:13
            - v1 let id `id` => nominal struct body_local_field_fixture[lib]::crate::GlobalId @ 11:9-11:11
            - v2 let right `right` => nominal struct body_local_field_fixture[lib]::crate::GlobalId @ 12:9-12:14
            body
            expr e5 block s1 => () @ 3:17-13:2
              stmt s0 item i0 @ 4:5-7:6
              stmt s1 item i1 @ 8:5-8:37
              stmt s2 let v0: User @ 10:5-10:20
              stmt s3 let v1 @ 11:5-11:22
                initializer
                  expr e1 field id -> field struct fn body_local_field_fixture[lib]::crate::use_it::User @ 4:5-7:6::id => nominal struct body_local_field_fixture[lib]::crate::GlobalId @ 11:14-11:21
                    base
                      expr e0 path user -> local v0 => local nominal struct fn body_local_field_fixture[lib]::crate::use_it::User @ 4:5-7:6 @ 11:14-11:18
              stmt s4 let v2 @ 12:5-12:29
                initializer
                  expr e4 field 1 -> field struct fn body_local_field_fixture[lib]::crate::use_it::Pair @ 8:5-8:37::#1 => nominal struct body_local_field_fixture[lib]::crate::GlobalId @ 12:17-12:28
                    base
                      expr e3 field pair -> field struct fn body_local_field_fixture[lib]::crate::use_it::User @ 4:5-7:6::pair => local nominal struct fn body_local_field_fixture[lib]::crate::use_it::Pair @ 8:5-8:37 @ 12:17-12:26
                        base
                          expr e2 path user -> local v0 => local nominal struct fn body_local_field_fixture[lib]::crate::use_it::User @ 4:5-7:6 @ 12:17-12:21
        "#]],
    );
}

#[test]
fn resolves_body_local_record_literals() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_local_record_literal_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    struct User {
        id: GlobalId,
    }
    union Bits {
        id: GlobalId,
    }

    let user = User { id: GlobalId };
    let bits = Bits { id: GlobalId };
}
"#,
        expect![[r#"
            package body_local_record_literal_fixture

            body_local_record_literal_fixture [lib]
            body b0 fn body_local_record_literal_fixture[lib]::crate::use_it @ 3:1-13:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v1; items i0, i1
            items
            - i0 struct User @ 4:5-6:6
            - i1 union Bits @ 7:5-9:6
            bindings
            - v0 let user `user` => local nominal struct fn body_local_record_literal_fixture[lib]::crate::use_it::User @ 4:5-6:6 @ 11:9-11:13
            - v1 let bits `bits` => local nominal union fn body_local_record_literal_fixture[lib]::crate::use_it::Bits @ 7:5-9:6 @ 12:9-12:13
            body
            expr e4 block s1 => () @ 3:17-13:2
              stmt s0 item i0 @ 4:5-6:6
              stmt s1 item i1 @ 7:5-9:6
              stmt s2 let v0 @ 11:5-11:38
                initializer
                  expr e1 record User -> local item struct fn body_local_record_literal_fixture[lib]::crate::use_it::User @ 4:5-6:6 => local nominal struct fn body_local_record_literal_fixture[lib]::crate::use_it::User @ 4:5-6:6 @ 11:16-11:37
                    field id
                      expr e0 path GlobalId -> item struct body_local_record_literal_fixture[lib]::crate::GlobalId => nominal struct body_local_record_literal_fixture[lib]::crate::GlobalId @ 11:27-11:35
              stmt s3 let v1 @ 12:5-12:38
                initializer
                  expr e3 record Bits -> local item union fn body_local_record_literal_fixture[lib]::crate::use_it::Bits @ 7:5-9:6 => local nominal union fn body_local_record_literal_fixture[lib]::crate::use_it::Bits @ 7:5-9:6 @ 12:16-12:37
                    field id
                      expr e2 path GlobalId -> item struct body_local_record_literal_fixture[lib]::crate::GlobalId => nominal struct body_local_record_literal_fixture[lib]::crate::GlobalId @ 12:27-12:35
        "#]],
    );
}

#[test]
fn substitutes_generic_body_local_type_alias_arguments() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_local_generic_alias_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Slot<T> {
    value: T,
}

pub fn use_it() {
    struct User;
    type Alias<T> = Slot<T>;

    let slot: Alias<User>;
    let value = slot.value;
}
"#,
        expect![[r#"
            package body_local_generic_alias_fixture

            body_local_generic_alias_fixture [lib]
            body b0 fn body_local_generic_alias_fixture[lib]::crate::use_it @ 5:1-11:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v1; items i0, i1
            items
            - i0 struct User @ 6:5-6:17
            - i1 type Alias @ 7:5-7:29
            bindings
            - v0 let slot `slot`: Alias<User> => nominal struct body_local_generic_alias_fixture[lib]::crate::Slot<local nominal struct fn body_local_generic_alias_fixture[lib]::crate::use_it::User @ 6:5-6:17> @ 9:9-9:13
            - v1 let value `value` => local nominal struct fn body_local_generic_alias_fixture[lib]::crate::use_it::User @ 6:5-6:17 @ 10:9-10:14
            body
            expr e2 block s1 => () @ 5:17-11:2
              stmt s0 item i0 @ 6:5-6:17
              stmt s1 item i1 @ 7:5-7:29
              stmt s2 let v0: Alias<User> @ 9:5-9:27
              stmt s3 let v1 @ 10:5-10:28
                initializer
                  expr e1 field value -> field struct body_local_generic_alias_fixture[lib]::crate::Slot::value => local nominal struct fn body_local_generic_alias_fixture[lib]::crate::use_it::User @ 6:5-6:17 @ 10:17-10:27
                    base
                      expr e0 path slot -> local v0 => nominal struct body_local_generic_alias_fixture[lib]::crate::Slot<local nominal struct fn body_local_generic_alias_fixture[lib]::crate::use_it::User @ 6:5-6:17> @ 10:17-10:21
        "#]],
    );
}

#[test]
fn resolves_body_local_impl_methods() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_local_impl_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    struct User;

    impl User {
        fn id(&self) -> GlobalId {
            missing()
        }

        fn again(&self) -> Self {
            missing()
        }

        fn associated() -> GlobalId {
            missing()
        }
    }

    let user: User;
    let id = user.id();
    let again = user.again();
}
"#,
        expect![[r#"
            package body_local_impl_fixture

            body_local_impl_fixture [lib]
            body b0 fn body_local_impl_fixture[lib]::crate::use_it @ 3:1-23:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v1, v2; items i0; impls m0
            items
            - i0 struct User @ 4:5-4:17
            impls
            - m0 impl User => struct fn body_local_impl_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 6:5-18:6
              - f0 fn id(&self) -> GlobalId
              - f1 fn again(&self) -> Self
              - f2 fn associated() -> GlobalId
            bindings
            - v0 let user `user`: User => local nominal struct fn body_local_impl_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 20:9-20:13
            - v1 let id `id` => nominal struct body_local_impl_fixture[lib]::crate::GlobalId @ 21:9-21:11
            - v2 let again `again` => local nominal struct fn body_local_impl_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 22:9-22:14
            body
            expr e4 block s1 => () @ 3:17-23:2
              stmt s0 item i0 @ 4:5-4:17
              stmt s1 impl m0 @ 6:5-18:6
              stmt s2 let v0: User @ 20:5-20:20
              stmt s3 let v1 @ 21:5-21:24
                initializer
                  expr e1 method_call id -> fn id => nominal struct body_local_impl_fixture[lib]::crate::GlobalId @ 21:14-21:23
                    receiver
                      expr e0 path user -> local v0 => local nominal struct fn body_local_impl_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 21:14-21:18
              stmt s4 let v2 @ 22:5-22:30
                initializer
                  expr e3 method_call again -> fn again => local nominal struct fn body_local_impl_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 22:17-22:29
                    receiver
                      expr e2 path user -> local v0 => local nominal struct fn body_local_impl_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 22:17-22:21
        "#]],
    );
}

#[test]
fn lowers_more_body_local_item_kinds() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_more_local_items_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    enum Action {
        Start(GlobalId),
        Stop,
    }
    union Bits {
        id: GlobalId,
    }
    type Alias = GlobalId;
    trait Named {}
    const DEFAULT: Alias = GlobalId;
    static mut CURRENT: GlobalId = GlobalId;
    fn helper() -> Alias {
        GlobalId
    }

    impl Action {
        const NAME: Alias = GlobalId;
        type Output = Alias;
        fn build() -> Alias {
            helper()
        }
    }

    let alias: Alias = helper();
    let default = DEFAULT;
    let current = CURRENT;
    let action = Action::Start(GlobalId);
}
"#,
        expect![[r#"
            package body_more_local_items_fixture

            body_more_local_items_fixture [lib]
            body b0 fn body_more_local_items_fixture[lib]::crate::use_it @ 3:1-31:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v1, v2, v3; items i0, i1, i2, i3; values c0, c1; functions f0; impls m0
            items
            - i0 enum Action @ 4:5-7:6
            - i1 union Bits @ 8:5-10:6
            - i2 type Alias @ 11:5-11:27
            - i3 trait Named @ 12:5-12:19
            - i4 type Output @ 21:9-21:29
            value_items
            - c0 const DEFAULT: Alias @ 13:5-13:37
            - c1 static CURRENT: GlobalId @ 14:5-14:45
            - c2 const NAME: Alias @ 20:9-20:38
            functions
              - f0 fn helper() -> Alias
            impls
            - m0 impl Action => enum fn body_more_local_items_fixture[lib]::crate::use_it::Action @ 4:5-7:6 @ 19:5-25:6
              - f1 fn build() -> Alias
              - c2 const NAME
              - i4 type Output
            bindings
            - v0 let alias `alias`: Alias => nominal struct body_more_local_items_fixture[lib]::crate::GlobalId @ 27:9-27:14
            - v1 let default `default` => nominal struct body_more_local_items_fixture[lib]::crate::GlobalId @ 28:9-28:16
            - v2 let current `current` => nominal struct body_more_local_items_fixture[lib]::crate::GlobalId @ 29:9-29:16
            - v3 let action `action` => local nominal enum fn body_more_local_items_fixture[lib]::crate::use_it::Action @ 4:5-7:6 @ 30:9-30:15
            body
            expr e7 block s1 => () @ 3:17-31:2
              stmt s0 item i0 @ 4:5-7:6
              stmt s1 item i1 @ 8:5-10:6
              stmt s2 item i2 @ 11:5-11:27
              stmt s3 item i3 @ 12:5-12:19
              stmt s4 value_item c0 @ 13:5-13:37
              stmt s5 value_item c1 @ 14:5-14:45
              stmt s6 function f0 @ 15:5-17:6
              stmt s7 impl m0 @ 19:5-25:6
              stmt s8 let v0: Alias @ 27:5-27:33
                initializer
                  expr e1 call => nominal struct body_more_local_items_fixture[lib]::crate::GlobalId @ 27:24-27:32
                    callee
                      expr e0 path helper -> fn helper => <unknown> @ 27:24-27:30
              stmt s9 let v1 @ 28:5-28:27
                initializer
                  expr e2 path DEFAULT -> local value const fn body_more_local_items_fixture[lib]::crate::use_it::DEFAULT @ 13:5-13:37 => nominal struct body_more_local_items_fixture[lib]::crate::GlobalId @ 28:19-28:26
              stmt s10 let v2 @ 29:5-29:27
                initializer
                  expr e3 path CURRENT -> local value static fn body_more_local_items_fixture[lib]::crate::use_it::CURRENT @ 14:5-14:45 => nominal struct body_more_local_items_fixture[lib]::crate::GlobalId @ 29:19-29:26
              stmt s11 let v3 @ 30:5-30:42
                initializer
                  expr e6 call => local nominal enum fn body_more_local_items_fixture[lib]::crate::use_it::Action @ 4:5-7:6 @ 30:18-30:41
                    callee
                      expr e4 path Action::Start -> variant enum fn body_more_local_items_fixture[lib]::crate::use_it::Action @ 4:5-7:6::Start => local nominal enum fn body_more_local_items_fixture[lib]::crate::use_it::Action @ 4:5-7:6 @ 30:18-30:31
                    arg
                      expr e5 path GlobalId -> item struct body_more_local_items_fixture[lib]::crate::GlobalId => nominal struct body_more_local_items_fixture[lib]::crate::GlobalId @ 30:32-30:40
        "#]],
    );
}

#[test]
fn resolves_body_local_values_by_scope_before_category() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_local_value_shadowing_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct Outer;
pub struct Inner;

pub fn use_it() {
    fn helper() -> Outer {
        Outer
    }
    let value = Outer;

    {
        fn value() -> Inner {
            Inner
        }
        let from_fn = value();
    };

    {
        const helper: Inner = Inner;
        let from_const = helper;
    };
}
"#,
        expect![[r#"
            package body_local_value_shadowing_fixture

            body_local_value_shadowing_fixture [lib]
            body b0 fn body_local_value_shadowing_fixture[lib]::crate::use_it @ 4:1-21:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0; functions f0
            - s2 parent s1: v1; functions f1
            - s3 parent s1: v2; values c0
            value_items
            - c0 const helper: Inner @ 18:9-18:37
            functions
              - f0 fn helper() -> Outer
              - f1 fn value() -> Inner
            bindings
            - v0 let value `value` => nominal struct body_local_value_shadowing_fixture[lib]::crate::Outer @ 8:9-8:14
            - v1 let from_fn `from_fn` => nominal struct body_local_value_shadowing_fixture[lib]::crate::Inner @ 14:13-14:20
            - v2 let from_const `from_const` => nominal struct body_local_value_shadowing_fixture[lib]::crate::Inner @ 19:13-19:23
            body
            expr e6 block s1 => () @ 4:17-21:2
              stmt s0 function f0 @ 5:5-7:6
              stmt s1 let v0 @ 8:5-8:23
                initializer
                  expr e0 path Outer -> item struct body_local_value_shadowing_fixture[lib]::crate::Outer => nominal struct body_local_value_shadowing_fixture[lib]::crate::Outer @ 8:17-8:22
              stmt s4 expr; @ 10:5-15:7
                expr e3 block s2 => () @ 10:5-15:6
                  stmt s2 function f1 @ 11:9-13:10
                  stmt s3 let v1 @ 14:9-14:31
                    initializer
                      expr e2 call => nominal struct body_local_value_shadowing_fixture[lib]::crate::Inner @ 14:23-14:30
                        callee
                          expr e1 path value -> fn value => <unknown> @ 14:23-14:28
              stmt s7 expr; @ 17:5-20:7
                expr e5 block s3 => () @ 17:5-20:6
                  stmt s5 value_item c0 @ 18:9-18:37
                  stmt s6 let v2 @ 19:9-19:33
                    initializer
                      expr e4 path helper -> local value const fn body_local_value_shadowing_fixture[lib]::crate::use_it::helper @ 18:9-18:37 => nominal struct body_local_value_shadowing_fixture[lib]::crate::Inner @ 19:26-19:32
        "#]],
    );
}

#[test]
fn resolves_body_local_associated_consts_and_types() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_local_assoc_items_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct GlobalId;

pub fn use_it() {
    struct User;

    impl User {
        const DEFAULT: GlobalId = GlobalId;
        type Id = GlobalId;
    }

    let default = User::DEFAULT;
    let typed: User::Id = GlobalId;
}
"#,
        expect![[r#"
            package body_local_assoc_items_fixture

            body_local_assoc_items_fixture [lib]
            body b0 fn body_local_assoc_items_fixture[lib]::crate::use_it @ 3:1-13:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v1; items i0; impls m0
            items
            - i0 struct User @ 4:5-4:17
            - i1 type Id @ 8:9-8:28
            value_items
            - c0 const DEFAULT: GlobalId @ 7:9-7:44
            impls
            - m0 impl User => struct fn body_local_assoc_items_fixture[lib]::crate::use_it::User @ 4:5-4:17 @ 6:5-9:6
              - c0 const DEFAULT
              - i1 type Id
            bindings
            - v0 let default `default` => nominal struct body_local_assoc_items_fixture[lib]::crate::GlobalId @ 11:9-11:16
            - v1 let typed `typed`: User::Id => nominal struct body_local_assoc_items_fixture[lib]::crate::GlobalId @ 12:9-12:14
            body
            expr e2 block s1 => () @ 3:17-13:2
              stmt s0 item i0 @ 4:5-4:17
              stmt s1 impl m0 @ 6:5-9:6
              stmt s2 let v0 @ 11:5-11:33
                initializer
                  expr e0 path User::DEFAULT -> local value const fn body_local_assoc_items_fixture[lib]::crate::use_it::DEFAULT @ 7:9-7:44 => nominal struct body_local_assoc_items_fixture[lib]::crate::GlobalId @ 11:19-11:32
              stmt s3 let v1: User::Id @ 12:5-12:36
                initializer
                  expr e1 path GlobalId -> item struct body_local_assoc_items_fixture[lib]::crate::GlobalId => nominal struct body_local_assoc_items_fixture[lib]::crate::GlobalId @ 12:27-12:35
        "#]],
    );
}

#[test]
fn propagates_body_local_enum_pattern_payload_types() {
    check_project_body_ir(
        r#"
//- /Cargo.toml
[package]
name = "body_local_enum_pattern_fixture"
version = "0.1.0"
edition = "2024"

//- /src/lib.rs
pub struct User;
pub struct GlobalId;

pub fn use_it() {
    enum Action {
        Start(User),
        Named { id: GlobalId },
    }

    let action: Action = Action::Start(User);
    let Action::Start(user) = action;
    let named: Action = Action::Start(User);
    let Action::Named { id } = named;
}
"#,
        expect![[r#"
            package body_local_enum_pattern_fixture

            body_local_enum_pattern_fixture [lib]
            body b0 fn body_local_enum_pattern_fixture[lib]::crate::use_it @ 4:1-14:2
            scopes
            - s0 parent <none>: <none>
            - s1 parent s0: v0, v1, v2, v3; items i0
            items
            - i0 enum Action @ 5:5-8:6
            bindings
            - v0 let action `action`: Action => local nominal enum fn body_local_enum_pattern_fixture[lib]::crate::use_it::Action @ 5:5-8:6 @ 10:9-10:15
            - v1 let user `user` => nominal struct body_local_enum_pattern_fixture[lib]::crate::User @ 11:23-11:27
            - v2 let named `named`: Action => local nominal enum fn body_local_enum_pattern_fixture[lib]::crate::use_it::Action @ 5:5-8:6 @ 12:9-12:14
            - v3 let id `id` => nominal struct body_local_enum_pattern_fixture[lib]::crate::GlobalId @ 13:25-13:27
            body
            expr e8 block s1 => () @ 4:17-14:2
              stmt s0 item i0 @ 5:5-8:6
              stmt s1 let v0: Action @ 10:5-10:46
                initializer
                  expr e2 call => local nominal enum fn body_local_enum_pattern_fixture[lib]::crate::use_it::Action @ 5:5-8:6 @ 10:26-10:45
                    callee
                      expr e0 path Action::Start -> variant enum fn body_local_enum_pattern_fixture[lib]::crate::use_it::Action @ 5:5-8:6::Start => local nominal enum fn body_local_enum_pattern_fixture[lib]::crate::use_it::Action @ 5:5-8:6 @ 10:26-10:39
                    arg
                      expr e1 path User -> item struct body_local_enum_pattern_fixture[lib]::crate::User => nominal struct body_local_enum_pattern_fixture[lib]::crate::User @ 10:40-10:44
              stmt s2 let v1 @ 11:5-11:38
                initializer
                  expr e3 path action -> local v0 => local nominal enum fn body_local_enum_pattern_fixture[lib]::crate::use_it::Action @ 5:5-8:6 @ 11:31-11:37
              stmt s3 let v2: Action @ 12:5-12:45
                initializer
                  expr e6 call => local nominal enum fn body_local_enum_pattern_fixture[lib]::crate::use_it::Action @ 5:5-8:6 @ 12:25-12:44
                    callee
                      expr e4 path Action::Start -> variant enum fn body_local_enum_pattern_fixture[lib]::crate::use_it::Action @ 5:5-8:6::Start => local nominal enum fn body_local_enum_pattern_fixture[lib]::crate::use_it::Action @ 5:5-8:6 @ 12:25-12:38
                    arg
                      expr e5 path User -> item struct body_local_enum_pattern_fixture[lib]::crate::User => nominal struct body_local_enum_pattern_fixture[lib]::crate::User @ 12:39-12:43
              stmt s4 let v3 @ 13:5-13:38
                initializer
                  expr e7 path named -> local v2 => local nominal enum fn body_local_enum_pattern_fixture[lib]::crate::use_it::Action @ 5:5-8:6 @ 13:32-13:37
        "#]],
    );
}
