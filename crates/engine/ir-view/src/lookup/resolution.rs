//! Generic resolution view from indexed symbols and path facts to declarations.
//!
//! Query modules choose the cursor policy and presentation. This view owns the cross-layer lookup
//! rules that turn paths, declaration refs, and body resolutions into canonical declaration
//! identities.

use rg_body_ir::{BodyResolution, BodyScopeQuery};
use rg_ir_model::{
    BodyBindingRef, BodyRef, DefId, LocalDefRef, ModuleRef, ScopeId, TypePathResolution,
    identity::{DeclarationRef, ExprRef},
};
use rg_ir_storage::{DefMapQuery, ItemStoreQuery, Path, TypePathContext};
use rg_item_tree::FieldKey;
use rg_ty::ItemPathQuery;

use crate::IndexedViewDb;

pub struct ResolutionView<'a, 'db>(&'a IndexedViewDb<'db>);

impl<'a, 'db> ResolutionView<'a, 'db> {
    pub fn new(db: &'a IndexedViewDb<'db>) -> Self {
        Self(db)
    }

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

    pub fn declarations_for_expr(&self, expr: ExprRef) -> anyhow::Result<Vec<DeclarationRef>> {
        let body_ref = expr.body_ir();
        let Some(body) = self.0.body_ir.body_data(body_ref)? else {
            return Ok(Vec::new());
        };
        let Some(expr_data) = body.expr(expr.expr_id()) else {
            return Ok(Vec::new());
        };
        self.declarations_for_body_resolution(Some(body_ref), &expr_data.resolution)
    }

    fn fallback_name_def(&self, local_def: LocalDefRef) -> DeclarationRef {
        DeclarationRef::local_def(local_def)
    }

    fn declarations_for_def(&self, def: DefId) -> anyhow::Result<Vec<DeclarationRef>> {
        match def {
            DefId::Module(module) => Ok(vec![DeclarationRef::module(module)]),
            DefId::Local(local_def) => {
                let declaration = self
                    .declaration_for_local_def(local_def)?
                    .unwrap_or_else(|| self.fallback_name_def(local_def));
                Ok(vec![declaration])
            }
        }
    }

    fn declaration_for_local_def(
        &self,
        local_def: LocalDefRef,
    ) -> anyhow::Result<Option<DeclarationRef>> {
        let Some(item) = ItemStoreQuery::new(self.0).semantic_item_for_local_def(local_def)? else {
            return Ok(None);
        };

        Ok(Some(DeclarationRef::from(item)))
    }

    pub fn declarations_for_use_path(
        &self,
        module: ModuleRef,
        path: &Path,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let mut declarations = Vec::new();
        for def in DefMapQuery::new(self.0)
            .resolve_path(module, path)?
            .resolved
        {
            declarations.extend(self.declarations_for_def(def)?);
        }
        Ok(declarations)
    }

    pub fn declarations_for_body_type_path(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let Some(body) = self.0.body_ir.body_data(body_ref)? else {
            return Ok(Vec::new());
        };
        let resolution = BodyScopeQuery::new(self.0, self.0, body_ref, body)
            .resolve_type_path_in_scope(scope, path)?;

        let declarations = self.declarations_for_body_type_path_resolution(resolution);
        if !declarations.is_empty() {
            return Ok(declarations);
        }

        self.declarations_for_use_path(body.owner_module(), path)
    }

    pub fn declarations_for_body_value_path(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let Some(body) = self.0.body_ir.body_data(body_ref)? else {
            return Ok(Vec::new());
        };
        let (resolution, _) = BodyScopeQuery::new(self.0, self.0, body_ref, body)
            .resolve_value_path_in_scope(scope, path)?;
        self.declarations_for_body_resolution(Some(body_ref), &resolution)
    }

    pub fn declarations_for_body_record_field(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        owner: &Path,
        key: &FieldKey,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let Some(body) = self.0.body_ir.body_data(body_ref)? else {
            return Ok(Vec::new());
        };
        let resolution = BodyScopeQuery::new(self.0, self.0, body_ref, body)
            .resolve_type_path_in_scope(scope, owner)?;

        let (TypePathResolution::SelfType(types) | TypePathResolution::TypeDefs(types)) =
            resolution
        else {
            return Ok(Vec::new());
        };

        let item_query = ItemStoreQuery::new(self.0);
        let mut declarations = Vec::new();
        for ty in types {
            for field in item_query.fields_for_type(ty)? {
                let Some(data) = item_query.field_data(field)? else {
                    continue;
                };
                if data.field.key.as_ref() == Some(key) {
                    declarations.push(DeclarationRef::from(field));
                }
            }
        }
        Ok(declarations)
    }

    pub(crate) fn declarations_for_body_resolution(
        &self,
        body_ref: Option<BodyRef>,
        resolution: &BodyResolution,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        match resolution {
            BodyResolution::Binding(binding) => Ok(body_ref
                .map(|body| BodyBindingRef {
                    body,
                    binding: *binding,
                })
                .map(DeclarationRef::body_binding)
                .into_iter()
                .collect()),
            BodyResolution::Declarations(resolved) => {
                let mut declarations = Vec::new();
                for declaration in resolved {
                    declarations.push(self.canonical_declaration(*declaration)?);
                }
                Ok(declarations)
            }
            BodyResolution::Unknown => Ok(Vec::new()),
        }
    }

    fn declarations_for_body_type_path_resolution(
        &self,
        resolution: TypePathResolution,
    ) -> Vec<DeclarationRef> {
        match resolution {
            TypePathResolution::SelfType(types) | TypePathResolution::TypeDefs(types) => {
                types.into_iter().map(DeclarationRef::from).collect()
            }
            TypePathResolution::TypeAliases(aliases) => {
                aliases.into_iter().map(DeclarationRef::from).collect()
            }
            TypePathResolution::Traits(traits) => {
                traits.into_iter().map(DeclarationRef::from).collect()
            }
            TypePathResolution::Unknown => Vec::new(),
        }
    }
}
