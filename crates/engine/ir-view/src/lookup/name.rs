//! Generic name lookup over module and body-local scopes.
//!
//! Completion renderers use these facts heavily, but the facts themselves are not completion
//! concepts: they are names visible from an indexed module or lexical body scope.

use anyhow::Context as _;
use rg_def_map::{
    DefMapQuery, DefMapSource, Namespace, NamespaceSet, VisibleScopeDef, VisibleScopeOrigin,
};
use rg_ir_model::{
    DefId, FunctionRef, GenericDefRef, GenericParamRef, ImplRef, ModuleRef, Path, SemanticItemRef,
    identity::DeclarationRef,
};
use rg_item_tree::Documentation;
use rg_semantic_ir::{GenericParamSource, GenericsQuery, ItemStoreQuery};
use rg_std::UniqueVec;

use crate::{IndexedViewDb, SymbolKind};

/// Namespace where a visible name can be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NameNamespace {
    Types,
    Values,
    Macros,
}

/// Namespace selected by source positions that cannot denote a macro.
///
/// Keeping this restriction in the type distinguishes body type/value paths from module scopes,
/// where macro names are a real third possibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueOrTypeNamespace {
    Types,
    Values,
}

impl From<ValueOrTypeNamespace> for NameNamespace {
    fn from(namespace: ValueOrTypeNamespace) -> Self {
        match namespace {
            ValueOrTypeNamespace::Types => Self::Types,
            ValueOrTypeNamespace::Values => Self::Values,
        }
    }
}

impl From<Namespace> for NameNamespace {
    fn from(namespace: Namespace) -> Self {
        match namespace {
            Namespace::Types => Self::Types,
            Namespace::Values => Self::Values,
            Namespace::Macros => Self::Macros,
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

/// Kind of declaration represented by a name from a generic scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenericScopeNameKind {
    /// A type parameter, trait `Self`, or impl `Self`.
    Type,
    /// A named const parameter.
    Const,
}

/// Stable semantic identity behind a name from a generic scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenericScopeNameTarget {
    /// A written type/const parameter, or trait `Self`, represented by `Generics`.
    Param(GenericParamRef),
    /// `Self` in an impl is an alias for the impl's concrete self type, not a generic parameter.
    ImplSelf(ImplRef),
}

/// One type or const name visible from a signature owner.
///
/// This vocabulary includes impl `Self` even though it is not a parameter: source lookup treats it
/// like a type name, while `target` retains the distinction needed to resolve the concrete self
/// type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericScopeName {
    label: String,
    kind: GenericScopeNameKind,
    target: GenericScopeNameTarget,
}

impl GenericScopeName {
    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn kind(&self) -> GenericScopeNameKind {
        self.kind
    }

    pub fn target(&self) -> GenericScopeNameTarget {
        self.target
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
        let def_maps = DefMapQuery::new(self.db);
        let resolved = def_maps.scope_resolver().resolve_path(
            importing_module,
            qualifier,
            NamespaceSet::TYPES,
        )?;
        let mut names = Vec::new();

        // Qualified module lookup only lists names from modules. Associated items hang off type
        // declarations and are resolved through member-specific views.
        for def in resolved.resolved {
            let DefId::Module(source_module) = def else {
                continue;
            };
            for visible_def in def_maps.visible_scope_defs(importing_module, source_module)? {
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

    /// Return named parameters visible from one declaration, including inherited owner params.
    ///
    /// Anonymous argument-position `impl Trait` parameters have no source name and lifetimes use
    /// different syntax, so neither is part of ordinary identifier completion. Impl `Self` is
    /// added explicitly because it resolves through the impl context rather than `Generics`.
    /// Thus, for `impl<T> Wrapper<T> { fn map<U>() {} }`, the method owner exposes `U`, inherited
    /// `T`, and `Self` through one lookup.
    pub fn generic_scope_names(
        &self,
        owner: GenericDefRef,
    ) -> anyhow::Result<Vec<GenericScopeName>> {
        let generics = GenericsQuery::new(self.db)
            .generics(owner)
            .context("read generic scope parameters")?;
        let mut names = Vec::new();
        let mut seen = UniqueVec::new();

        for param in generics.iter().rev() {
            let (label, kind) = match param.source() {
                GenericParamSource::Type(data) => {
                    (data.name.to_string(), GenericScopeNameKind::Type)
                }
                GenericParamSource::Const(data) => {
                    (data.name.to_string(), GenericScopeNameKind::Const)
                }
                GenericParamSource::TraitSelf => ("Self".to_string(), GenericScopeNameKind::Type),
                GenericParamSource::Lifetime(_) | GenericParamSource::ArgumentImplTrait(_) => {
                    continue;
                }
            };
            if !seen.push((label.clone(), kind)) {
                continue;
            }
            names.push(GenericScopeName {
                label,
                kind,
                target: GenericScopeNameTarget::Param(param.param()),
            });
        }

        let context = ItemStoreQuery::new(self.db)
            .type_path_context_for_generic_def(owner)
            .context("read generic scope type-path context")?;
        if let Some(impl_ref) = context.and_then(|context| context.impl_ref)
            && seen.push(("Self".to_string(), GenericScopeNameKind::Type))
        {
            names.push(GenericScopeName {
                label: "Self".to_string(),
                kind: GenericScopeNameKind::Type,
                target: GenericScopeNameTarget::ImplSelf(impl_ref),
            });
        }

        Ok(names)
    }

    /// Convert one DefMap-visible name into the declaration facts exposed by view.
    fn module_scope_name(
        &self,
        visible_def: VisibleScopeDef,
    ) -> anyhow::Result<Option<ModuleScopeName>> {
        let mut function = None;
        let (declaration, kind, documentation) = match visible_def.def {
            DefId::Module(module) => {
                let Some(data) = self.db.module_data(module)? else {
                    return Ok(None);
                };
                (
                    DeclarationRef::Module(module),
                    SymbolKind::Module,
                    data.docs.as_ref().map(Documentation::text),
                )
            }
            DefId::Local(local_def_ref) => {
                let Some(data) = self.db.local_def_data(local_def_ref)? else {
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
                if let Some(variant_def_data) = self.db.local_enum_variant_data(variant_def)?
                    && let Some(variant_ref) = item_query
                        .enum_variant_ref_for_local_enum_variant(variant_def, variant_def_data)?
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
