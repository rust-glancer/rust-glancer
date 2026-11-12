//! Collects module declarations, direct bindings, and raw imports from item-shaped syntax.
//!
//! Target roots start here. Written files, included fragments, and macro output all use the shared
//! declaration collector below. Import lookup and macro execution happen later during scope
//! construction, using the same crate builders and source identities.

use std::{collections::HashMap, sync::Arc};

use anyhow::Context as _;
use rg_cfg_eval::CfgOptions;
use rg_ir_model::{CrateRef, DefMapRef, ModuleRef};
use rg_item_tree::ItemTrees;
use rg_parse::{CargoTarget, ModuleFileContext};
use rg_text::{Name, RustEdition};

use super::{
    MacroSourceFileResolutions,
    crates::{CrateBuilder, ExternPreludeBuilder, KnownModuleFiles},
    macros::{MacroCallOrigin, TextualMacroScopes},
};
use crate::{DefMapBuilder, ModuleBuilder, ModuleDocumentation, ModuleOrigin, Visibility};

mod declarations;
pub(crate) use self::declarations::DeclarationCollector;

#[allow(clippy::too_many_arguments)]
pub(super) fn collect_crate(
    crate_ref: CrateRef,
    target: &CargoTarget,
    edition: RustEdition,
    cfg_options: CfgOptions,
    item_tree_package: &ItemTrees,
    roots: HashMap<Name, ModuleRef>,
    known_module_files: Arc<KnownModuleFiles>,
) -> anyhow::Result<CrateBuilder> {
    let resolutions = MacroSourceFileResolutions::default();
    let root_file = item_tree_package
        .file(target.root_file)
        .context("target root file should be lowered")?;
    let mut builder = DefMapBuilder::new(DefMapRef::Crate(crate_ref));
    // A target root has neither a declaration name nor a parent. All descendants use the
    // shared collector, just like files and declarations discovered by macro expansion.
    let mut root = ModuleBuilder::new(
        ModuleOrigin::Root {
            file_id: target.root_file,
        },
        Visibility::Public,
        None,
    );
    root.docs = ModuleDocumentation::new(None, root_file.docs.clone());
    let root_module = builder.alloc_module(root);
    let mut state = CrateBuilder {
        crate_ref,
        cargo_target: target.id,
        crate_name: target.name.clone(),
        root_module,
        edition,
        cfg_options,
        target_kind: target.kind.clone(),
        known_module_files,
        def_map_builder: builder,
        extern_prelude: ExternPreludeBuilder::new(roots),
        prelude: None,
        macro_definitions: HashMap::new(),
        textual_macro_scopes: TextualMacroScopes::default(),
        macro_use_imports: Vec::new(),
        macro_directives: Vec::new(),
        macro_source_file_requests: Vec::new(),
        pending_generated_modules: Vec::new(),
        pending_includes: Vec::new(),
        macro_expansion_limit: None,
    };
    let context = Arc::new(ModuleFileContext::for_target_root(&target.src_path));
    DeclarationCollector::new(
        &mut state,
        item_tree_package,
        &resolutions,
        MacroCallOrigin::Source,
        None,
    )
    .collect_module_file(root_module, target.root_file, context)
    .with_context(|| format!("collect crate scope for {}", target.name))?;
    Ok(state)
}
