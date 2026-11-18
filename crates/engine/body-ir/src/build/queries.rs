//! Prepare and infer one outer owner and the bodies nested inside it.

use rg_arena::Arena;
use rg_def_map::{CrateDefMapReader, DefinitionReader};
use rg_ir_model::{BodyId, BodyKey};
use rg_item_tree::SourceLocations as _;
use rg_semantic_ir::{
    BodyOwnerData, ItemLookupQuery, ItemReader, ItemSourceView, TargetDeclarationReader, body_owner,
};
use rg_source::{QueryContext, ReadError};
use rg_std::Shrink as _;
use rg_workspace::CrateConfig;

use super::{
    lower::{BodyLoweringTask, BodyMacroExpansion, BodyTaskLowering, LoweredBodies},
    state::BodyStoreBuilder,
};
use crate::{BodyFactsStore, BodySourceStore, BodyStore, PreparedBodyStore, profile::metric};

/// One preparation produces semantic data and fresh occurrences as independently compared outputs.
#[ajika::output]
pub struct PreparedBody {
    data: PreparedBodyStore,
    sources: Arena<BodyId, BodySourceStore>,
}

impl PreparedBody {
    pub(crate) fn can_prepare(owner: &BodyOwnerData) -> bool {
        // TODO: Lower bodies emitted by item-position macros from their expansion syntax.
        // Their invocation has no function or initializer AST for the file lowerer to open.
        owner.source.as_item_tree().is_some()
    }
}

/// Query a single outer owner. The target's body-owner list establishes whether the owner still
/// exists; an older record cannot supply its previous body after that declaration disappears.
#[ajika::query]
pub fn prepare_body(db: &QueryContext<'_>, key: BodyKey) -> ajika::Result<Option<PreparedBody>> {
    metric::OWNERS.inc();
    let Some(owner) = body_owner(db, key)?
        .copied()
        .filter(PreparedBody::can_prepare)
    else {
        return Ok(None);
    };

    let span = ItemSourceView::new(db, key.krate, owner.source)?
        .location(owner.location)
        .ok_or_else(|| ajika::Error::application("body owner occurrence is absent"))?
        .span;

    let readers = TargetDeclarationReader::for_target(db, key.krate)?;
    let definitions: DefinitionReader<'_> = CrateDefMapReader(&readers);
    let items = ItemReader(&readers);
    let config = CrateConfig::read(db, key.krate)?;
    let edition = config.edition;
    let cfg = config.cfg();
    let cancellation = db.services().cancellation.clone();
    let build = || -> anyhow::Result<_> {
        let mut lowered = LoweredBodies::default();
        let mut macros = BodyMacroExpansion::new(edition, &definitions, cfg, cancellation.clone());
        BodyTaskLowering::new(db, edition, &mut lowered, cfg, &cancellation)
            .lower_tasks(&[BodyLoweringTask::for_owner(owner, span)], &mut macros)?;
        if lowered.bodies().is_empty() {
            return Ok(None);
        }
        // Nested bodies allocate into this root's arena. They can borrow earlier local items in
        // the worklist without crossing an Ajika query boundary or aliasing another root's IDs.
        let (mut data, mut sources) =
            BodyStoreBuilder::new(key, db, edition, lowered, cancellation).prepare(
                &definitions,
                &items,
                cfg,
            )?;
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
pub fn infer_body(db: &QueryContext<'_>, key: BodyKey) -> ajika::Result<BodyFactsStore> {
    let prepared = prepare_body(db, key)?;
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
pub fn body_store(db: &QueryContext<'_>, key: BodyKey) -> ajika::Result<Option<BodyStore>> {
    let prepared = prepare_body(db, key)?;
    let Some(prepared) = prepared.as_ref() else {
        return Ok(None);
    };
    Ok(Some(BodyStore::new(
        prepared.data.value(db)?,
        infer_body(db, key)?,
        prepared.sources.value(db)?,
    )))
}
