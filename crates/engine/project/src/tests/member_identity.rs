use std::fs;

use rg_analysis::ReferenceQuery;
use rg_body_ir::{BodyReader, CurrentSourceSelection};
use rg_ir_model::{
    DefMapRef, FunctionId, ImplRef, ItemStoreId, PrimitiveTy, SemanticItemRef, Span, TraitDefRef,
    UnsignedIntTy,
};
use rg_item_tree::LangItem;
use rg_semantic_ir::{GenericsQuery, ItemLookupQuery, ItemReader, ItemStoreQuery};
use rg_std::CancellationToken;

use crate::{Project, testonly::ProjectSourceFixture};

#[test]
fn member_identity_survives_access_order_editor_bodies_and_restart() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:member_identity
pub struct Left;
pub struct Right;
impl Left {
    pub const LIMIT: usize = 2;
    pub fn $left_decl$first() -> u8 { let value = 1u8; let _ = $left_body$value; value }
}
impl Right {
    pub const LIMIT: usize = 5;
    pub fn $right_decl$first() -> u16 { let value = 1u16; let _ = $right_body$value; value }
}
macro_rules! method {
    ($name:ident, $ty:ty) => { pub fn $name() -> $ty { loop {} } };
}
pub struct GeneratedLeft;
pub struct GeneratedRight;
impl GeneratedLeft { method!($generated_left_decl$copied, u32); }
impl GeneratedRight { method!($generated_right_decl$copied, u64); }
pub trait Generic<T> { type Slot; fn echo(value: T) -> T { value } }
pub trait Other<T> {
    #[lang = "fn_once_output"] type Slot;
    fn echo(value: T) -> T { value }
}
fn calls() {
    let left = Left::$left_call$first(); let _ = $left_value$left;
    let right = Right::$right_call$first(); let _ = $right_value$right;
    let generated_left = GeneratedLeft::$generated_left_call$copied(); let _ = $generated_left_value$generated_left;
    let generated_right = GeneratedRight::$generated_right_call$copied(); let _ = $generated_right_value$generated_right;
}
"#,
    );
    let path = fixture.path("src/lib.rs");
    let text = fs::read_to_string(&path).expect("fixture source");
    let check = |project: &Project, reverse: bool, current: bool| {
        project.inspect(|snapshot| {
            let context = snapshot
                .file_context_for_path(&path)
                .expect("route source")
                .expect("target");
            let target = (context.crate_ref, context.file);
            let analysis = if current {
                let source = snapshot
                    .prepare_current_source(target, &text, &CancellationToken::new())
                    .expect("editor source");
                let (analysis, summary) = snapshot
                    .analysis_for_current_source(
                        target,
                        source,
                        CurrentSourceSelection::IntersectingRange(Span {
                            start: 0,
                            end: text.len() as u32,
                        }),
                        |_| Ok(()),
                    )
                    .expect("editor bodies");
                assert!(summary.is_complete(), "{summary:?}");
                analysis
            } else {
                snapshot.analysis()
            };
            let mut cases = vec![
                ("left", "first", UnsignedIntTy::U8),
                ("right", "first", UnsignedIntTy::U16),
                ("generated_left", "copied", UnsignedIntTy::U32),
                ("generated_right", "copied", UnsignedIntTy::U64),
            ];
            if reverse {
                cases.reverse();
            }
            for (name, spelling, expected) in cases {
                let offset = |suffix| {
                    fixture
                        .markers()
                        .position(&format!("{name}_{suffix}"))
                        .offset
                };
                assert_eq!(
                    analysis
                        .type_at(target.0, target.1, offset("value"))
                        .expect("call type")
                        .expect("inferred result")
                        .primitive(),
                    Some(PrimitiveTy::UnsignedInt(expected)),
                    "{name}"
                );
                let destinations = analysis
                    .goto_definition(target.0, target.1, offset("call"))
                    .expect("method navigation");
                let declaration = Span {
                    start: offset("decl"),
                    end: offset("decl") + spelling.len() as u32,
                };
                assert_eq!(destinations.len(), 1, "{name}");
                assert_eq!(destinations[0].span, Some(declaration), "{name}");
                if spelling == "first" {
                    assert_eq!(
                        analysis
                            .type_at(target.0, target.1, offset("body"))
                            .expect("method body type")
                            .expect("inferred local")
                            .primitive(),
                        Some(PrimitiveTy::UnsignedInt(expected)),
                        "{name}"
                    );
                    if !current {
                        let crates = [target.0];
                        let references = analysis
                            .references(
                                target.0,
                                target.1,
                                offset("call"),
                                ReferenceQuery::find_references(&crates, true),
                            )
                            .expect("method references");
                        assert_eq!(
                            references
                                .iter()
                                .map(|location| location.span)
                                .collect::<Vec<_>>(),
                            [
                                declaration,
                                Span {
                                    start: offset("call"),
                                    end: offset("call") + 5
                                }
                            ],
                            "{name}"
                        );
                    }
                }
            }

            let reader = ItemReader(snapshot.declarations());
            let items = ItemStoreQuery::new(&reader);
            let declarations = reader
                .all_items(DefMapRef::Crate(target.0))
                .expect("declarations");
            let mut first = Vec::new();
            let bodies = BodyReader::new(snapshot.query_context(), snapshot.declarations());
            for item in &declarations {
                match item.item() {
                    SemanticItemRef::Function(function)
                        if item.name().is_some_and(|name| name.as_str() == "first") =>
                    {
                        first.push(function)
                    }
                    SemanticItemRef::Function(function)
                        if item.name().is_some_and(|name| name.as_str() == "echo") =>
                    {
                        let generics = GenericsQuery::new(&reader)
                            .generics(function.into())
                            .expect("inherited generics");
                        assert_eq!(
                            generics.parent_len(),
                            2,
                            "trait Self and T belong to the header"
                        );
                    }
                    SemanticItemRef::Const(konst) => {
                        let ItemStoreId::Impl(id) = konst.store.id else {
                            panic!("associated const");
                        };
                        let owner = items
                            .impl_data(ImplRef {
                                origin: konst.store.origin,
                                id,
                            })
                            .expect("impl header")
                            .expect("impl");
                        let expected = if owner.self_ty.to_string() == "Left" {
                            "2"
                        } else {
                            "5"
                        };
                        assert_eq!(
                            bodies
                                .initializer_value(konst.into())
                                .expect("const body")
                                .expect("const value")
                                .to_string(),
                            expected
                        );
                    }
                    _ => {}
                }
            }
            assert_eq!(first.len(), 2);
            assert_eq!(first[0].id, FunctionId(0));
            assert_eq!(first[1].id, FunctionId(0));
            assert_ne!(first[0], first[1]);

            let lookup = ItemLookupQuery::build_from(reader, target.0, &CancellationToken::new())
                .expect("language items");
            let output = lookup
                .lang_type_alias(LangItem::FnOnceOutput)
                .expect("associated language item");
            let ItemStoreId::Trait(id) = output.store.id else {
                panic!("trait member identity");
            };
            let owner = items
                .trait_data(TraitDefRef {
                    origin: output.store.origin,
                    id,
                })
                .expect("trait header")
                .expect("trait");
            assert_eq!(owner.name.as_str(), "Other");
            declarations
                .iter()
                .map(|item| item.item())
                .collect::<Vec<_>>()
        })
    };
    let project = fixture.build_project();
    let identities = check(&project, false, false);
    assert_eq!(check(&project, true, true), identities);
    drop(project);
    let restarted = fixture.build_project();
    assert_eq!(check(&restarted, true, false), identities);
}

#[test]
fn nested_member_arenas_preserve_lexical_scope() {
    let fixture = ProjectSourceFixture::build(
        r#"
//- /src/lib.rs crate:nested_members
fn outer() {
    struct Left;
    struct Right;
    trait Local<T> { fn echo(value: T) -> T { value } }
    impl Local<u32> for Left {}
    impl Left { fn first() -> u8 { 1 } const LIMIT: usize = 2; }
    impl Right { fn first() -> u16 { 1 } const LIMIT: usize = 5; }
    let left = Left::first(); let _ = $left$left;
    let right = Right::first(); let _ = $right$right;
    let echo = Left::echo(1u32); let _ = $echo$echo;
    fn inner() {
        struct Left;
        impl Left { fn first() -> u64 { 1 } }
        let inner = Left::first(); let _ = $inner$inner;
    }
    inner();
}
"#,
    );
    for reverse in [false, true] {
        let project = fixture.build_project();
        project.inspect(|snapshot| {
            let context = snapshot
                .file_context_for_path(fixture.path("src/lib.rs"))
                .expect("route source")
                .expect("target");
            let analysis = snapshot.analysis();
            let mut cases = vec![
                ("left", UnsignedIntTy::U8),
                ("right", UnsignedIntTy::U16),
                ("echo", UnsignedIntTy::U32),
                ("inner", UnsignedIntTy::U64),
            ];
            if reverse {
                cases.reverse();
            }
            for (marker, expected) in cases {
                let offset = fixture.markers().position(marker).offset;
                assert_eq!(
                    analysis
                        .type_at(context.crate_ref, context.file, offset)
                        .expect("nested call type")
                        .expect("inferred result")
                        .primitive(),
                    Some(PrimitiveTy::UnsignedInt(expected)),
                    "{marker}"
                );
            }
        });
    }
}
