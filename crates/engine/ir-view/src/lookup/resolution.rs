//! Generic resolution view from indexed symbols and path facts to declarations.
//!
//! Query modules choose the cursor policy and presentation. This view owns the cross-layer lookup
//! rules that turn paths, declaration refs, and body resolutions into canonical declaration
//! identities.

use rg_ir_model::Path;
use rg_ir_model::items::FieldKey;
use rg_ir_model::{
    BodyRef, DefId, LocalDefRef, ModuleRef, ScopeId, TypePathResolution,
    identity::{DeclarationRef, ExprRef},
};
use rg_ir_storage::{
    DefMapQuery, DefMapSource, ItemStoreQuery, NameResolutionFilter, TypePathContext,
};
use rg_ty::ItemPathQuery;

use crate::{IndexedViewDb, body::BodyResolutionView, source::IndexedTypePathScope};

/// Turns indexed resolution facts into canonical declaration refs.
pub struct ResolutionView<'a, 'db>(&'a IndexedViewDb<'db>);

impl<'a, 'db> ResolutionView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self(db)
    }

    /// Resolve a type-position path from a signature context.
    pub fn declarations_for_semantic_type_path(
        &self,
        context: TypePathContext,
        path: &Path,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        Ok(ItemPathQuery::new(self.0, self.0)
            .semantic_items_for_type_path(context, path)?
            .into_iter()
            .map(DeclarationRef::from)
            .collect())
    }

    /// Resolve a type-position path from either signature or body source.
    pub fn declarations_for_type_path(
        &self,
        scope: IndexedTypePathScope,
        path: &Path,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        match scope {
            IndexedTypePathScope::Signature(context) => {
                let declarations = self.declarations_for_semantic_type_path(context, path)?;
                if declarations.is_empty() {
                    self.declarations_for_use_path(context.module, path)
                } else {
                    Ok(declarations)
                }
            }
            IndexedTypePathScope::Body(scope) => {
                self.declarations_for_body_type_path(scope.body_ir(), scope.scope_id(), path)
            }
        }
    }

    /// Converts declaration-like refs into the canonical declaration identity exposed to queries.
    ///
    /// DefMap local defs are normalized to their semantic item refs when the item store has a
    /// matching item. Other declaration refs are already canonical and pass through unchanged.
    pub fn canonical_declaration(
        &self,
        declaration: DeclarationRef,
    ) -> anyhow::Result<DeclarationRef> {
        match declaration {
            DeclarationRef::Module(module) => Ok(DeclarationRef::module(module)),
            DeclarationRef::LocalDef(local_def) => Ok(self
                .declaration_for_local_def(local_def)?
                .unwrap_or_else(|| self.fallback_name_def(local_def))),
            DeclarationRef::Item(_)
            | DeclarationRef::Field(_)
            | DeclarationRef::EnumVariant(_)
            | DeclarationRef::BodyBinding(_) => Ok(declaration),
        }
    }

    /// Return declarations already attached to a resolved body expression.
    pub fn declarations_for_expr(&self, expr: ExprRef) -> anyhow::Result<Vec<DeclarationRef>> {
        let body_ref = expr.body_ir();
        let Some(body) = self.0.body_ir.body_data(body_ref)? else {
            return Ok(Vec::new());
        };
        self.canonical_declarations(body.expr_declarations(body_ref, expr.expr_id()))
    }

    /// Keep unresolved DefMap names addressable when no semantic item exists.
    fn fallback_name_def(&self, local_def: LocalDefRef) -> DeclarationRef {
        DeclarationRef::local_def(local_def)
    }

    /// Convert a DefMap result into declaration refs.
    fn declarations_for_def(&self, def: DefId) -> anyhow::Result<Vec<DeclarationRef>> {
        match def {
            DefId::Module(module) => Ok(vec![DeclarationRef::module(module)]),
            DefId::Local(local_def) => {
                let declaration = self
                    .declaration_for_local_def(local_def)?
                    .unwrap_or_else(|| self.fallback_name_def(local_def));
                Ok(vec![declaration])
            }
            DefId::EnumVariant(variant_def) => {
                let item_query = ItemStoreQuery::new(self.0);
                if let Some(variant_def_data) = self
                    .0
                    .def_map_for_origin(variant_def.origin)?
                    .and_then(|def_map| def_map.local_enum_variant(variant_def.local_enum_variant))
                    && let Some(variant_ref) = item_query.enum_variant_ref_for_local_def_index(
                        LocalDefRef {
                            origin: variant_def.origin,
                            local_def: variant_def_data.enum_def,
                        },
                        variant_def_data.index,
                        Some(variant_def_data.name.as_str()),
                    )?
                {
                    Ok(vec![DeclarationRef::EnumVariant(variant_ref)])
                } else {
                    Ok(Vec::new())
                }
            }
        }
    }

    /// Return the semantic declaration for a local def when lowering produced one.
    fn declaration_for_local_def(
        &self,
        local_def: LocalDefRef,
    ) -> anyhow::Result<Option<DeclarationRef>> {
        let Some(item) = ItemStoreQuery::new(self.0).semantic_item_for_local_def(local_def)? else {
            return Ok(None);
        };

        Ok(Some(DeclarationRef::from(item)))
    }

    /// Resolve a normal use-path from a module.
    pub fn declarations_for_use_path(
        &self,
        module: ModuleRef,
        path: &Path,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let mut declarations = Vec::new();
        let def_maps = DefMapQuery::new(self.0);
        for def in def_maps
            .scope_resolver()
            .resolve_path(module, path, NameResolutionFilter::AllNamespaces)?
            .resolved
        {
            declarations.extend(self.declarations_for_def(def)?);
        }
        Ok(declarations)
    }

    /// Resolve a type path inside a body, falling back to item-use resolution when needed.
    pub fn declarations_for_body_type_path(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let Some(body) = self.0.body_ir.body_data(body_ref)? else {
            return Ok(Vec::new());
        };
        let Some(resolution) =
            BodyResolutionView::new(self.0).type_path_resolution(body_ref, scope, path)?
        else {
            return Ok(Vec::new());
        };

        let declarations = self.declarations_for_body_type_path_resolution(resolution);
        if !declarations.is_empty() {
            return Ok(declarations);
        }

        self.declarations_for_use_path(body.owner_module(), path)
    }

    /// Resolve a value path inside a body without considering local binding order.
    pub fn declarations_for_body_value_path(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let declarations = BodyResolutionView::new(self.0)
            .nonlocal_value_path_declarations(body_ref, scope, path)?;
        self.canonical_declarations(declarations)
    }

    /// Resolve a record field key from the body-local owner path.
    pub fn declarations_for_body_record_field(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        owner: &Path,
        key: &FieldKey,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let Some(resolution) =
            BodyResolutionView::new(self.0).type_path_resolution(body_ref, scope, owner)?
        else {
            return Ok(Vec::new());
        };

        let (TypePathResolution::SelfType(ty) | TypePathResolution::TypeDef(ty)) = resolution
        else {
            return Ok(Vec::new());
        };

        let item_query = ItemStoreQuery::new(self.0);
        let mut declarations = Vec::new();
        for field in item_query.fields_for_type(ty)? {
            let Some(data) = item_query.field_data(field)? else {
                continue;
            };
            if data.field.key.as_ref() == Some(key) {
                declarations.push(DeclarationRef::from(field));
            }
        }
        Ok(declarations)
    }

    /// Canonicalize each declaration returned by lower-level lookup.
    fn canonical_declarations(
        &self,
        declarations: Vec<DeclarationRef>,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        declarations
            .into_iter()
            .map(|declaration| self.canonical_declaration(declaration))
            .collect()
    }

    /// Convert a body type-path result into declaration refs.
    fn declarations_for_body_type_path_resolution(
        &self,
        resolution: TypePathResolution,
    ) -> Vec<DeclarationRef> {
        match resolution {
            TypePathResolution::SelfType(ty) | TypePathResolution::TypeDef(ty) => {
                vec![DeclarationRef::from(ty)]
            }
            TypePathResolution::TypeAlias(alias) => vec![DeclarationRef::from(alias)],
            TypePathResolution::Trait(trait_ref) => vec![DeclarationRef::from(trait_ref)],
            TypePathResolution::Unknown => Vec::new(),
        }
    }
}
