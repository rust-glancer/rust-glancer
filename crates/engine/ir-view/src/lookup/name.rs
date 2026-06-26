//! Generic name lookup over module and body-local scopes.
//!
//! Completion renderers use these facts heavily, but the facts themselves are not completion
//! concepts: they are names visible from an indexed module or lexical body scope.

use rg_ir_model::items::Documentation;
use rg_ir_model::{
    DefId, FunctionRef, LocalDefRef, ModuleRef, Path, SemanticItemRef, identity::DeclarationRef,
};
use rg_ir_storage::{
    DefMapQuery, ItemStoreQuery, ScopeNamespace, VisibleScopeDef, VisibleScopeOrigin,
};

use crate::{IndexedViewDb, SymbolKind};

/// Namespace where a visible name can be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NameNamespace {
    Types,
    Values,
    Macros,
}

impl From<ScopeNamespace> for NameNamespace {
    fn from(namespace: ScopeNamespace) -> Self {
        match namespace {
            ScopeNamespace::Types => Self::Types,
            ScopeNamespace::Values => Self::Values,
            ScopeNamespace::Macros => Self::Macros,
        }
    }
}

/// Where a visible module-scope name came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameOrigin {
    ModuleScope,
    Prelude,
    ExternRoot,
}

impl From<VisibleScopeOrigin> for NameOrigin {
    fn from(origin: VisibleScopeOrigin) -> Self {
        match origin {
            VisibleScopeOrigin::ModuleScope => Self::ModuleScope,
            VisibleScopeOrigin::Prelude => Self::Prelude,
            VisibleScopeOrigin::ExternRoot => Self::ExternRoot,
        }
    }
}

/// One name visible from a module scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleScopeName {
    label: String,
    namespace: NameNamespace,
    origin: NameOrigin,
    declaration: DeclarationRef,
    kind: SymbolKind,
    documentation: Option<String>,
    function: Option<FunctionRef>,
}

impl ModuleScopeName {
    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn namespace(&self) -> NameNamespace {
        self.namespace
    }

    pub fn origin(&self) -> NameOrigin {
        self.origin
    }

    pub fn declaration(&self) -> DeclarationRef {
        self.declaration
    }

    pub fn kind(&self) -> SymbolKind {
        self.kind
    }

    pub fn documentation(&self) -> Option<&str> {
        self.documentation.as_deref()
    }

    pub fn function(&self) -> Option<FunctionRef> {
        self.function
    }
}

/// Looks up visible names and returns declaration-shaped view facts.
pub struct NameLookupView<'a, 'db> {
    db: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> NameLookupView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self { db }
    }

    /// Return names visible through a module qualifier such as `foo::`.
    pub fn module_names_for_path(
        &self,
        importing_module: ModuleRef,
        qualifier: &Path,
    ) -> anyhow::Result<Vec<ModuleScopeName>> {
        let resolved = DefMapQuery::new(self.db)
            .resolve_path_in_type_namespace(importing_module, qualifier)?;
        let mut names = Vec::new();

        // Qualified module lookup only lists names from modules. Associated items hang off type
        // declarations and are resolved through member-specific views.
        for def in resolved.resolved {
            let DefId::Module(source_module) = def else {
                continue;
            };
            for visible_def in
                DefMapQuery::new(self.db).visible_scope_defs(importing_module, source_module)?
            {
                if let Some(name) = self.module_scope_name(visible_def)? {
                    names.push(name);
                }
            }
        }

        Ok(names)
    }

    /// Return names visible without a qualifier in a module.
    pub fn unqualified_module_names(
        &self,
        module: ModuleRef,
    ) -> anyhow::Result<Vec<ModuleScopeName>> {
        let mut names = Vec::new();
        for visible_def in DefMapQuery::new(self.db).visible_unqualified_scope_defs(module)? {
            if let Some(name) = self.module_scope_name(visible_def)? {
                names.push(name);
            }
        }
        Ok(names)
    }

    /// Convert one DefMap-visible name into the declaration facts exposed by view.
    fn module_scope_name(
        &self,
        visible_def: VisibleScopeDef,
    ) -> anyhow::Result<Option<ModuleScopeName>> {
        let def_maps = DefMapQuery::new(self.db);
        let mut function = None;
        let (declaration, kind, documentation) = match visible_def.def {
            DefId::Module(module) => {
                let Some(data) = def_maps.module_data(module)? else {
                    return Ok(None);
                };
                (
                    DeclarationRef::Module(module),
                    SymbolKind::Module,
                    data.docs.as_ref().map(Documentation::text),
                )
            }
            DefId::Local(local_def_ref) => {
                let Some(data) = def_maps.local_def_data(local_def_ref)? else {
                    return Ok(None);
                };
                if let Some(SemanticItemRef::Function(function_ref)) =
                    ItemStoreQuery::new(self.db).semantic_item_for_local_def(local_def_ref)?
                {
                    function = Some(function_ref);
                }
                (
                    DeclarationRef::LocalDef(local_def_ref),
                    SymbolKind::from_local_def_kind(data.kind),
                    None,
                )
            }
            DefId::EnumVariant(variant_def) => {
                let item_query = ItemStoreQuery::new(self.db);
                if let Some(variant_def_data) = def_maps.local_enum_variant_data(variant_def)?
                    && let Some(variant_ref) = item_query.enum_variant_ref_for_local_def_index(
                        LocalDefRef {
                            origin: variant_def.origin,
                            local_def: variant_def_data.enum_def,
                        },
                        variant_def_data.index,
                        Some(variant_def_data.name.as_str()),
                    )?
                {
                    let docs = item_query
                        .enum_variant_data(variant_ref)?
                        .and_then(|data| data.variant.docs.as_ref().map(Documentation::text));
                    (
                        DeclarationRef::EnumVariant(variant_ref),
                        SymbolKind::EnumVariant,
                        docs,
                    )
                } else {
                    return Ok(None);
                }
            }
        };

        Ok(Some(ModuleScopeName {
            label: visible_def.label,
            namespace: visible_def.namespace.into(),
            origin: visible_def.origin.into(),
            declaration,
            kind,
            documentation,
            function,
        }))
    }
}
