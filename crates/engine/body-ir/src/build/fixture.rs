//! Run body kernels over local arenas for tests of lowering and inference.

use std::collections::HashMap;

use rg_cfg_eval::CfgEvaluator;
use rg_ir_model::{BodyRef, BodyRoot, CrateId, CrateRef, PackageSlot};
use rg_semantic_ir::{ItemLookupQuery, ItemLookupQueryCache, testonly::SemanticIrFixture};
use rg_std::CancellationToken;

use super::{
    lower::{BodyTaskSource, CrateLowering},
    state::CrateBodyBuildState,
};
use crate::{BodyStore, resolution::BodyResolutionContext};

pub(crate) fn build_bodies(
    semantic: &mut SemanticIrFixture,
) -> anyhow::Result<HashMap<CrateRef, BodyStore>> {
    let definitions = semantic.def_map_db().reader();
    let items = semantic.semantic_ir_db().reader();
    let cancellation = CancellationToken::new();
    let lookup_cache = ItemLookupQueryCache::new();
    let mut stores = HashMap::new();
    let mut values = Vec::new();
    for (package_slot, package) in semantic.parse_db().packages().iter().enumerate() {
        for target in package.targets() {
            let krate = CrateRef {
                package: PackageSlot(package_slot),
                crate_id: CrateId(target.id.0),
            };
            let def_map = definitions
                .def_map(krate)?
                .expect("fixture target has definitions");
            let store = items.items(krate)?.expect("fixture target has items");
            let cfg = CfgEvaluator::new(package.cfg_options(), target.enables_test_cfg());
            let include_functions =
                package.is_workspace_member() && target.kind.is_primary_analysis_target();
            let lowered = CrateLowering {
                parse_package: package,
                def_map: &definitions,
                definitions: def_map,
                items: store,
                include_functions,
                crate_bodies: Default::default(),
                cfg,
                cancellation: &cancellation,
            }
            .lower()?;
            if !lowered.bodies().is_empty() {
                let lookup =
                    ItemLookupQuery::build_with_cache(items, krate, &lookup_cache, &cancellation)?;
                for (id, body) in lowered.bodies().iter_with_ids() {
                    if matches!(body.body().owner(), crate::BodyOwner::Function(_)) {
                        continue;
                    }
                    let context = BodyResolutionContext::new(
                        &definitions,
                        &items,
                        BodyRef::local(krate, id),
                        body.body(),
                        &lookup,
                        cancellation.clone(),
                    );
                    values.extend(context.evaluate_initializer(body)?);
                }
            }
            if include_functions {
                let bodies = CrateBodyBuildState::new(
                    krate,
                    BodyTaskSource::Parsed(package),
                    BodyRoot::Local,
                    lowered,
                    cancellation.clone(),
                )
                .resolve(&definitions, &items, &lookup_cache, cfg)?;
                stores.insert(krate, bodies);
            }
        }
    }
    semantic
        .semantic_ir_db_mut()
        .apply_initializer_values(values);
    Ok(stores)
}
