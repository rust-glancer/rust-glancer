use std::fmt;

use rg_ir_model::{CrateRef, DefMapRef, FileId, ModuleId, ModuleRef};
use rg_source::ReadError;
use rg_std::{ExpectedUnique, UniqueVec};

use crate::{CrateMetadata, DefMapSource, DefMapView, ModuleOrigin};

/// Read crate facts separately from the crate's namespaces.
///
/// A path renderer can ask for the crate name through `crate_metadata`. Namespace lookup opens a
/// [`DefMapView`] through `definitions`, then selects the module bindings or declarations it needs.
pub trait CrateDefinitionSource {
    fn crate_metadata(&self, krate: CrateRef) -> Result<Option<&CrateMetadata>, ReadError>;
    fn definitions(&self, krate: CrateRef) -> Result<Option<DefMapView<'_>>, ReadError>;
}

impl<S: CrateDefinitionSource + ?Sized> CrateDefinitionSource for &S {
    fn crate_metadata(&self, krate: CrateRef) -> Result<Option<&CrateMetadata>, ReadError> {
        (**self).crate_metadata(krate)
    }

    fn definitions(&self, krate: CrateRef) -> Result<Option<DefMapView<'_>>, ReadError> {
        (**self).definitions(krate)
    }
}

/// Definition access in a semantic operation. Local construction can use the same reader
/// with its already completed declarations.
pub type DefinitionReader<'db> = CrateDefMapReader<&'db (dyn CrateDefinitionSource + Sync)>;

/// Routes semantic definition queries to crate metadata and independently readable declarations.
#[derive(Clone, Copy)]
pub struct CrateDefMapReader<S>(pub S);

impl<S> fmt::Debug for CrateDefMapReader<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CrateDefMapReader")
    }
}

impl<S: CrateDefinitionSource> CrateDefMapReader<S> {
    pub fn crate_metadata(&self, krate: CrateRef) -> Result<Option<&CrateMetadata>, ReadError> {
        self.0.crate_metadata(krate)
    }

    pub fn definitions(&self, krate: CrateRef) -> Result<Option<DefMapView<'_>>, ReadError> {
        self.0.definitions(krate)
    }

    /// Find a module by its definition file and inline-module path.
    ///
    /// The first module is the unique crate module whose definition is `file`. Components such as
    /// `outer::inner` then follow inline child modules declared in that same file. A file shared
    /// by multiple module declarations, or an ambiguous path, has no unique answer.
    pub fn module_for_inline_path<T>(
        &self,
        crate_ref: CrateRef,
        file: FileId,
        inline_module_path: &[T],
    ) -> Result<Option<ModuleRef>, ReadError>
    where
        T: AsRef<str>,
    {
        let Some(def_map) = self.definitions(crate_ref)? else {
            return Ok(None);
        };

        let mut file_module = ExpectedUnique::new();
        for (module_index, module) in def_map.modules()?.iter().enumerate() {
            let owns_definition_file = match module.origin {
                ModuleOrigin::Root { file_id } => file_id == file,
                ModuleOrigin::OutOfLine {
                    definition_file: Some(definition_file),
                    ..
                } => definition_file == file,
                ModuleOrigin::Synthetic { .. }
                | ModuleOrigin::Inline { .. }
                | ModuleOrigin::OutOfLine {
                    definition_file: None,
                    ..
                } => false,
            };
            if owns_definition_file {
                file_module.push(ModuleId(module_index));
            }
        }
        let Some(mut module_id) = file_module.into_option() else {
            return Ok(None);
        };

        for component in inline_module_path {
            let Some(module) = def_map.module(module_id)? else {
                return Ok(None);
            };
            let mut child = ExpectedUnique::new();
            for (name, child_id) in &module.children {
                if name == component.as_ref() {
                    child.push(*child_id);
                }
            }
            let Some(child) = child.into_option() else {
                return Ok(None);
            };
            let Some(child_data) = def_map.module(child)? else {
                return Ok(None);
            };
            if !matches!(
                child_data.origin,
                ModuleOrigin::Inline {
                    declaration_source,
                    ..
                } if declaration_source.file_id == file
            ) {
                return Ok(None);
            }
            module_id = child;
        }

        Ok(Some(ModuleRef {
            origin: DefMapRef::Crate(crate_ref),
            module: module_id,
        }))
    }
}

impl<S: CrateDefinitionSource> DefMapSource for CrateDefMapReader<S> {
    fn definitions_for_origin(
        &self,
        origin: DefMapRef,
    ) -> Result<Option<DefMapView<'_>>, ReadError> {
        let DefMapRef::Crate(krate) = origin else {
            return Ok(None);
        };
        self.definitions(krate)
    }

    fn crate_is_proc_macro(&self, krate: CrateRef) -> Result<bool, ReadError> {
        Ok(self
            .0
            .crate_metadata(krate)?
            .is_some_and(CrateMetadata::is_proc_macro))
    }

    fn target_pointer_width(&self, krate: CrateRef) -> Result<Option<u32>, ReadError> {
        Ok(self
            .0
            .crate_metadata(krate)?
            .and_then(CrateMetadata::target_pointer_width))
    }

    fn extern_root(&self, krate: CrateRef, name: &str) -> Result<Option<ModuleRef>, ReadError> {
        Ok(self
            .0
            .crate_metadata(krate)?
            .and_then(|data| data.extern_prelude().get(name).copied()))
    }

    fn extern_roots(&self, krate: CrateRef) -> Result<Vec<(String, ModuleRef)>, ReadError> {
        Ok(self
            .0
            .crate_metadata(krate)?
            .map(|data| {
                data.extern_prelude()
                    .iter()
                    .map(|(name, &module)| (name.to_string(), module))
                    .collect()
            })
            .unwrap_or_default())
    }

    fn prelude_module(&self, krate: CrateRef) -> Result<Option<ModuleRef>, ReadError> {
        Ok(self
            .0
            .crate_metadata(krate)?
            .and_then(CrateMetadata::prelude))
    }

    fn item_lookup_dependencies(&self, krate: CrateRef) -> Result<UniqueVec<CrateRef>, ReadError> {
        Ok(self
            .crate_metadata(krate)?
            .map(CrateMetadata::item_lookup_dependencies)
            .unwrap_or_default())
    }

    fn root_module(&self, krate: CrateRef) -> Result<Option<ModuleRef>, ReadError> {
        Ok(self
            .0
            .crate_metadata(krate)?
            .map(|data| ModuleRef::krate(krate, data.root_module())))
    }
}
