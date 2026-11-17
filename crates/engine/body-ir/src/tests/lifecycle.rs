use std::sync::atomic::{AtomicUsize, Ordering};

use rg_def_map::CrateDependencies;
use rg_ir_model::{
    BodyId, BodyKey, BodyRef, DefMapRef, ExprId, FileId, ModuleRef, SavedBodyOwner, SemanticItemRef,
};
use rg_semantic_ir::{CrateItemSource as _, SemanticItemSource as _, TargetDeclarationReader};
use rg_source::{CapturedSource, QueryContext, SourceInput};
use rg_ty::{ConstValue, GenericArg};
use rg_workspace::{TargetKind, testonly::WorkspaceFixture};

use crate::{BodyReader, CurrentSourceStore, ExprKind, testonly::BodyIrFixture};

static SEMANTIC_BODY_READS: AtomicUsize = AtomicUsize::new(0);

// Count consumer executions, rather than preparation: trivia must refresh preparation's source
// output while callers that only read local meaning keep their cached answers.
#[ajika::query]
fn body_local_meaning(
    db: &QueryContext<'_>,
    body: BodyRef,
    masked_file: Option<FileId>,
) -> ajika::Result<Option<(Vec<String>, ModuleRef)>> {
    SEMANTIC_BODY_READS.fetch_add(1, Ordering::Relaxed);
    let declarations = TargetDeclarationReader::for_target(db, body.crate_ref)?;
    let reader = BodyReader::new(db, &declarations).with_current_source(CurrentSourceStore {
        masked_file: masked_file.map(|file| (body.crate_ref, file)),
        ..CurrentSourceStore::default()
    });
    let Some(def_map) = reader.body_def_map(body)? else {
        return Ok(None);
    };
    let module = def_map.module_refs().next().expect("body has a root scope");
    let lookup = reader.signature_lookup_module(module)?;
    let mut names: Vec<_> = (&reader)
        .all_items(DefMapRef::Body(body))?
        .into_iter()
        .filter_map(|item| item.name().map(ToString::to_string))
        .collect();
    names.sort();
    Ok(Some((names, lookup)))
}

#[test]
fn semantic_body_reads_reuse_meaning_after_source_movement() {
    let workspace = WorkspaceFixture::build(
        r#"
//- /src/lib.rs crate:body_meaning
mod other;
pub fn outer() { struct Local; fn nested() {} }
//- /src/other.rs
pub fn other() {}
"#,
    );
    let mut database = rg_item_tree::testonly::fixture_database(workspace.metadata());
    let targets = database
        .update_inputs(|update| CrateDependencies::write_inputs(workspace.metadata(), update))
        .expect("fixture targets");
    let target = targets[0];
    let (body, owner_file, other_file) = database
        .run(|db| {
            let declarations = TargetDeclarationReader::for_target(db, target)?;
            let reader = BodyReader::new(db, &declarations);
            let owners: Vec<_> = reader.owners(target)?.copied().collect();
            let outer = declarations
                .all_items(DefMapRef::Crate(target))?
                .into_iter()
                .find_map(|item| match item.item() {
                    SemanticItemRef::Function(function)
                        if item.name().is_some_and(|name| name.as_str() == "outer") =>
                    {
                        Some(SavedBodyOwner::Function(function.id))
                    }
                    _ => None,
                })
                .expect("fixture outer function");
            let owner = owners
                .iter()
                .find(|owner| owner.owner == outer)
                .expect("outer function has a body");
            let other_file = owners
                .iter()
                .find(|other| other.file != owner.file)
                .expect("fixture has owners in two files")
                .file;
            Ok((
                BodyKey {
                    krate: target,
                    owner: owner.owner,
                }
                .body(BodyId(0)),
                owner.file,
                other_file,
            ))
        })
        .expect("fixture body identity");

    let path = workspace.metadata().packages()[0]
        .root_dir()
        .join("src/lib.rs");
    let interner = database.interner();
    SEMANTIC_BODY_READS.store(0, Ordering::Relaxed);
    for (source, expected_name, expected_reads) in [
        (
            "mod other; pub fn outer() { struct Local; fn nested() {} }",
            "Local",
            3,
        ),
        (
            "// moved owner\nmod other; pub fn outer() { /* moved item */ struct Local; fn nested() {} }",
            "Local",
            3,
        ),
        (
            "mod other; pub fn outer() { struct Changed; fn nested() {} }",
            "Changed",
            5,
        ),
    ] {
        let source = CapturedSource::new(&path, source).expect("fixture source");
        database
            .update_inputs(|update| SourceInput::set_saved(update, &interner, &source))
            .expect("update fixture source");
        database
            .run(|db| {
                // An overlay in another file leaves this owner visible; its own file hides it.
                for masked_file in [None, Some(other_file)] {
                    let meaning = body_local_meaning(db, body, masked_file)?;
                    let (names, module) = meaning.as_ref().as_ref().expect("visible body");
                    assert_eq!(names, &[expected_name, "nested"]);
                    assert_eq!(module.origin, DefMapRef::Body(body));
                }
                assert!(body_local_meaning(db, body, Some(owner_file))?.is_none());
                Ok(())
            })
            .expect("read local meaning");
        assert_eq!(
            SEMANTIC_BODY_READS.load(Ordering::Relaxed),
            expected_reads,
            "{source:?}"
        );
    }
}

#[test]
fn finalized_bodies_have_aligned_structural_and_semantic_arenas() {
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
            assert_eq!(body.exprs().len(), body.expr_facts().len());
            for (expr_idx, data) in body.exprs().iter().enumerate() {
                let expr = ExprId(expr_idx);
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
