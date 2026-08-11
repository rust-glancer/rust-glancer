//! Editor-facing names from module and declaration-owned generic scopes.
//!
//! The view preserves the semantic facts needed after lookup: namespace and origin, declaration
//! identity, macro syntax kind, documentation, and user-facing attributes. Type/const generic
//! names and lifetime names stay separate because their source syntax differs. The view also
//! exposes the narrower module-name sets required by extern roots and `pub(in ...)` ancestor paths.
//! Completion uses these facts heavily, but ranking and insertion policy do not belong here.

use anyhow::Context as _;
use rg_def_map::{
    DefMapQuery, DefMapSource, MacroDefinitionKind, Namespace, NamespaceSet, VisibleScopeDef,
    VisibleScopeOrigin,
};
use rg_ir_model::{
    DefId, FunctionRef, GenericDefRef, GenericParamRef, ImplRef, ModuleRef, Path, PathRoot,
    SemanticItemRef, identity::DeclarationRef,
};
use rg_item_tree::{BuiltinMacroKind, Documentation, UserFacingAttrs};
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

/// Rust syntax family in which one visible macro definition can be named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroKind {
    Invocation,
    Attribute,
    Derive,
}

impl From<MacroDefinitionKind> for MacroKind {
    fn from(kind: MacroDefinitionKind) -> Self {
        match kind {
            MacroDefinitionKind::Invocation => Self::Invocation,
            MacroDefinitionKind::Attribute => Self::Attribute,
            MacroDefinitionKind::Derive => Self::Derive,
        }
    }
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
    macro_kind: Option<MacroKind>,
    user_facing_attrs: UserFacingAttrs,
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

    /// Whether this macro can be invoked with `!` in ordinary source syntax.
    pub fn is_invocation_macro(&self) -> bool {
        self.macro_kind == Some(MacroKind::Invocation)
    }

    pub fn macro_kind(&self) -> Option<MacroKind> {
        self.macro_kind
    }

    pub fn user_facing_attrs(&self) -> UserFacingAttrs {
        self.user_facing_attrs
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

/// One written lifetime parameter visible from a declaration signature.
///
/// Lifetimes use apostrophe syntax and therefore stay separate from ordinary generic-scope names.
/// The label retains that syntax, for example `'item`, while the target keeps the semantic
/// parameter used by navigation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifetimeScopeName {
    label: String,
    target: GenericParamRef,
}

impl LifetimeScopeName {
    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn target(&self) -> GenericParamRef {
        self.target
    }
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
        if qualifier.root() == PathRoot::Absolute && qualifier.segments().is_empty() {
            let mut names = Vec::new();
            for visible_def in def_maps
                .visible_absolute_root_defs(importing_module)
                .context("read absolute root names")?
            {
                if let Some(name) = self
                    .module_scope_name(importing_module, visible_def)
                    .context("project absolute root name")?
                {
                    names.push(name);
                }
            }
            return Ok(names);
        }
        let resolved = def_maps
            .scope_resolver()
            .resolve_path(importing_module, qualifier, NamespaceSet::TYPES)
            .context("resolve module name qualifier")?;
        let mut names = Vec::new();

        // Qualified module lookup only lists names from modules. Associated items hang off type
        // declarations and are resolved through member-specific views.
        for def in resolved.resolved {
            let DefId::Module(source_module) = def else {
                continue;
            };
            for visible_def in def_maps
                .visible_scope_defs(importing_module, source_module)
                .context("read qualified module names")?
            {
                if let Some(name) = self
                    .module_scope_name(importing_module, visible_def)
                    .context("project qualified module name")?
                {
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
        for visible_def in DefMapQuery::new(self.db)
            .visible_unqualified_scope_defs(module)
            .context("read unqualified module names")?
        {
            if let Some(name) = self
                .module_scope_name(module, visible_def)
                .context("project unqualified module name")?
            {
                names.push(name);
            }
        }
        Ok(names)
    }

    /// Return only dependency crate roots available through the extern prelude.
    pub fn extern_crate_names(&self, module: ModuleRef) -> anyhow::Result<Vec<ModuleScopeName>> {
        Ok(self
            .unqualified_module_names(module)
            .context("read extern crate names")?
            .into_iter()
            .filter(|name| {
                name.origin() == NameOrigin::ExternRoot && name.kind() == SymbolKind::Module
            })
            .collect())
    }

    /// Return only ancestor modules that may continue a `pub(in ...)` path.
    ///
    /// Ordinary path lookup can see siblings and descendants, but Rust restricts `pub(in path)` to
    /// an ancestor of the item. At the cursor below, `api` is valid while another crate-root module
    /// such as `tests` is not:
    ///
    /// ```text
    /// mod api {
    ///     mod detail {
    ///         pub(in crate::ap$0) struct Token;
    ///     }
    /// }
    /// ```
    pub fn visibility_module_names_for_path(
        &self,
        importing_module: ModuleRef,
        qualifier: &Path,
    ) -> anyhow::Result<Vec<ModuleScopeName>> {
        let mut ancestors = Vec::new();
        let mut current = Some(importing_module);
        while let Some(module) = current {
            ancestors.push(module);
            current = self
                .db
                .module_data(module)
                .context("read visibility path ancestor")?
                .and_then(|data| {
                    data.parent.map(|parent| ModuleRef {
                        origin: module.origin,
                        module: parent,
                    })
                });
        }

        Ok(self
            .module_names_for_path(importing_module, qualifier)
            .context("read visibility path names")?
            .into_iter()
            .filter(|name| {
                let DeclarationRef::Module(module) = name.declaration() else {
                    return false;
                };
                ancestors.contains(&module)
            })
            .collect())
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

    /// Return lifetime parameters visible from one declaration, nearest owner first.
    ///
    /// ```text
    /// impl<'ctx> Wrapper<'ctx> {
    ///     fn borrow<'item>(&'item self, value: &'ctx str) {}
    /// }
    /// ```
    ///
    /// From `borrow`, the result is `'item` followed by `'ctx`. Repeated spellings are collapsed
    /// in that order, so a nearer declaration wins if incomplete source contains a shadowing name.
    pub fn lifetime_scope_names(
        &self,
        owner: GenericDefRef,
    ) -> anyhow::Result<Vec<LifetimeScopeName>> {
        let generics = GenericsQuery::new(self.db)
            .generics(owner)
            .context("read lifetime scope parameters")?;
        let mut names = Vec::new();
        let mut seen = UniqueVec::new();
        for param in generics.iter().rev() {
            let GenericParamSource::Lifetime(data) = param.source() else {
                continue;
            };
            let label = data.name.to_string();
            if seen.push(label.clone()) {
                names.push(LifetimeScopeName {
                    label,
                    target: param.param(),
                });
            }
        }
        Ok(names)
    }

    /// Convert one DefMap-visible route into editor-facing declaration facts.
    ///
    /// The importing module matters even after DefMap has selected the name. Re-export attributes
    /// can replace the target's user-facing attributes for that route, and cross-crate lookup must
    /// hide unstable or `doc(hidden)` routes without hiding the same declaration inside its own
    /// crate. Macro kind comes from DefMap metadata rather than a semantic item shape, so a
    /// proc-macro export is not mistaken for its implementation function.
    pub(super) fn module_scope_name(
        &self,
        importing_module: ModuleRef,
        visible_def: VisibleScopeDef,
    ) -> anyhow::Result<Option<ModuleScopeName>> {
        let target_crate = match visible_def.def {
            DefId::Module(module) => module.origin.origin_crate(),
            DefId::Local(local) => local.origin.origin_crate(),
            DefId::EnumVariant(variant) => variant.origin.origin_crate(),
        };
        let mut function = None;
        let mut macro_kind = None;
        let (declaration, kind, documentation, mut user_facing_attrs) = match visible_def.def {
            DefId::Module(module) => {
                let Some(data) = self
                    .db
                    .module_data(module)
                    .context("read visible module data")?
                else {
                    return Ok(None);
                };
                (
                    DeclarationRef::Module(module),
                    SymbolKind::Module,
                    data.docs.as_ref().map(Documentation::text),
                    data.user_facing_attrs,
                )
            }
            DefId::Local(local_def_ref) => {
                let Some(data) = self
                    .db
                    .local_def_data(local_def_ref)
                    .context("read visible local definition data")?
                else {
                    return Ok(None);
                };
                if let Some(SemanticItemRef::Function(function_ref)) = ItemStoreQuery::new(self.db)
                    .semantic_item_for_local_def(local_def_ref)
                    .context("resolve visible function definition")?
                {
                    function = Some(function_ref);
                }
                if let Some(macro_definition) = DefMapQuery::new(self.db)
                    .macro_definition_view(DefId::Local(local_def_ref))
                    .context("read visible macro definition")?
                {
                    // Unknown compiler builtins include derive/attribute-only names. Exclude that
                    // mixed bucket from invocation sites until their precise flavor is retained.
                    if !matches!(
                        macro_definition.data.builtin,
                        Some(BuiltinMacroKind::Unsupported)
                    ) {
                        macro_kind = Some(macro_definition.data.kind.into());
                    }
                }
                (
                    DeclarationRef::LocalDef(local_def_ref),
                    SymbolKind::from_local_def_kind(data.kind),
                    None,
                    data.user_facing_attrs,
                )
            }
            DefId::EnumVariant(variant_def) => {
                let item_query = ItemStoreQuery::new(self.db);
                if let Some(variant_def_data) = self
                    .db
                    .local_enum_variant_data(variant_def)
                    .context("read visible enum variant definition")?
                    && let Some(variant_ref) = item_query
                        .enum_variant_ref_for_local_enum_variant(variant_def, variant_def_data)
                        .context("resolve visible enum variant")?
                {
                    let docs = item_query
                        .enum_variant_data(variant_ref)
                        .context("read visible enum variant data")?
                        .and_then(|data| data.variant.docs.as_ref().map(Documentation::text));
                    (
                        DeclarationRef::EnumVariant(variant_ref),
                        SymbolKind::EnumVariant,
                        docs,
                        variant_def_data.user_facing_attrs,
                    )
                } else {
                    return Ok(None);
                }
            }
        };

        // Attributes on an outer re-export deliberately replace declaration attributes for that
        // route. This lets a crate expose a supported public facade over a hidden implementation,
        // while a hidden re-export remains hidden even when its target is otherwise public.
        let mut imported_attrs = Vec::new();
        for import_ref in &visible_def.attribute_imports {
            if let Some(import) = self
                .db
                .import_data(*import_ref)
                .context("read visible name import attributes")?
            {
                imported_attrs.push(import.user_facing_attrs);
            }
        }
        if !imported_attrs.is_empty() {
            user_facing_attrs = imported_attrs
                .iter()
                .copied()
                .find(|attrs| !attrs.is_doc_hidden() && !attrs.is_unstable())
                .unwrap_or(imported_attrs[0]);
        }

        let importing_crate = importing_module.origin.origin_crate();
        if target_crate != importing_crate
            && (user_facing_attrs.is_doc_hidden() || user_facing_attrs.is_unstable())
        {
            return Ok(None);
        }

        Ok(Some(ModuleScopeName {
            label: visible_def.label,
            namespace: visible_def.namespace.into(),
            origin: visible_def.origin.into(),
            declaration,
            kind,
            documentation,
            function,
            macro_kind,
            user_facing_attrs,
        }))
    }
}
