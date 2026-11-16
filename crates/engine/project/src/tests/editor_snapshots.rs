use std::{fs, path::Path, sync::Arc, time::Duration};

use rg_analysis::CompletionQuery;
use rg_ir_model::{PrimitiveTy, Span, UnsignedIntTy};
use rg_profile::test_support::ProfileTest;
use rg_source::CapturedSource;
use rg_std::CancellationToken;

use crate::{
    ProjectSnapshot, SavedFileChange, profile_descriptors, testonly::ProjectSourceFixture,
};

const FIXTURE: &str = r#"
//- /src/lib.rs crate:editor_snapshots
mod model;
mod consumer;
//- /src/model.rs
pub struct Value;
impl Value {
    pub fn get(&self) -> u8 { 1 }
}
pub fn inspect() { let result = Value.get(); let _ = result; }
//- /src/consumer.rs
pub fn read() {
    let value = crate::model::Value;
    let result = value.get();
    let _ = result;
}
"#;

fn result_type(snapshot: ProjectSnapshot<'_>, path: &Path) -> anyhow::Result<PrimitiveTy> {
    let context = snapshot
        .file_context_for_path(path)?
        .expect("source target");
    let text = snapshot.sources().text(context.file)?;
    let offset = text.rfind("result;").expect("result use") as u32;
    Ok(snapshot
        .analysis()
        .type_at(context.crate_ref, context.file, offset)?
        .expect("result type")
        .primitive()
        .expect("primitive result"))
}

#[test]
fn editor_declarations_reach_other_files_and_survive_request_boundaries() {
    let fixture = ProjectSourceFixture::build(FIXTURE);
    let project = fixture.build_project();
    let model = fixture.path("src/model.rs");
    let consumer = fixture.path("src/consumer.rs");
    let saved_generation = project.generation_id();
    let (saved_type, text, syntax) = project
        .with_snapshot(CancellationToken::new(), |snapshot| {
            let context = snapshot
                .file_context_for_path(&consumer)?
                .expect("consumer");
            let sources = snapshot.sources();
            let text = sources.text(context.file)?;
            let syntax = sources.syntax(
                context.file,
                snapshot
                    .package_edition(context.crate_ref.package)
                    .expect("edition"),
            )?;
            Ok((
                result_type(snapshot, &consumer)?,
                Arc::downgrade(&text),
                Arc::downgrade(&syntax),
            ))
        })
        .expect("saved query");
    assert!(text.upgrade().is_none(), "saved text is request-scoped");
    assert!(syntax.upgrade().is_none(), "syntax is request-scoped");
    assert_eq!(saved_type, PrimitiveTy::UnsignedInt(UnsignedIntTy::U8));

    let edited = CapturedSource::new(
        &model,
        r#"// moved declaration
pub struct Value;
impl Value {
    pub fn get(&self) -> u16 { 1 }
    pub fn extra(&self) -> bool { true }
}
pub fn inspect() { let result = Value.get(); let _ = result; }
"#,
    )
    .expect("editor capture");
    let documents = [edited];
    let query = || {
        project
            .with_editor_snapshot(&documents, CancellationToken::new(), |snapshot| {
                assert_eq!(
                    result_type(snapshot, &consumer)?,
                    PrimitiveTy::UnsignedInt(UnsignedIntTy::U16)
                );
                let context = snapshot
                    .file_context_for_path(&consumer)?
                    .expect("consumer target");
                let text = snapshot.sources().text(context.file)?;
                let call = (text.find(".get()").expect("method call") + 1) as u32;
                let analysis = snapshot.analysis();
                let destinations =
                    analysis.goto_definition(context.crate_ref, context.file, call)?;
                assert_eq!(destinations.len(), 1);
                let start = (documents[0].text().find("fn get").expect("edited method") + 3) as u32;
                assert_eq!(
                    destinations[0].span,
                    Some(Span {
                        start,
                        end: start + 3
                    })
                );
                assert_eq!(
                    snapshot.sources().text(destinations[0].file_id)?.as_ref(),
                    documents[0].text()
                );
                let completions = analysis.completions_at(
                    CompletionQuery::new(context.crate_ref, context.file, call)
                        .with_source_text(&text),
                )?;
                assert!(
                    completions.iter().any(|item| item.label == "extra"),
                    "{completions:?}"
                );
                Ok(())
            })
            .expect("editor query")
    };
    query();
    // Idle collection must preserve input identity, editor bytes and reusable query results.
    project.collect();
    let profile = ProfileTest::start(profile_descriptors(), "body_ir.saved,def_map.construction");
    query();
    let profile = profile.finish();
    for counter in [
        "body_ir.saved.owners",
        "body_ir.saved.inference_owners",
        "def_map.construction.import_resolution.runs",
    ] {
        assert_eq!(
            profile.inner().counter(counter).unwrap_or_default(),
            0,
            "{counter}"
        );
    }
    assert_eq!(project.generation_id(), saved_generation);
    assert!(
        project
            .queries
            .lock()
            .expect("query owner")
            .statistics()
            .session_queries
            > 0
    );

    // Releasing query work must leave the captured editor text usable. A zero retention budget
    // then forces subsequent requests to rebuild from that same input instead of saved disk text.
    project.clear_session();
    assert_eq!(
        project
            .queries
            .lock()
            .expect("query owner")
            .statistics()
            .session_queries,
        0
    );
    query();
    project.set_session_retention(ajika::Retention {
        max_entries: 0,
        max_bytes: 0,
        max_idle: Duration::ZERO,
    });
    query();
    assert_eq!(
        project
            .queries
            .lock()
            .expect("query owner")
            .statistics()
            .session_queries,
        0
    );
    query();

    // The production saved-snapshot API retains its policy even after editor operations.
    assert_eq!(
        project
            .with_snapshot(CancellationToken::new(), |snapshot| {
                result_type(snapshot, &consumer)
            })
            .expect("saved query after editor requests"),
        saved_type
    );
}

#[test]
fn saving_one_editor_document_preserves_other_edits_and_promotes_saved_results() {
    let fixture = ProjectSourceFixture::build(FIXTURE);
    let mut project = fixture.build_project();
    let model = fixture.path("src/model.rs");
    let consumer = fixture.path("src/consumer.rs");
    let model_edit = CapturedSource::new(
        &model,
        fs::read_to_string(&model)
            .expect("model source")
            .replace("u8", "u16"),
    )
    .expect("model capture");
    let consumer_edit = CapturedSource::new(
        &consumer,
        fs::read_to_string(&consumer)
            .expect("consumer source")
            .replace("value.get()", "9u32"),
    )
    .expect("consumer capture");
    project
        .with_editor_snapshot(
            &[model_edit.clone(), consumer_edit.clone()],
            CancellationToken::new(),
            |snapshot| {
                assert_eq!(
                    result_type(snapshot, &model)?,
                    PrimitiveTy::UnsignedInt(UnsignedIntTy::U16)
                );
                assert_eq!(
                    result_type(snapshot, &consumer)?,
                    PrimitiveTy::UnsignedInt(UnsignedIntTy::U32)
                );
                Ok(())
            },
        )
        .expect("both editor documents");

    fs::write(&model, model_edit.text()).expect("save model bytes");
    project
        .apply_changes([SavedFileChange::captured(model_edit)])
        .expect("publish saved model");
    let profile = ProfileTest::start(profile_descriptors(), "body_ir.saved");
    project
        .with_editor_snapshot(
            std::slice::from_ref(&consumer_edit),
            CancellationToken::new(),
            |snapshot| {
                assert_eq!(
                    result_type(snapshot, &model)?,
                    PrimitiveTy::UnsignedInt(UnsignedIntTy::U16)
                );
                assert_eq!(
                    result_type(snapshot, &consumer)?,
                    PrimitiveTy::UnsignedInt(UnsignedIntTy::U32)
                );
                Ok(())
            },
        )
        .expect("save keeps the other editor document");
    assert_eq!(
        profile
            .finish()
            .inner()
            .counter("body_ir.saved.inference_owners")
            .unwrap_or_default(),
        0,
        "saving identical bytes can promote retained inference"
    );

    assert_eq!(
        project
            .with_editor_snapshot(&[], CancellationToken::new(), |snapshot| {
                result_type(snapshot, &consumer)
            })
            .expect("discard consumer edit"),
        PrimitiveTy::UnsignedInt(UnsignedIntTy::U16)
    );
    project
        .with_editor_snapshot(&[consumer_edit], CancellationToken::new(), |snapshot| {
            assert_eq!(
                result_type(snapshot, &consumer)?,
                PrimitiveTy::UnsignedInt(UnsignedIntTy::U32)
            );
            Ok(())
        })
        .expect("reinstall unsaved consumer before restart");
    drop(project);

    let reopened = fixture.build_project();
    assert_eq!(
        reopened
            .with_snapshot(CancellationToken::new(), |snapshot| {
                result_type(snapshot, &consumer)
            })
            .expect("restart sees only saved changes"),
        PrimitiveTy::UnsignedInt(UnsignedIntTy::U16)
    );
}
