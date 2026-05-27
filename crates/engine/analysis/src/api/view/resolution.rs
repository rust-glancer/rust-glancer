//! Generic resolution view from indexed symbols and path facts to declarations.
//!
//! Query modules choose the cursor policy and presentation. This view owns the cross-layer lookup
//! rules that turn paths, declaration refs, and body resolutions into canonical declaration
//! identities.

use rg_body_ir::{BodyResolution, BodyTypePathResolution};
use rg_def_map::Path;
use rg_ir_model::{
    BodyBindingRef, BodyRef, DefId, LocalDefRef, ModuleRef, ResolvedDeclarationRef, ScopeId,
    identity::{DeclarationRef, DeclarationRefRepr, ExprRef, NameDefRef, NameDefRefRepr},
};

use crate::api::view::IndexedViewDb;

pub(crate) struct ResolutionView<'a, 'db>(&'a IndexedViewDb<'db>);

impl<'a, 'db> ResolutionView<'a, 'db> {
    pub(crate) fn new(analysis: &'a IndexedViewDb<'db>) -> Self {
        Self(analysis)
    }

    pub(crate) fn declarations_for_semantic_type_path(
        &self,
        context: rg_semantic_ir::TypePathContext,
        path: &Path,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        Ok(self
            .0
            .semantic_ir
            .semantic_items_for_type_path(&self.0.def_map, context, path)?
            .into_iter()
            .map(|item| DeclarationRef::semantic(item.into()))
            .collect())
    }

    pub(crate) fn declarations_for_declaration(
        &self,
        declaration: DeclarationRef,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        match declaration.repr() {
            DeclarationRefRepr::Module(module) => self.declarations_for_def(DefId::Module(module)),
            DeclarationRefRepr::NameDef(name_def) => match name_def.repr() {
                NameDefRefRepr::DefMapLocal(local_def) => {
                    self.declarations_for_def(DefId::Local(local_def))
                }
            },
            DeclarationRefRepr::Item(_)
            | DeclarationRefRepr::Function(_)
            | DeclarationRefRepr::Field(_)
            | DeclarationRefRepr::EnumVariant(_)
            | DeclarationRefRepr::Binding(_)
            | DeclarationRefRepr::Impl(_) => Ok(vec![declaration]),
        }
    }

    pub(crate) fn declarations_for_expr(
        &self,
        expr: ExprRef,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
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
        DeclarationRef::name_def(NameDefRef::def_map_local(local_def))
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
        let Some(item) = self.0.semantic_ir.semantic_item_for_local_def(local_def)? else {
            return Ok(None);
        };

        Ok(Some(DeclarationRef::semantic(item.into())))
    }

    pub(crate) fn declarations_for_use_path(
        &self,
        module: ModuleRef,
        path: &Path,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let mut declarations = Vec::new();
        for def in self.0.def_map.resolve_path(module, path)?.resolved {
            declarations.extend(self.declarations_for_def(def)?);
        }
        Ok(declarations)
    }

    pub(crate) fn declarations_for_body_type_path(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let resolution = self.0.body_ir.resolve_type_path_in_scope(
            &self.0.def_map,
            &self.0.semantic_ir,
            body_ref,
            scope,
            path,
        )?;

        let declarations = self.declarations_for_body_type_path_resolution(resolution);
        if !declarations.is_empty() {
            return Ok(declarations);
        }

        let Some(body) = self.0.body_ir.body_data(body_ref)? else {
            return Ok(Vec::new());
        };
        self.declarations_for_use_path(body.owner_module(), path)
    }

    pub(crate) fn declarations_for_body_value_path(
        &self,
        body_ref: BodyRef,
        scope: ScopeId,
        path: &Path,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        let (resolution, _) = self.0.body_ir.resolve_value_path_in_scope(
            &self.0.def_map,
            &self.0.semantic_ir,
            body_ref,
            scope,
            path,
        )?;
        self.declarations_for_body_resolution(Some(body_ref), &resolution)
    }

    pub(crate) fn declarations_for_body_resolution(
        &self,
        body_ref: Option<BodyRef>,
        resolution: &BodyResolution,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        match resolution {
            BodyResolution::Local(binding) => Ok(body_ref
                .map(|body| BodyBindingRef {
                    body,
                    binding: *binding,
                })
                .map(DeclarationRef::body_binding)
                .into_iter()
                .collect()),
            BodyResolution::Declaration(resolved)
            | BodyResolution::Field(resolved)
            | BodyResolution::Function(resolved)
            | BodyResolution::Method(resolved)
            | BodyResolution::EnumVariant(resolved) => {
                let mut declarations = Vec::new();
                for declaration in resolved {
                    declarations.extend(self.declarations_for_resolved_declaration(*declaration)?);
                }
                Ok(declarations)
            }
            BodyResolution::Unknown => Ok(Vec::new()),
        }
    }

    fn declarations_for_resolved_declaration(
        &self,
        declaration: ResolvedDeclarationRef,
    ) -> anyhow::Result<Vec<DeclarationRef>> {
        match declaration {
            ResolvedDeclarationRef::Def(def) => self.declarations_for_def(def),
            ResolvedDeclarationRef::Semantic(declaration) => {
                Ok(vec![DeclarationRef::semantic(declaration)])
            }
            ResolvedDeclarationRef::Body(declaration) => {
                Ok(vec![DeclarationRef::body(declaration)])
            }
        }
    }

    fn declarations_for_body_type_path_resolution(
        &self,
        resolution: BodyTypePathResolution,
    ) -> Vec<DeclarationRef> {
        match resolution {
            BodyTypePathResolution::BodyLocal(item) => vec![DeclarationRef::body_item(item)],
            BodyTypePathResolution::SelfType(types) | BodyTypePathResolution::TypeDefs(types) => {
                types
                    .into_iter()
                    .map(|ty| DeclarationRef::semantic(ty.into()))
                    .collect()
            }
            BodyTypePathResolution::Traits(traits) => traits
                .into_iter()
                .map(|ty| DeclarationRef::semantic(ty.into()))
                .collect(),
            BodyTypePathResolution::Primitive(_) | BodyTypePathResolution::Unknown => Vec::new(),
        }
    }
}
