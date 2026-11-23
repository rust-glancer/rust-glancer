//! Prepare and infer one outer owner and the bodies nested inside it.

use rg_arena::Arena;
use rg_def_map::{
    CrateDefMapReader, DefinitionReader, GeneratedBodySyntax, ItemSourceKind, MacroCopyCoverage,
};
use rg_ir_model::{BodyId, BodyKey};
use rg_item_tree::SourceLocations as _;
use rg_semantic_ir::{
    BodyOwnerData, ItemLookupQuery, ItemReader, ItemSourceView, TargetDeclarationReader, body_owner,
};
use rg_source::{QueryContext, ReadError};
use rg_workspace::CrateConfig;

use super::{
    lower::{
        BodyLoweringTask, BodyMacroExpansion, BodyTaskLowering, BodyTaskSource, GeneratedBodyTask,
        LoweredBodies,
    },
    state::BodyStoreBuilder,
};
use crate::{BodyFactsStore, BodySourceStore, BodyStore, PreparedBodyStore, profile::metric};

/// One preparation produces semantic data and fresh occurrences as independently compared outputs.
#[ajika::output]
pub struct PreparedBody {
    data: PreparedBodyStore,
    sources: Arena<BodyId, BodySourceStore>,
    expansions: Vec<MacroCopyCoverage>,
}

impl PreparedBody {
    pub(crate) fn can_prepare(owner: &BodyOwnerData) -> bool {
        matches!(
            owner.source.kind,
            ItemSourceKind::ItemTree(_) | ItemSourceKind::Generated(_)
        )
    }
}

/// Query a single outer owner. The target's body-owner list establishes whether the owner still
/// exists; an older record cannot supply its previous body after that declaration disappears.
#[ajika::query]
pub fn prepare_body(db: &QueryContext<'_>, key: BodyKey) -> ajika::Result<Option<PreparedBody>> {
    metric::OWNERS.inc();
    let Some(owner) = body_owner(db, key)?.copied() else {
        return Ok(None);
    };

    let source = match owner.source.kind {
        ItemSourceKind::ItemTree(item) => {
            if let Some(body) = GeneratedBodySyntax::for_file_item(db, key.krate, item)? {
                BodyTaskSource::Generated(GeneratedBodyTask::for_owner(body))
            } else {
                let span = ItemSourceView::new(db, key.krate, owner.source)?
                    .location(owner.location)
                    .ok_or_else(|| ajika::Error::application("body owner occurrence is absent"))?
                    .span;
                BodyTaskSource::File {
                    file: owner.file,
                    span,
                }
            }
        }
        ItemSourceKind::Generated(item) => {
            let Some(body) = ItemSourceView::generated_body_syntax(db, key.krate, item)? else {
                return Ok(None);
            };
            BodyTaskSource::Generated(GeneratedBodyTask::for_owner(body))
        }
        ItemSourceKind::Synthetic(_) | ItemSourceKind::Body(_) => return Ok(None),
    };

    let readers = TargetDeclarationReader::for_target(db, key.krate)?;
    let definitions: DefinitionReader<'_> = CrateDefMapReader(&readers);
    let items = ItemReader(&readers);
    let config = CrateConfig::read(db, key.krate)?;
    let edition = config.edition;
    let cfg = config.cfg();
    let cancellation = db.services().cancellation.clone();
    let build = || -> anyhow::Result<_> {
        let mut lowered = LoweredBodies::new(key);
        let mut macros = BodyMacroExpansion::new(edition, &definitions, cfg, cancellation.clone());
        BodyTaskLowering::new(db, edition, &mut lowered, cfg, &cancellation).lower_tasks(
            &mut [BodyLoweringTask::for_owner(owner, source)],
            &mut macros,
        )?;
        if lowered.bodies().is_empty() {
            return Ok(None);
        }
        // Nested bodies allocate into this root's arena. They can borrow earlier local items in
        // the worklist without crossing an Ajika query boundary or aliasing another root's IDs.
        let prepared = BodyStoreBuilder::new(key, db, edition, lowered, cancellation).prepare(
            &definitions,
            &items,
            cfg,
        )?;
        Ok(Some(prepared))
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
