//! Prepare and infer one saved body owner with the same worklist as current-source analysis.

use rg_arena::Arena;
use rg_cfg_eval::CfgEvaluator;
use rg_def_map::{CrateDefMapReader, DefinitionReader};
use rg_ir_model::{BodyId, BodyKey};
use rg_item_tree::SourceLocations as _;
use rg_semantic_ir::{
    ItemLookupQuery, ItemReader, SavedDeclarationSourceView, TargetDeclarationReader,
    target_declarations,
};
use rg_source::{QueryContext, ReadError};
use rg_std::Shrink as _;
use rg_workspace::{PackageInput, TargetInput};

use super::{
    lower::{
        BodyLoweringTask, BodyMacroExpansion, BodyTaskLowering, BodyTaskSource, LoweredBodies,
    },
    state::BodyStoreBuilder,
};
use crate::{BodyFactsStore, BodySourceStore, BodyStore, PreparedBodyStore, profile::metric};

/// One preparation produces semantic data and fresh occurrences as independently compared outputs.
#[ajika::output]
pub struct PreparedBody {
    data: PreparedBodyStore,
    sources: Arena<BodyId, BodySourceStore>,
}

/// Query a single outer owner. The current target directory establishes whether the owner still
/// exists; an older record cannot supply its previous body after that declaration disappears.
#[ajika::query]
pub fn prepare_saved_body(
    db: &QueryContext<'_>,
    key: BodyKey,
) -> ajika::Result<Option<PreparedBody>> {
    metric::OWNERS.inc();
    let declarations = target_declarations(db, key.krate)?;
    let owners = declarations.body_owners.value_ref(db)?;
    let Some(owner) = owners
        .iter()
        .find(|owner| owner.owner == key.owner)
        .copied()
    else {
        return Ok(None);
    };

    // TODO: Lower bodies produced by item-position macros from their expansion syntax.
    // Their navigation fallback is an invocation, which is not a written body declaration.
    if owner.source.as_item_tree().is_none() {
        return Ok(None);
    }
    let span = SavedDeclarationSourceView::new(db, key.krate, owner.source)?
        .location(owner.location)
        .ok_or_else(|| ajika::Error::application("body owner occurrence is absent"))?
        .span;

    let readers = TargetDeclarationReader::for_target(db, key.krate)?;
    let definitions: DefinitionReader<'_> = CrateDefMapReader(&readers);
    let items = ItemReader(&readers);
    let package = PackageInput::at(key.krate.package);
    let edition = package.edition(db)?;
    let cfg = package.cfg(db)?;
    let target_kind = TargetInput::at(key.krate).kind(db)?;
    let cfg = CfgEvaluator::new(&cfg, target_kind.enables_test_cfg());
    let source = BodyTaskSource::Tracked { db, edition };
    let cancellation = db.services().cancellation.clone();
    let build = || -> anyhow::Result<_> {
        let mut lowered = LoweredBodies::default();
        let mut macros = BodyMacroExpansion::new(edition, &definitions, cfg, cancellation.clone());
        BodyTaskLowering::new(source, &mut lowered, cfg, &cancellation).lower_tasks(
            &[BodyLoweringTask::for_saved_owner(key.krate, owner, span)],
            &mut macros,
        )?;
        if lowered.bodies().is_empty() {
            return Ok(None);
        }
        // Nested bodies allocate into this root's arena. They can borrow earlier local items in
        // the worklist without crossing an Ajika query boundary or aliasing another root's IDs.
        let (mut data, mut sources) =
            BodyStoreBuilder::new(key.body(BodyId(0)), source, lowered, cancellation)
                .prepare_saved(&definitions, &items, cfg)?;
        data.shrink_to_fit();
        sources.shrink_to_fit();
        Ok(Some(PreparedBody::new(db, data, sources)?))
    };
    build().map_err(|error| {
        ReadError::query_cause(error.as_ref())
            .unwrap_or_else(|| ajika::Error::application(error.into_boxed_dyn_error()))
    })
}

/// Inference observes the prepared meaning through its own query frame. Source movement can
/// rerun preparation without invalidating this result when the semantic output stays equal.
#[ajika::query]
pub fn infer_saved_body(db: &QueryContext<'_>, key: BodyKey) -> ajika::Result<BodyFactsStore> {
    let prepared = prepare_saved_body(db, key)?;
    let Some(prepared) = prepared.as_ref() else {
        return Ok(BodyFactsStore::default());
    };
    let data = prepared.data.value_ref(db)?;
    metric::INFERRED_OWNERS.inc();
    let readers = TargetDeclarationReader::for_target(db, key.krate)?;
    let definitions: DefinitionReader<'_> = CrateDefMapReader(&readers);
    let items = ItemReader(&readers);
    let cancellation = &db.services().cancellation;
    let infer = || -> anyhow::Result<_> {
        let lookup = ItemLookupQuery::build_from(items, key.krate, cancellation)?;
        data.infer(&definitions, &items, &lookup, cancellation)
    };
    infer().map_err(|error| {
        ReadError::query_cause(error.as_ref())
            .unwrap_or_else(|| ajika::Error::application(error.into_boxed_dyn_error()))
    })
}

/// Share the acquired payloads during one operation without persisting another complete body.
#[ajika::query(transient, returns(ref))]
pub fn saved_body(db: &QueryContext<'_>, key: BodyKey) -> ajika::Result<Option<BodyStore>> {
    let prepared = prepare_saved_body(db, key)?;
    let Some(prepared) = prepared.as_ref() else {
        return Ok(None);
    };
    Ok(Some(BodyStore::new(
        prepared.data.value(db)?,
        infer_saved_body(db, key)?,
        prepared.sources.value(db)?,
    )))
}
