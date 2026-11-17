//! Definition reads over an owned map or independently persisted declaration tables.
//!
//! A view is only a route to data. Opening it does not acquire module scopes, documentation,
//! or macro tokens; each accessor observes the output it actually reads.

use std::{collections::HashMap, fmt};

use rg_arena::{Arena, ArenaId as _, StoredArena};
use rg_ir_model::{
    DefMapRef, FileId, ImportId, LocalDefId, LocalEnumVariantId, LocalImplId, ModuleId,
};
use rg_source::QueryContext;
use wincode::{SchemaRead, SchemaWrite};

use super::DefMap;
use crate::{
    ImportData, ItemScope, ItemSource, LocalDefData, LocalEnumVariantData, LocalEnumVariantEntry,
    LocalImplData, MacroDefinitionData, ModuleData, ModuleDocumentation,
};

#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite, Default)]
struct FileDefinitions {
    definitions: Vec<LocalDefId>,
    imports: Vec<ImportId>,
}

/// A completed DefMap split into outputs that lookup can read separately.
///
/// Following a module's parent needs only module metadata. Looking up `api::run` also needs
/// `api`'s bindings, while reading a macro definition needs that macro's tokens. Those are separate
/// outputs, so each operation can leave the other payloads unloaded. Declaration records are
/// grouped with [`StoredArena`], preserving the IDs assigned during collection.
#[derive(Debug, Clone, PartialEq, Eq, SchemaRead, SchemaWrite)]
pub struct StoredDefMap {
    origin: DefMapRef,
    modules: ajika::Output<Arena<ModuleId, ModuleData>>,
    scopes: Arena<ModuleId, ajika::Output<ItemScope>>,
    docs: Arena<ModuleId, Option<ajika::Output<ModuleDocumentation>>>,
    definitions: StoredArena<LocalDefId, LocalDefData>,
    variants: StoredArena<LocalEnumVariantId, LocalEnumVariantData>,
    enum_variants: ajika::Output<HashMap<LocalDefId, Vec<LocalEnumVariantId>>>,
    impls: StoredArena<LocalImplId, LocalImplData>,
    imports: StoredArena<ImportId, ImportData>,
    macros: HashMap<LocalDefId, ajika::Output<MacroDefinitionData>>,
    foreign_blocks: ajika::Output<HashMap<LocalDefId, ItemSource>>,
    files: ajika::Output<HashMap<FileId, FileDefinitions>>,
}

impl StoredDefMap {
    /// Consume the completed map so the aggregate and its independently readable outputs do not
    /// coexist in persistence. Collection still finishes imports and macros before publication.
    pub fn publish(db: &QueryContext<'_>, map: DefMap) -> ajika::Result<Self> {
        let DefMap {
            own_ref: origin,
            data,
        } = map;
        // These indexes select records without opening every group. Build them from the allocated
        // IDs before moving the arenas, so file and enum lookups refer to the same published records.
        let mut files: HashMap<FileId, FileDefinitions> = HashMap::new();
        for (id, definition) in data.local_defs.iter_with_ids() {
            files
                .entry(definition.file_id)
                .or_default()
                .definitions
                .push(id);
        }
        for (id, import) in data.imports.iter_with_ids() {
            files
                .entry(import.source.file_id)
                .or_default()
                .imports
                .push(id);
        }
        let mut enum_variants: HashMap<LocalDefId, Vec<LocalEnumVariantId>> = HashMap::new();
        for (id, variant) in data.local_enum_variants.iter_with_ids() {
            enum_variants.entry(variant.enum_def).or_default().push(id);
        }
        let scopes = data
            .scopes
            .into_vec()
            .into_iter()
            .enumerate()
            .map(|(id, scope)| db.output(("module_scope", id), scope.into()))
            .collect::<ajika::Result<Vec<_>>>()?;
        let docs = data
            .docs
            .into_vec()
            .into_iter()
            .enumerate()
            .map(|(id, docs)| {
                docs.map(|docs| db.output(("module_docs", id), docs.into()))
                    .transpose()
            })
            .collect::<ajika::Result<Vec<_>>>()?;
        let macros = data
            .macro_definitions
            .into_iter()
            .map(|(id, data)| Ok((id, db.output(("macro_definition", id), data.into())?)))
            .collect::<ajika::Result<HashMap<_, _>>>()?;
        Ok(Self {
            origin,
            modules: db.output("module_metadata", data.modules.into())?,
            scopes: Arena::from_vec(scopes),
            docs: Arena::from_vec(docs),
            definitions: StoredArena::publish(db, "definitions", data.local_defs)?,
            variants: StoredArena::publish(db, "enum_variants", data.local_enum_variants)?,
            enum_variants: db.output("enum_variant_ids", enum_variants.into())?,
            impls: StoredArena::publish(db, "local_impls", data.local_impls)?,
            imports: StoredArena::publish(db, "imports", data.imports)?,
            macros,
            foreign_blocks: db.output("foreign_blocks", data.foreign_blocks.into())?,
            files: db.output("definition_files", files.into())?,
        })
    }

    pub fn view<'db>(&'db self, db: &'db QueryContext<'db>) -> DefMapView<'db> {
        DefMapView {
            storage: DefinitionStorage::Stored { db, map: self },
        }
    }
}

#[derive(Clone, Copy)]
enum DefinitionStorage<'db> {
    Local(&'db DefMap),
    Stored {
        db: &'db QueryContext<'db>,
        map: &'db StoredDefMap,
    },
}

/// Read one map's records without requiring lookup to know how the map is stored.
///
/// Builders and body-local readers borrow a completed [`DefMap`]. Target readers use
/// [`StoredDefMap`] and acquire the selected outputs through Ajika. Opening the view itself
/// does not load those records. Whole-map operations must enumerate the data they need explicitly.
#[derive(Clone, Copy)]
pub struct DefMapView<'db> {
    storage: DefinitionStorage<'db>,
}

impl fmt::Debug for DefMapView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let origin = match self.storage {
            DefinitionStorage::Local(map) => map.own_ref(),
            DefinitionStorage::Stored { map, .. } => map.origin,
        };
        f.debug_struct("DefMapView")
            .field("origin", &origin)
            .finish_non_exhaustive()
    }
}

impl<'db> DefMapView<'db> {
    pub(crate) fn local(map: &'db DefMap) -> Self {
        Self {
            storage: DefinitionStorage::Local(map),
        }
    }

    /// Module metadata excludes scopes and documentation, including during whole-target scans.
    pub fn modules(self) -> ajika::Result<&'db [ModuleData]> {
        match self.storage {
            DefinitionStorage::Local(map) => Ok(map.modules()),
            DefinitionStorage::Stored { db, map } => Ok(map.modules.value_ref(db)?.as_slice()),
        }
    }

    pub fn module(self, id: ModuleId) -> ajika::Result<Option<&'db ModuleData>> {
        Ok(self.modules()?.get(id.index()))
    }

    pub fn module_scope(self, id: ModuleId) -> ajika::Result<Option<&'db ItemScope>> {
        match self.storage {
            DefinitionStorage::Local(map) => Ok(map.module_scope(id)),
            DefinitionStorage::Stored { db, map } => map
                .scopes
                .get(id)
                .map(|output| output.value_ref(db))
                .transpose(),
        }
    }

    pub fn module_docs(self, id: ModuleId) -> ajika::Result<Option<&'db ModuleDocumentation>> {
        match self.storage {
            DefinitionStorage::Local(map) => Ok(map.module_docs(id)),
            DefinitionStorage::Stored { db, map } => map
                .docs
                .get(id)
                .and_then(Option::as_ref)
                .map(|output| output.value_ref(db))
                .transpose(),
        }
    }

    pub fn macro_definition(
        self,
        id: LocalDefId,
    ) -> ajika::Result<Option<&'db MacroDefinitionData>> {
        match self.storage {
            DefinitionStorage::Local(map) => Ok(map.macro_definition(id)),
            DefinitionStorage::Stored { db, map } => map
                .macros
                .get(&id)
                .map(|output| output.value_ref(db))
                .transpose(),
        }
    }

    pub fn foreign_block(self, id: LocalDefId) -> ajika::Result<Option<ItemSource>> {
        match self.storage {
            DefinitionStorage::Local(map) => Ok(map.foreign_block(id)),
            DefinitionStorage::Stored { db, map } => {
                Ok(map.foreign_blocks.value_ref(db)?.get(&id).copied())
            }
        }
    }

    /// When a file is selected, filter identities before acquiring their record groups.
    /// The returned IDs are the original arena IDs, even when filtering leaves gaps between them.
    /// Macro-produced definitions retain their original source file, including reused include input.
    pub fn local_defs(
        self,
        file: Option<FileId>,
    ) -> ajika::Result<Vec<(LocalDefId, &'db LocalDefData)>> {
        match self.storage {
            DefinitionStorage::Local(map) => Ok(map
                .local_defs()
                .iter()
                .enumerate()
                .filter(|(_, data)| file.is_none_or(|file| data.file_id == file))
                .map(|(index, data)| (LocalDefId(index), data))
                .collect()),
            DefinitionStorage::Stored { db, map } => match file {
                None => map.definitions.entries(db),
                Some(file) => map
                    .files
                    .value_ref(db)?
                    .get(&file)
                    .into_iter()
                    .flat_map(|file| file.definitions.iter().copied())
                    .map(|id| {
                        Ok((
                            id,
                            map.definitions.get(db, id)?.expect("indexed definition"),
                        ))
                    })
                    .collect(),
            },
        }
    }

    /// Apply the file filter before acquiring import records; `None` enumerates the whole map.
    pub fn imports(self, file: Option<FileId>) -> ajika::Result<Vec<(ImportId, &'db ImportData)>> {
        match self.storage {
            DefinitionStorage::Local(map) => Ok(map
                .imports()
                .iter()
                .enumerate()
                .filter(|(_, data)| file.is_none_or(|file| data.source.file_id == file))
                .map(|(index, data)| (ImportId(index), data))
                .collect()),
            DefinitionStorage::Stored { db, map } => match file {
                None => map.imports.entries(db),
                Some(file) => map
                    .files
                    .value_ref(db)?
                    .get(&file)
                    .into_iter()
                    .flat_map(|file| file.imports.iter().copied())
                    .map(|id| Ok((id, map.imports.get(db, id)?.expect("indexed import"))))
                    .collect(),
            },
        }
    }

    pub fn enum_variants(self, id: LocalDefId) -> ajika::Result<Vec<LocalEnumVariantEntry<'db>>> {
        match self.storage {
            DefinitionStorage::Local(map) => {
                Ok(map.local_enum_variant_entries_for_enum(id).collect())
            }
            DefinitionStorage::Stored { db, map } => map
                .enum_variants
                .value_ref(db)?
                .get(&id)
                .into_iter()
                .flatten()
                .copied()
                .map(|id| {
                    let data = map.variants.get(db, id)?.expect("indexed enum variant");
                    Ok(LocalEnumVariantEntry::new(map.origin, id, data))
                })
                .collect(),
        }
    }

    pub fn local_def(self, id: LocalDefId) -> ajika::Result<Option<&'db LocalDefData>> {
        match self.storage {
            DefinitionStorage::Local(map) => Ok(map.local_def(id)),
            DefinitionStorage::Stored { db, map } => map.definitions.get(db, id),
        }
    }

    pub fn local_impl(self, id: LocalImplId) -> ajika::Result<Option<&'db LocalImplData>> {
        match self.storage {
            DefinitionStorage::Local(map) => Ok(map.local_impl(id)),
            DefinitionStorage::Stored { db, map } => map.impls.get(db, id),
        }
    }

    pub fn local_enum_variant(
        self,
        id: LocalEnumVariantId,
    ) -> ajika::Result<Option<&'db LocalEnumVariantData>> {
        match self.storage {
            DefinitionStorage::Local(map) => Ok(map.local_enum_variant(id)),
            DefinitionStorage::Stored { db, map } => map.variants.get(db, id),
        }
    }

    pub fn import(self, id: ImportId) -> ajika::Result<Option<&'db ImportData>> {
        match self.storage {
            DefinitionStorage::Local(map) => Ok(map.import(id)),
            DefinitionStorage::Stored { db, map } => map.imports.get(db, id),
        }
    }
}
