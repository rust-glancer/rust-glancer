use std::sync::atomic::{AtomicUsize, Ordering};

use rg_def_map::CrateDependencies;
use rg_ir_model::{BodyId, BodyKey, BodyRef, DefMapRef, ExprId, SemanticItemRef};
use rg_item_tree::testonly::fixture_database;
use rg_profile::test_support::ProfileTest;
use rg_semantic_ir::{CrateItemSource as _, SemanticItemSource as _, TargetDeclarationReader};
use rg_source::{CapturedSource, QueryContext, SourceInput};
use rg_ty::{ConstValue, GenericArg};
use rg_workspace::{TargetKind, testonly::WorkspaceFixture};

use crate::{BodyReader, ExprKind, profile_descriptors, testonly::BodyIrFixture};

static SEMANTIC_BODY_READS: AtomicUsize = AtomicUsize::new(0);

// Count consumer executions, rather than preparation: trivia must refresh preparation's source
// output while callers that only read local meaning keep their cached answers.
#[ajika::query]
fn body_local_meaning(
    db: &QueryContext<'_>,
    body: BodyRef,
) -> ajika::Result<Option<(Vec<String>, DefMapRef)>> {
    SEMANTIC_BODY_READS.fetch_add(1, Ordering::Relaxed);
    let declarations = TargetDeclarationReader::for_target(db, body.crate_ref)?;
    let reader = BodyReader::new(db, &declarations);
    let Some(def_map) = reader.body_def_map(body)? else {
        return Ok(None);
    };
    let mut names: Vec<_> = (&reader)
        .all_items(DefMapRef::Body(body))?
        .into_iter()
        .filter_map(|item| item.name().map(ToString::to_string))
        .collect();
    names.sort();
    Ok(Some((names, def_map.own_ref())))
}

#[test]
fn semantic_body_reads_reuse_meaning_after_source_movement() {
    let workspace = WorkspaceFixture::build(
        r#"
//- /src/lib.rs crate:body_meaning
pub fn outer() { struct Local; const VALUE: usize = 2 + 2; fn nested() {} }
"#,
    );
    let mut database = fixture_database(workspace.metadata());
    let targets = database
        .update_inputs(|update| CrateDependencies::write_inputs(workspace.metadata(), update))
        .expect("fixture targets");
    let target = targets[0];
    let body = database
        .run(|db| {
            let declarations = TargetDeclarationReader::for_target(db, target)?;
            let outer = declarations
                .scope_items(DefMapRef::Crate(target))?
                .into_iter()
                .find_map(|item| match item.item() {
                    SemanticItemRef::Function(function)
                        if item.name().is_some_and(|name| name.as_str() == "outer") =>
                    {
                        Some(function.into())
                    }
                    _ => None,
                })
                .expect("fixture outer function");
            Ok(BodyKey {
                krate: target,
                owner: outer,
            }
            .body(BodyId(0)))
        })
        .expect("fixture body identity");

    let path = workspace.metadata().packages()[0]
        .root_dir()
        .join("src/lib.rs");
    let interner = database.interner();
    SEMANTIC_BODY_READS.store(0, Ordering::Relaxed);
    let profile = ProfileTest::start(profile_descriptors(), "body_ir.const_eval");
    for (source, expected_name, expected_reads) in [
        (
            "pub fn outer() { struct Local; const VALUE: usize = 2 + 2; fn nested() {} }",
            "Local",
            1,
        ),
        (
            "// moved owner\npub fn outer() { /* moved item */ struct Local; const VALUE: usize = 2  + 2; fn nested() {} }",
            "Local",
            1,
        ),
        (
            "pub fn outer() { struct Changed; const VALUE: usize = 2 + 2; fn nested() {} }",
            "Changed",
            2,
        ),
    ] {
        let source = CapturedSource::new(&path, source).expect("fixture source");
        database
            .update_inputs(|update| SourceInput::set_saved(update, &interner, &source))
            .expect("update fixture source");
        database
            .run(|db| {
                let meaning = body_local_meaning(db, body)?;
                let (names, origin) = meaning.as_ref().as_ref().expect("visible body");
                assert_eq!(names, &[expected_name, "VALUE", "nested"]);
                assert_eq!(*origin, DefMapRef::Body(body));
                Ok(())
            })
            .expect("read local meaning");
        assert_eq!(
            SEMANTIC_BODY_READS.load(Ordering::Relaxed),
            expected_reads,
            "{source:?}"
        );
    }
    assert_eq!(
        profile
            .finish()
            .inner()
            .counter("body_ir.const_eval.initializers")
            .unwrap_or_default(),
        0
    );
}

#[test]
fn finalized_bodies_publish_types_and_selected_calls() {
    let mut fixture = BodyIrFixture::build(
        r#"
//- /src/lib.rs crate:body_lifecycle_fixture
fn identity<'a, T, const N: usize>(value: &'a [T; N]) -> &'a [T; N] { value }

pub fn use_it(value: &[u8; 3]) {
    let inferred: _ = identity::<u8, 3>(value);
}
"#,
    );
    fixture.inspect(|view| {
        let target = view
            .semantic()
            .crate_ref("body_lifecycle_fixture", TargetKind::Lib);
        let bodies = view.bodies(target);

        let mut selected_calls = Vec::new();
        for (_, body) in bodies {
            for (expr_idx, data) in body.exprs().iter().enumerate() {
                let expr = ExprId(expr_idx);
                assert!(
                    body.expr_ty(expr).is_some(),
                    "every expression has type facts"
                );
                if matches!(
                    data.kind,
                    ExprKind::Call { .. } | ExprKind::MethodCall { .. }
                ) && let Some(call) = body.call_facts(expr)
                {
                    selected_calls.push(call);
                }
            }
        }

        let [call] = selected_calls.as_slice() else {
            panic!("fixture should persist exactly one selected call")
        };
        assert!(matches!(
            call.generic_args().as_slice(),
            [
                GenericArg::Lifetime(_),
                GenericArg::Type(_),
                GenericArg::Const(ConstValue::Scalar(3)),
            ]
        ));
    });
}
