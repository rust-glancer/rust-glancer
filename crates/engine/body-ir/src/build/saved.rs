//! Prepare and infer one saved body owner with the same worklist as current-source analysis.

use rg_cfg_eval::CfgEvaluator;
use rg_def_map::{CrateDefMapReader, DefinitionReader};
use rg_ir_model::{BodyId, BodyKey, BodyRoot, EvaluatedConstValue, SavedBodyOwner};
use rg_item_tree::SourceLocations as _;
use rg_semantic_ir::{
    ItemLookupQuery, ItemReader, SavedDeclarationSourceView, TargetDeclarationReader,
    target_declarations,
};
use rg_source::{QueryContext, ReadError};
use rg_std::Shrink as _;
use rg_workspace::{PackageInput, TargetInput};
use wincode::{SchemaRead, SchemaWrite};

use super::{
    lower::{
        BodyLoweringTask, BodyMacroExpansion, BodyTaskLowering, BodyTaskSource, LoweredCrateBodies,
    },
    state::CrateBodyBuildState,
};
use crate::{BodyStore, profile::metric, resolution::BodyResolutionContext};

#[derive(Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct SavedBody {
    pub bodies: BodyStore,
    pub initializer: Option<EvaluatedConstValue>,
}

/// Query a single outer owner. The current target directory establishes whether the owner still
/// exists; an older record cannot supply its previous body after that declaration disappears.
#[ajika::query(returns(ref))]
pub fn saved_body(db: &QueryContext<'_>, key: BodyKey) -> ajika::Result<Option<SavedBody>> {
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
        let mut lowered = LoweredCrateBodies::default();
        let mut macros = BodyMacroExpansion::new(edition, &definitions, cfg, cancellation.clone());
        BodyTaskLowering::new(source, &mut lowered, cfg, &cancellation).lower_tasks(
            &[BodyLoweringTask::for_saved_owner(key.krate, owner, span)],
            &mut macros,
        )?;
        if lowered.bodies().is_empty() {
            return Ok(None);
        }
        let initializer = if matches!(
            key.owner,
            SavedBodyOwner::Const(_) | SavedBodyOwner::Static(_)
        ) {
            // Saved declaration hover needs the same value as the old initializer pass. Evaluate
            // it from the owner's lowered expression before that temporary syntax is dropped.
            let lookup = ItemLookupQuery::build_from(items, key.krate, &cancellation)?;
            let lowered_root = &lowered.bodies()[BodyId(0)];
            BodyResolutionContext::new(
                &definitions,
                &items,
                key.body(BodyId(0)),
                lowered_root.body(),
                &lookup,
                cancellation.clone(),
            )
            .evaluate_initializer(lowered_root)?
            .map(|(_, value)| value)
        } else {
            None
        };
        // Nested bodies allocate into this root's arena. They can borrow earlier local items in
        // the worklist without crossing an Ajika query boundary or aliasing another root's IDs.
        let mut bodies = CrateBodyBuildState::new(
            key.krate,
            source,
            BodyRoot::saved(key.owner),
            lowered,
            cancellation,
        )
        .resolve(&definitions, &items, cfg)?;
        bodies.shrink_to_fit();
        Ok(Some(SavedBody {
            bodies,
            initializer,
        }))
    };
    build().map_err(|error| {
        ReadError::query_cause(error.as_ref())
            .unwrap_or_else(|| ajika::Error::application(error.into_boxed_dyn_error()))
    })
}
