//! Follow contextual module edges from one target root.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::{Path, PathBuf},
};

use rg_ir_model::{CrateRef, FileId, PackageSlot};
use rg_parse::ModuleFileContext;
use rg_source::{SourceError, SourcePresence};
use rg_std::NativeOsString;
use rg_syntax::ast::{self, HasModuleItem, HasName};
use rg_text::RustEdition;
use rg_workspace::{PackageInput, TargetInput};
use wincode::{SchemaRead, SchemaWrite};

/// Source membership reached through written module edges. The declaration builder extends
/// this membership when resolving module-position macros and includes.
#[derive(Debug, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct TargetSources {
    pub root: FileId,
    pub files: Vec<FileId>,
    /// Candidate spellings used by the module resolver, including `#[path]` and symlinks.
    pub module_paths: BTreeMap<NativeOsString, FileId>,
}

#[ajika::query]
pub fn target_sources(
    db: &rg_source::QueryContext<'_>,
    target: CrateRef,
) -> ajika::Result<TargetSources> {
    let root_path = TargetInput::at(target).root_path(db)?;
    let Some(root) = SourcePresence::at(root_path.clone()).resolved_file(db)? else {
        return Err(ajika::Error::application(SourceError::Io {
            path: native_path(root_path)?,
            source: std::io::ErrorKind::NotFound.into(),
        }));
    };
    let context = ModuleFileContext::for_target_root(&root.path(&db.interner())?);
    TargetSources::discover_file(db, target.package, root, &context)
}

impl TargetSources {
    /// Discover written descendants of a file that module-position expansion reached.
    pub fn discover_file(
        db: &rg_source::QueryContext<'_>,
        package_slot: PackageSlot,
        root: FileId,
        context: &ModuleFileContext,
    ) -> ajika::Result<Self> {
        let edition = PackageInput::at(package_slot).edition(db)?;
        let mut discovery = SourceDiscovery {
            db,
            edition,
            files: BTreeSet::new(),
            module_paths: BTreeMap::new(),
            active: HashSet::new(),
        };
        discovery.visit_file(root, context)?;
        Ok(Self {
            root,
            files: discovery.files.into_iter().collect(),
            module_paths: discovery.module_paths,
        })
    }
}

struct SourceDiscovery<'db> {
    db: &'db rg_source::QueryContext<'db>,
    edition: RustEdition,
    files: BTreeSet<FileId>,
    module_paths: BTreeMap<NativeOsString, FileId>,
    active: HashSet<FileId>,
}

impl SourceDiscovery<'_> {
    fn file_at(&self, path: &Path) -> ajika::Result<Option<FileId>> {
        SourcePresence::at(NativeOsString::from_os_str(path.as_os_str())).resolved_file(self.db)
    }

    fn visit_file(&mut self, file: FileId, context: &ModuleFileContext) -> ajika::Result<()> {
        // The same source can be included under multiple logical directories. Only a visit on
        // the current recursion path is a cycle regardless of its context.
        if !self.active.insert(file) {
            return Ok(());
        }
        self.files.insert(file);
        self.db.check_cancelled()?;
        let syntax = rg_parse::saved_syntax(self.db, file, self.edition)?.tree();
        self.visit_items(context, syntax.items())?;
        self.active.remove(&file);
        Ok(())
    }

    fn visit_items(
        &mut self,
        context: &ModuleFileContext,
        items: impl Iterator<Item = ast::Item>,
    ) -> ajika::Result<()> {
        for item in items {
            let ast::Item::Module(module) = item else {
                continue;
            };
            if let Some(items) = module.item_list() {
                let inline_context = module.name().map_or_else(
                    || context.clone(),
                    |name| {
                        context.descend_inline(
                            rg_text::identifier_text(&name.text()),
                            rg_parse::module_path_override(&module).as_deref(),
                        )
                    },
                );
                self.visit_items(&inline_context, items.items())?;
            } else if let Some(resolution) = context.resolve_module_file(&module, |path| {
                self.file_at(path).map(|resolved| resolved.is_some())
            })? {
                let (path, child_context) = resolution.into_parts();
                let child = self
                    .file_at(&path)?
                    .expect("module path was observed as present");
                self.module_paths
                    .insert(NativeOsString::from_os_str(path.as_os_str()), child);
                self.visit_file(child, &child_context)?;
            }
        }
        Ok(())
    }
}

fn native_path(path: NativeOsString) -> ajika::Result<PathBuf> {
    path.into_os_string()
        .map(PathBuf::from)
        .ok_or_else(|| ajika::Error::contract("invalid native source path"))
}
