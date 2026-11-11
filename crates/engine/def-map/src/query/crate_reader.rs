use rg_ir_model::{CrateRef, DefMapRef, FileId, ModuleRef};
use rg_source::ReadError;
use rg_std::ExpectedUnique;

use crate::{CrateData, DefMap, DefMapSource, ModuleOrigin};

/// Acquisition boundary for sources that store one definition payload per configured target.
pub trait CrateDataSource {
    type Error;

    fn crate_data(&self, krate: CrateRef) -> Result<Option<&CrateData>, Self::Error>;
}

impl<S: CrateDataSource + ?Sized> CrateDataSource for &S {
    type Error = S::Error;

    fn crate_data(&self, krate: CrateRef) -> Result<Option<&CrateData>, Self::Error> {
        (**self).crate_data(krate)
    }
}

/// Definition access in a semantic operation. Local construction can use the same reader
/// with its already completed declarations.
pub type DefinitionReader<'db> =
    CrateDefMapReader<&'db (dyn CrateDataSource<Error = ReadError> + Sync)>;

/// Projects a target payload into the semantic definition queries used by features and builders.
#[derive(Clone, Copy)]
pub struct CrateDefMapReader<S>(pub S);

impl<S> std::fmt::Debug for CrateDefMapReader<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CrateDefMapReader")
    }
}

impl<S: CrateDataSource> CrateDefMapReader<S> {
    pub fn crate_data(&self, krate: CrateRef) -> Result<Option<&CrateData>, S::Error> {
        self.0.crate_data(krate)
    }

    pub fn def_map(&self, krate: CrateRef) -> Result<Option<&DefMap>, S::Error> {
        Ok(self.crate_data(krate)?.map(CrateData::def_map))
    }

    /// Find the saved module named by an inline-module path from current syntax.
    ///
    /// The first module is the unique crate module whose definition is `file`. Components such as
    /// `outer::inner` then follow inline child modules declared in that same file. This remains
    /// usable after edits move those modules away from their saved byte ranges, but it deliberately
    /// rejects new, renamed, and ambiguous module paths.
    pub fn module_for_inline_path<T>(
        &self,
        crate_ref: CrateRef,
        file: FileId,
        inline_module_path: &[T],
    ) -> Result<Option<ModuleRef>, S::Error>
    where
        T: AsRef<str>,
    {
        let Some(def_map) = self.def_map(crate_ref)? else {
            return Ok(None);
        };

        let mut file_module = ExpectedUnique::new();
        for (module_index, module) in def_map.modules().iter().enumerate() {
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
                file_module.push(rg_ir_model::ModuleId(module_index));
            }
        }
        let Some(mut module_id) = file_module.into_option() else {
            return Ok(None);
        };

        for component in inline_module_path {
            let Some(module) = def_map.module(module_id) else {
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
            let Some(child_data) = def_map.module(child) else {
                return Ok(None);
            };
            if !matches!(
                child_data.origin,
                ModuleOrigin::Inline {
                    declaration_file,
                    ..
                } if declaration_file == file
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

impl<S: CrateDataSource> DefMapSource for CrateDefMapReader<S> {
    type Error = S::Error;

    fn def_map_for_origin(&self, origin: DefMapRef) -> Result<Option<&DefMap>, Self::Error> {
        let DefMapRef::Crate(krate) = origin else {
            return Ok(None);
        };
        self.def_map(krate)
    }

    fn crate_is_proc_macro(&self, krate: CrateRef) -> Result<bool, Self::Error> {
        Ok(self
            .0
            .crate_data(krate)?
            .is_some_and(CrateData::is_proc_macro))
    }

    fn target_pointer_width(&self, krate: CrateRef) -> Result<Option<u32>, Self::Error> {
        Ok(self
            .0
            .crate_data(krate)?
            .and_then(CrateData::target_pointer_width))
    }

    fn extern_root(&self, krate: CrateRef, name: &str) -> Result<Option<ModuleRef>, Self::Error> {
        Ok(self
            .0
            .crate_data(krate)?
            .and_then(|data| data.extern_prelude().get(name).copied()))
    }

    fn extern_roots(&self, krate: CrateRef) -> Result<Vec<(String, ModuleRef)>, Self::Error> {
        Ok(self
            .0
            .crate_data(krate)?
            .map(|data| {
                data.extern_prelude()
                    .iter()
                    .map(|(name, &module)| (name.to_string(), module))
                    .collect()
            })
            .unwrap_or_default())
    }

    fn prelude_module(&self, krate: CrateRef) -> Result<Option<ModuleRef>, Self::Error> {
        Ok(self.0.crate_data(krate)?.and_then(CrateData::prelude))
    }

    fn item_lookup_dependencies(
        &self,
        krate: CrateRef,
    ) -> Result<rg_std::UniqueVec<CrateRef>, Self::Error> {
        Ok(self
            .crate_data(krate)?
            .map(CrateData::item_lookup_dependencies)
            .unwrap_or_default())
    }

    fn root_module(&self, krate: CrateRef) -> Result<Option<ModuleRef>, Self::Error> {
        Ok(self
            .0
            .crate_data(krate)?
            .and_then(CrateData::root_module)
            .map(|root| ModuleRef::krate(krate, root)))
    }
}
