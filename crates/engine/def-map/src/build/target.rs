//! Source loading and scope construction for one configured Cargo target.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use rg_ir_model::{CrateRef, FileId, ModuleRef, PackageSlot};
use rg_item_tree::{ItemTrees, TargetSources, file_declarations, target_sources};
use rg_parse::{CargoTarget, CargoTargetId, TargetFileManifest};
use rg_source::{SourceInput, SourcePresence};
use rg_std::NativeOsString;
use rg_text::Name;
use rg_workspace::{PackageInput, TargetInput, WorkspaceMetadata};

use super::{
    CrateBuildOutput, MacroSourceFileRequest, MacroSourceFileResolution,
    collect::collect_crate,
    crates::KnownModuleFiles,
    plan::CrateBuildPlan,
    resolution_env::BuildResolutionEnv,
    resolve::{resolve_crate, select_prelude},
};
use crate::{CrateData, CrateResolutionEnv, MacroDefinitionEnv};

#[ajika::input(key = CrateRef)]
pub struct CrateDependencies {
    #[input(arc)]
    roots: BTreeMap<Name, ModuleRef>,
}

impl CrateDependencies {
    /// Normalize cycles in the complete target graph before any provider asks for a dependency.
    pub fn write_inputs(
        workspace: &WorkspaceMetadata,
        update: &mut ajika::InputUpdate,
    ) -> ajika::Result<Vec<CrateRef>> {
        let plan = CrateBuildPlan::prepare(workspace);
        update.forget_all(Self::ROOTS);
        let mut order = Vec::with_capacity(plan.crates.len());
        for krate in plan.crates {
            Self::at(krate.crate_ref).set_roots(
                update,
                krate.implicit_roots.into_iter().collect::<BTreeMap<_, _>>(),
            )?;
            order.push(krate.crate_ref);
        }
        Ok(order)
    }
}

impl CrateData {
    /// Settle the target's scopes and generated declarations using captured source inputs.
    /// The caller can lower semantic items from these trees before dropping the build data.
    pub fn build_target(
        db: &rg_source::QueryContext<'_>,
        krate: CrateRef,
        dependencies: &(impl CrateResolutionEnv<Error = rg_source::ReadError> + MacroDefinitionEnv),
    ) -> ajika::Result<(CrateBuildOutput, ItemTrees, TargetFileManifest)> {
        let build = || -> anyhow::Result<_> {
            let config = TargetInput::at(krate);
            let package = PackageInput::at(krate.package);
            let manifest = target_sources(db, krate)?;
            let mut sources = TargetCrateSources {
                db,
                package: krate.package,
                trees: ItemTrees::default(),
                module_paths: BTreeMap::new(),
            };
            sources.acquire(&manifest)?;
            let target = CargoTarget {
                id: CargoTargetId(krate.crate_id.0),
                name: config.name(db)?,
                kind: config.kind(db)?,
                src_path: sources.file_path(manifest.root)?,
                root_file: manifest.root,
            };
            let roots = CrateDependencies::at(krate).roots(db)?;
            let mut state = collect_crate(
                krate,
                &target,
                package.edition(db)?,
                package.cfg(db)?.as_ref().clone(),
                sources.trees(),
                roots
                    .iter()
                    .map(|(name, &module)| (name.clone(), module))
                    .collect(),
                Arc::new(sources.known_module_files()),
            )?;
            state.censor_proc_macro_exports();
            state.prelude = select_prelude(
                &BuildResolutionEnv::new(dependencies, &state),
                &state,
                &package.origin(db)?,
            )?;
            resolve_crate(dependencies, &mut sources, &mut state)?;
            let manifest = TargetFileManifest {
                root: manifest.root,
                files: sources.trees.files().map(|tree| tree.file).collect(),
            };
            Ok((state.finish(&package.name(db)?), sources.trees, manifest))
        };
        build().map_err(|error| {
            for cause in error.chain() {
                if let Some(error) = cause.downcast_ref::<ajika::Error>() {
                    return error.clone();
                }
            }
            ajika::Error::application(error.reallocate_into_boxed_dyn_error_without_backtrace())
        })
    }
}

pub(super) struct TargetCrateSources<'db> {
    db: &'db rg_source::QueryContext<'db>,
    package: PackageSlot,
    trees: ItemTrees,
    module_paths: BTreeMap<NativeOsString, FileId>,
}

impl TargetCrateSources<'_> {
    fn acquire(&mut self, manifest: &TargetSources) -> ajika::Result<()> {
        let files: Vec<_> = manifest
            .files
            .iter()
            .copied()
            .filter(|&file| self.trees.file(file).is_none())
            .collect();
        let trees = self.db.parallel_map(&files, |db, &file| {
            file_declarations(db, self.package, file)
        })?;
        self.trees.extend_files(trees);
        self.module_paths.extend(
            manifest
                .module_paths
                .iter()
                .map(|(path, &file)| (path.clone(), file)),
        );
        Ok(())
    }

    fn file_path(&self, file: FileId) -> ajika::Result<PathBuf> {
        file.path(&self.db.interner())
    }

    fn file_at(&self, path: &Path) -> ajika::Result<Option<FileId>> {
        SourcePresence::at(NativeOsString::from_os_str(path.as_os_str())).resolved_file(self.db)
    }

    pub(super) fn trees(&self) -> &ItemTrees {
        &self.trees
    }

    pub(super) fn known_module_files(&self) -> KnownModuleFiles {
        KnownModuleFiles(self.module_paths.clone())
    }

    pub(super) fn check_cancelled(&self) -> ajika::Result<()> {
        self.db.check_cancelled()
    }

    pub(super) fn load_file(
        &mut self,
        request: &MacroSourceFileRequest,
    ) -> ajika::Result<MacroSourceFileResolution> {
        self.check_cancelled()?;
        let (file, context, answer) = match request {
            MacroSourceFileRequest::Module {
                parent_context,
                module_name,
                path_override,
            } => {
                let module = parent_context.resolve_module_name(
                    module_name,
                    path_override.as_deref(),
                    |path| self.file_at(path).map(|file| file.is_some()),
                )?;
                let Some(module) = module else {
                    return Ok(MacroSourceFileResolution::Missing);
                };
                let (path, context) = module.into_parts();
                let file_id = self
                    .file_at(&path)?
                    .expect("module path was observed as present");
                let context = Arc::new(context);
                let answer = MacroSourceFileResolution::Module {
                    file_id,
                    child_context: Arc::clone(&context),
                };
                (file_id, context, answer)
            }
            MacroSourceFileRequest::Include {
                origin_file,
                module_file_context,
                path,
            } => {
                let origin = self.file_path(*origin_file)?;
                let generated = PackageInput::at(self.package).generated_sources(self.db)?;
                let file = path
                    .resolve(&origin, generated.as_ref())
                    .map(|path| self.file_at(&path))
                    .transpose()?
                    .flatten();
                let Some(file_id) = file else {
                    return Ok(MacroSourceFileResolution::Missing);
                };
                if !SourceInput::is_readable(self.db, file_id)? {
                    return Ok(MacroSourceFileResolution::Missing);
                }
                (
                    file_id,
                    Arc::clone(module_file_context),
                    MacroSourceFileResolution::Include { file_id },
                )
            }
        };
        let manifest = TargetSources::discover_file(self.db, self.package, file, &context)?;
        self.acquire(&manifest)?;
        Ok(answer)
    }
}
