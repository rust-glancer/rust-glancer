//! Type-path resolution with body-local scope awareness.
//!
//! Semantic IR can resolve module items, but body-local structs live in lexical scopes. This
//! resolver checks those scopes first and then falls back to the semantic/def-map context.

use rg_def_map::{DefMapReadTxn, ModuleRef, Path, PathSegment};
use rg_item_tree::{GenericArg, TypePath, TypeRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{FunctionRef, SemanticIrReadTxn, TypeDefRef, TypePathContext};

use crate::{
    BodyItemKind,
    ir::body::BodyData,
    ir::ids::{BodyItemId, BodyItemRef, BodyRef, ScopeId},
    ir::item::BodyItemOwner,
    ir::resolved::BodyTypePathResolution,
    ir::ty::{BodyGenericArg, BodyLocalNominalTy, BodyTy},
};

use super::{
    method::{local_impl_applies_to_receiver, local_impl_self_subst_for_impl},
    ty::{
        TypeSubst, local_type_subst, subst_from_generics, substitute_type_param,
        ty_from_body_resolution, ty_from_type_ref_in_context, type_ref_is_self,
    },
};

pub(super) struct BodyTypePathResolver<'query, 'db, 'body> {
    def_map: &'query DefMapReadTxn<'db>,
    semantic_ir: &'query SemanticIrReadTxn<'db>,
    body_ref: BodyRef,
    body: &'body BodyData,
}

impl<'query, 'db, 'body> BodyTypePathResolver<'query, 'db, 'body> {
    pub(super) fn new(
        def_map: &'query DefMapReadTxn<'db>,
        semantic_ir: &'query SemanticIrReadTxn<'db>,
        body_ref: BodyRef,
        body: &'body BodyData,
    ) -> Self {
        Self {
            def_map,
            semantic_ir,
            body_ref,
            body,
        }
    }

    pub(super) fn resolve_in_scope(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<BodyTypePathResolution, PackageStoreError> {
        // Body-local type names shadow module items inside their lexical scope. Qualified paths
        // skip this branch because local items cannot be named through module paths.
        if let Some(name) = path.single_name() {
            if let Some(item) = self.resolve_local_type_item(scope, name) {
                return Ok(BodyTypePathResolution::BodyLocal(BodyItemRef {
                    body: self.body_ref,
                    item,
                }));
            }
        }
        if let Some(item) = self.resolve_local_associated_type_item(scope, path)? {
            return Ok(BodyTypePathResolution::BodyLocal(item));
        }

        let context = self.context_for_function(self.body.owner, self.body.owner_module)?;
        let resolution = self
            .semantic_ir
            .resolve_type_path(self.def_map, context, path)?;
        Ok(BodyTypePathResolution::from(resolution))
    }

    pub(super) fn ty_from_type_ref_in_scope(
        &self,
        ty: &TypeRef,
        scope: ScopeId,
    ) -> Result<BodyTy, PackageStoreError> {
        self.ty_from_type_ref_in_scope_with_subst(ty, scope, &TypeSubst::new())
    }

    pub(super) fn ty_from_type_ref_in_scope_with_subst(
        &self,
        ty: &TypeRef,
        scope: ScopeId,
        subst: &TypeSubst,
    ) -> Result<BodyTy, PackageStoreError> {
        // Path types are the only type syntax we resolve structurally today. Other forms stay as
        // syntax unless they have a cheap built-in representation such as `()` or `!`.
        match ty {
            TypeRef::Path(type_path) => {
                let path = Path::from_type_path(type_path);
                if let Some(ty) = substitute_type_param(&path, subst) {
                    return Ok(ty);
                }

                let args = self.generic_args_from_type_path_in_scope(type_path, scope, subst)?;
                if let Some(ty) =
                    self.ty_from_local_associated_type_path(type_path, &path, scope, subst, &args)?
                {
                    return Ok(ty);
                }

                let resolution = self.resolve_in_scope(scope, &path)?;
                if let BodyTypePathResolution::BodyLocal(item_ref) = resolution {
                    let Some(item) = self.body.local_item(item_ref.item) else {
                        return Ok(BodyTy::Unknown);
                    };
                    return match item.kind {
                        BodyItemKind::Struct | BodyItemKind::Enum | BodyItemKind::Union => {
                            Ok(ty_from_body_resolution(
                                BodyTypePathResolution::BodyLocal(item_ref),
                                BodyTy::Syntax(ty.clone()),
                                args,
                            ))
                        }
                        BodyItemKind::TypeAlias => {
                            if let Some(aliased_ty) = item.aliased_ty() {
                                let mut alias_subst = subst.clone();
                                if let Some(generics) = item.generic_params() {
                                    alias_subst.extend(subst_from_generics(generics, &args));
                                }
                                self.ty_from_type_ref_in_scope_with_subst(
                                    aliased_ty,
                                    item.scope,
                                    &alias_subst,
                                )
                            } else {
                                Ok(BodyTy::Syntax(ty.clone()))
                            }
                        }
                        BodyItemKind::Trait => Ok(BodyTy::Syntax(ty.clone())),
                    };
                }

                Ok(ty_from_body_resolution(
                    resolution,
                    BodyTy::Syntax(ty.clone()),
                    args,
                ))
            }
            _ => self.ty_from_type_ref_in_context(
                ty,
                self.context_for_function(self.body.owner, self.body.owner_module)?,
                subst,
            ),
        }
    }

    pub(super) fn local_item_from_type_ref_in_scope(
        &self,
        ty: &TypeRef,
        scope: ScopeId,
    ) -> Result<Option<BodyItemRef>, PackageStoreError> {
        let TypeRef::Path(type_path) = ty else {
            return Ok(None);
        };
        let path = Path::from_type_path(type_path);
        match self.resolve_in_scope(scope, &path)? {
            BodyTypePathResolution::BodyLocal(item) => Ok(self
                .body
                .local_item(item.item)
                .filter(|data| {
                    matches!(
                        data.kind,
                        BodyItemKind::Struct | BodyItemKind::Enum | BodyItemKind::Union
                    )
                })
                .map(|_| item)),
            BodyTypePathResolution::SelfType(_)
            | BodyTypePathResolution::TypeDefs(_)
            | BodyTypePathResolution::Traits(_)
            | BodyTypePathResolution::Unknown => Ok(None),
        }
    }

    pub(super) fn ty_from_type_ref_for_function_with_subst(
        &self,
        ty: &TypeRef,
        function: FunctionRef,
        subst: &TypeSubst,
    ) -> Result<BodyTy, PackageStoreError> {
        self.ty_from_type_ref_in_context_with_subst(
            ty,
            self.context_for_function(function, self.body.owner_module)?,
            subst,
        )
    }

    fn ty_from_type_ref_in_context(
        &self,
        ty: &TypeRef,
        context: TypePathContext,
        subst: &TypeSubst,
    ) -> Result<BodyTy, PackageStoreError> {
        self.ty_from_type_ref_in_context_with_subst(ty, context, subst)
    }

    pub(super) fn ty_from_type_ref_in_context_with_subst(
        &self,
        ty: &TypeRef,
        context: TypePathContext,
        subst: &TypeSubst,
    ) -> Result<BodyTy, PackageStoreError> {
        ty_from_type_ref_in_context(
            self.def_map,
            self.semantic_ir,
            ty,
            context,
            BodyTy::Syntax(ty.clone()),
            subst,
        )
    }

    pub(super) fn self_tys_for_function(
        &self,
        function: FunctionRef,
    ) -> Result<Vec<TypeDefRef>, PackageStoreError> {
        // `self` parameters and explicit `Self` annotations need the enclosing impl owner, not
        // just the owner module. Semantic IR owns that function-to-owner mapping.
        let Some(impl_ref) = self
            .context_for_function(function, self.body.owner_module)?
            .impl_ref
        else {
            return Ok(Vec::new());
        };

        Ok(self
            .semantic_ir
            .impl_data(impl_ref)?
            .map(|impl_data| impl_data.resolved_self_tys.clone())
            .unwrap_or_default())
    }

    fn context_for_function(
        &self,
        function: FunctionRef,
        fallback_module: ModuleRef,
    ) -> Result<TypePathContext, PackageStoreError> {
        Ok(self
            .semantic_ir
            .type_path_context_for_function(function)?
            .unwrap_or_else(|| TypePathContext::module(fallback_module)))
    }

    fn resolve_local_type_item(&self, scope: ScopeId, name: &str) -> Option<BodyItemId> {
        self.body.walk_scopes(scope, |scope_data| {
            for item in scope_data.local_items.iter().rev() {
                let Some(item_data) = self.body.local_item(*item) else {
                    continue;
                };
                if item_data.name == name {
                    return Some(*item);
                }
            }

            None
        })
    }

    fn resolve_local_associated_type_item(
        &self,
        scope: ScopeId,
        path: &Path,
    ) -> Result<Option<BodyItemRef>, PackageStoreError> {
        let Some((prefix, name)) = split_associated_path(path) else {
            return Ok(None);
        };

        let prefix_resolution = self.resolve_in_scope(scope, &prefix)?;
        for ty in self.local_nominal_tys_from_resolution(prefix_resolution) {
            if let Some(item) = self.local_associated_type_item_for_type(&ty, name)? {
                return Ok(Some(item));
            }
        }

        Ok(None)
    }

    fn ty_from_local_associated_type_path(
        &self,
        type_path: &TypePath,
        path: &Path,
        scope: ScopeId,
        subst: &TypeSubst,
        args: &[BodyGenericArg],
    ) -> Result<Option<BodyTy>, PackageStoreError> {
        let Some((_, name)) = split_associated_path(path) else {
            return Ok(None);
        };
        let Some(prefix_ty_ref) = prefix_type_ref(type_path) else {
            return Ok(None);
        };
        let prefix_ty = self.ty_from_type_ref_in_scope_with_subst(&prefix_ty_ref, scope, subst)?;

        for ty in prefix_ty.local_nominals() {
            let Some(item_ref) = self.local_associated_type_item_for_type(ty, name)? else {
                continue;
            };
            return self
                .ty_from_local_associated_type_item(item_ref, ty, args)
                .map(Some);
        }

        Ok(None)
    }

    fn local_associated_type_item_for_type(
        &self,
        ty: &BodyLocalNominalTy,
        name: &str,
    ) -> Result<Option<BodyItemRef>, PackageStoreError> {
        for impl_id in self
            .body
            .inherent_impls_for_local_type(self.body_ref, ty.item)
        {
            let Some(impl_data) = self.body.local_impl(impl_id) else {
                continue;
            };
            if !local_impl_applies_to_receiver(
                self.def_map,
                self.semantic_ir,
                self.body_ref,
                self.body,
                impl_data,
                ty,
            )? {
                continue;
            }

            for item in &impl_data.types {
                let Some(item_data) = self.body.local_item(*item) else {
                    continue;
                };
                if item_data.name == name {
                    return Ok(Some(BodyItemRef {
                        body: self.body_ref,
                        item: *item,
                    }));
                }
            }
        }

        Ok(None)
    }

    fn ty_from_local_associated_type_item(
        &self,
        item_ref: BodyItemRef,
        receiver_ty: &BodyLocalNominalTy,
        args: &[BodyGenericArg],
    ) -> Result<BodyTy, PackageStoreError> {
        let Some(item) = self.body.local_item(item_ref.item) else {
            return Ok(BodyTy::Unknown);
        };
        let Some(aliased_ty) = item.aliased_ty() else {
            return Ok(BodyTy::Unknown);
        };
        if type_ref_is_self(aliased_ty) {
            return Ok(BodyTy::LocalNominal(vec![receiver_ty.clone()]));
        }

        let BodyItemOwner::LocalImpl(impl_id) = item.owner else {
            return self.ty_from_type_ref_in_scope_with_subst(
                aliased_ty,
                item.scope,
                &TypeSubst::new(),
            );
        };
        let Some(impl_data) = self.body.local_impl(impl_id) else {
            return Ok(BodyTy::Unknown);
        };

        let mut alias_subst = local_type_subst(self.body, receiver_ty);
        alias_subst.extend(local_impl_self_subst_for_impl(impl_data, receiver_ty));
        if let Some(generics) = item.generic_params() {
            alias_subst.extend(subst_from_generics(generics, args));
        }
        self.ty_from_type_ref_in_scope_with_subst(aliased_ty, item.scope, &alias_subst)
    }

    fn local_nominal_tys_from_resolution(
        &self,
        resolution: BodyTypePathResolution,
    ) -> Vec<BodyLocalNominalTy> {
        match resolution {
            BodyTypePathResolution::BodyLocal(item) => self
                .body
                .local_item(item.item)
                .filter(|data| data.is_nominal_type())
                .map(|_| vec![BodyLocalNominalTy::bare(item)])
                .unwrap_or_default(),
            BodyTypePathResolution::SelfType(_)
            | BodyTypePathResolution::TypeDefs(_)
            | BodyTypePathResolution::Traits(_)
            | BodyTypePathResolution::Unknown => Vec::new(),
        }
    }

    fn generic_args_from_type_path_in_scope(
        &self,
        type_path: &rg_item_tree::TypePath,
        scope: ScopeId,
        subst: &TypeSubst,
    ) -> Result<Vec<BodyGenericArg>, PackageStoreError> {
        let Some(segment) = type_path.segments.last() else {
            return Ok(Vec::new());
        };
        self.generic_args_from_item_tree_args_in_scope(&segment.args, scope, subst)
    }

    fn generic_args_from_item_tree_args_in_scope(
        &self,
        args: &[GenericArg],
        scope: ScopeId,
        subst: &TypeSubst,
    ) -> Result<Vec<BodyGenericArg>, PackageStoreError> {
        let mut generic_args = Vec::new();
        for arg in args {
            generic_args.push(self.generic_arg_from_item_tree_arg_in_scope(arg, scope, subst)?);
        }
        Ok(generic_args)
    }

    fn generic_arg_from_item_tree_arg_in_scope(
        &self,
        arg: &GenericArg,
        scope: ScopeId,
        subst: &TypeSubst,
    ) -> Result<BodyGenericArg, PackageStoreError> {
        match arg {
            GenericArg::Type(ty) => Ok(BodyGenericArg::Type(Box::new(
                self.ty_from_type_ref_in_scope_with_subst(ty, scope, subst)?,
            ))),
            GenericArg::Lifetime(lifetime) => Ok(BodyGenericArg::Lifetime(lifetime.clone())),
            GenericArg::Const(value) => Ok(BodyGenericArg::Const(value.clone())),
            GenericArg::AssocType { name, ty } => Ok(BodyGenericArg::AssocType {
                name: name.clone(),
                ty: match ty {
                    Some(ty) => Some(Box::new(
                        self.ty_from_type_ref_in_scope_with_subst(ty, scope, subst)?,
                    )),
                    None => None,
                },
            }),
            GenericArg::Unsupported(text) => Ok(BodyGenericArg::Unsupported(text.clone())),
        }
    }
}

fn split_associated_path(path: &Path) -> Option<(Path, &str)> {
    if path.segments.len() < 2 {
        return None;
    }

    let PathSegment::Name(last_segment) = path.segments.last()? else {
        return None;
    };

    Some((
        Path {
            absolute: path.absolute,
            segments: path.segments[..path.segments.len() - 1].to_vec(),
        },
        last_segment.as_str(),
    ))
}

fn prefix_type_ref(path: &TypePath) -> Option<TypeRef> {
    let prefix_len = path.segments.len().checked_sub(1)?;
    if prefix_len == 0 {
        return None;
    }

    Some(TypeRef::Path(TypePath {
        source_span: path.source_span,
        absolute: path.absolute,
        segments: path.segments[..prefix_len].to_vec(),
    }))
}
