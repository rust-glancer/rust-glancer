//! Generic name lookup over module and body-local scopes.
//!
//! Completion renderers use these facts heavily, but the facts themselves are not completion
//! concepts: they are names visible from an indexed module or lexical body scope.

use rg_def_map::{Path, ScopeNamespace, VisibleScopeDef, VisibleScopeOrigin};
use rg_ir_model::{
    DefId, ModuleRef, SemanticItemRef,
    identity::{DeclarationRef, FunctionRef as AnalysisFunctionRef},
};
use rg_semantic_ir::Documentation;

use crate::api::view::{IndexedSymbolKind, IndexedViewDb};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum NameNamespace {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NameOrigin {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModuleScopeName {
    label: String,
    namespace: NameNamespace,
    origin: NameOrigin,
    declaration: DeclarationRef,
    kind: IndexedSymbolKind,
    documentation: Option<String>,
    function: Option<AnalysisFunctionRef>,
}

impl ModuleScopeName {
    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn namespace(&self) -> NameNamespace {
        self.namespace
    }

    pub(crate) fn origin(&self) -> NameOrigin {
        self.origin
    }

    pub(crate) fn declaration(&self) -> DeclarationRef {
        self.declaration
    }

    pub(crate) fn kind(&self) -> IndexedSymbolKind {
        self.kind
    }

    pub(crate) fn documentation(&self) -> Option<&str> {
        self.documentation.as_deref()
    }

    pub(crate) fn function(&self) -> Option<AnalysisFunctionRef> {
        self.function
    }
}

pub(crate) struct NameLookupView<'a, 'db> {
    analysis: &'a IndexedViewDb<'db>,
}

impl<'a, 'db> NameLookupView<'a, 'db> {
    pub(crate) fn new(analysis: &'a IndexedViewDb<'db>) -> Self {
        Self { analysis }
    }

    pub(crate) fn module_names_for_path(
        &self,
        importing_module: ModuleRef,
        qualifier: &Path,
    ) -> anyhow::Result<Vec<ModuleScopeName>> {
        let resolved = self
            .analysis
            .def_map
            .resolve_path_in_type_namespace(importing_module, qualifier)?;
        let mut names = Vec::new();

        // Qualified module lookup only lists names from modules. Associated items hang off type
        // declarations and are resolved through member-specific views.
        for def in resolved.resolved {
            let DefId::Module(source_module) = def else {
                continue;
            };
            for visible_def in self
                .analysis
                .def_map
                .visible_scope_defs(importing_module, source_module)?
            {
                if let Some(name) = self.module_scope_name(visible_def)? {
                    names.push(name);
                }
            }
        }

        Ok(names)
    }

    pub(crate) fn unqualified_module_names(
        &self,
        module: ModuleRef,
    ) -> anyhow::Result<Vec<ModuleScopeName>> {
        let mut names = Vec::new();
        for visible_def in self
            .analysis
            .def_map
            .visible_unqualified_scope_defs(module)?
        {
            if let Some(name) = self.module_scope_name(visible_def)? {
                names.push(name);
            }
        }
        Ok(names)
    }

    fn module_scope_name(
        &self,
        visible_def: VisibleScopeDef,
    ) -> anyhow::Result<Option<ModuleScopeName>> {
        let declaration = DeclarationRef::from_def(visible_def.def);
        let mut function = None;
        let (kind, documentation) = match visible_def.def {
            DefId::Module(module) => {
                let Some(data) = self.analysis.def_map.module(module)? else {
                    return Ok(None);
                };
                (
                    IndexedSymbolKind::Module,
                    data.docs.as_ref().map(Documentation::text),
                )
            }
            DefId::Local(local_def) => {
                let Some(data) = self.analysis.def_map.local_def(local_def)? else {
                    return Ok(None);
                };
                if let Some(SemanticItemRef::Function(function_ref)) = self
                    .analysis
                    .semantic_ir
                    .semantic_item_for_local_def(local_def)?
                {
                    function = Some(AnalysisFunctionRef::semantic(function_ref));
                }
                (IndexedSymbolKind::from_local_def_kind(data.kind), None)
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
