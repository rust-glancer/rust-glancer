//! Tiny type conversion helpers used by body resolution.
//!
//! These helpers preserve known nominal/generic facts. They do not infer missing generic
//! arguments, solve bounds, or inspect expression bodies to discover return types.

use rg_def_map::{DefMapReadTxn, Path};
use rg_item_tree::{GenericArg as ItemGenericArg, GenericParams, Mutability, TypeRef};
use rg_package_store::PackageStoreError;
use rg_semantic_ir::{SemanticIrReadTxn, TypePathContext};
use rg_text::Name;
use rg_ty::{GenericArg, NominalTy, Ty, TypeSubst};

use crate::ir::resolved::BodyTypePathResolution;

/// Converts syntax-level type data into the shared type vocabulary in one module/impl
/// context, applying direct generic substitutions where they are already known.
pub(crate) fn ty_from_type_ref_in_context(
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    ty: &TypeRef,
    context: TypePathContext,
    unresolved_path_fallback: Ty,
    subst: &TypeSubst,
) -> Result<Ty, PackageStoreError> {
    match ty {
        TypeRef::Unit => Ok(Ty::Unit),
        TypeRef::Never => Ok(Ty::Never),
        TypeRef::Path(type_path) => {
            let path = Path::from_type_path(type_path);
            if let Some(ty) = substitute_type_param(&path, subst) {
                return Ok(ty);
            }

            let args = generic_args_from_type_path_in_context(
                def_map,
                semantic_ir,
                type_path,
                context,
                subst,
            )?;
            let resolution = BodyTypePathResolution::from(
                semantic_ir.resolve_type_path(def_map, context, &path)?,
            );
            let fallback = if matches!(resolution, BodyTypePathResolution::Unknown) {
                path.single_name()
                    .and_then(rg_ty::PrimitiveTy::from_name)
                    .map(Ty::Primitive)
                    .unwrap_or(unresolved_path_fallback)
            } else {
                unresolved_path_fallback
            };
            Ok(ty_from_body_resolution(resolution, fallback, args))
        }
        TypeRef::Reference {
            mutability, inner, ..
        } => Ok(Ty::reference(
            match mutability {
                Mutability::Shared => rg_ty::RefMutability::Shared,
                Mutability::Mutable => rg_ty::RefMutability::Mutable,
            },
            ty_from_type_ref_in_context(
                def_map,
                semantic_ir,
                inner,
                context,
                Ty::syntax((**inner).clone()),
                subst,
            )?,
        )),
        TypeRef::Unknown(_) | TypeRef::Infer => Ok(Ty::Unknown),
        TypeRef::Tuple(types) if types.is_empty() => Ok(Ty::Unit),
        _ => Ok(Ty::syntax(ty.clone())),
    }
}

pub(super) fn ty_from_body_resolution(
    resolution: BodyTypePathResolution,
    fallback: Ty,
    args: Vec<GenericArg>,
) -> Ty {
    // Attach the generic arguments from the source path to whichever nominal definition the path
    // resolved to. Ambiguous multi-target resolution keeps the same args on every candidate.
    match resolution {
        BodyTypePathResolution::Primitive(primitive) => Ty::Primitive(primitive),
        BodyTypePathResolution::SelfType(types) => Ty::self_ty(
            types
                .into_iter()
                .map(|def| NominalTy {
                    def,
                    args: args.clone(),
                })
                .collect(),
        ),
        BodyTypePathResolution::TypeDefs(types) => Ty::nominal(
            types
                .into_iter()
                .map(|def| NominalTy {
                    def,
                    args: args.clone(),
                })
                .collect(),
        ),
        BodyTypePathResolution::TypeAliases(_) => fallback,
        BodyTypePathResolution::Traits(_) => fallback,
        BodyTypePathResolution::Unknown => fallback,
    }
}

pub(super) fn subst_from_generics(generics: &GenericParams, args: &[GenericArg]) -> TypeSubst {
    // We only substitute type parameters. Lifetimes, const args, associated type args, and
    // unsupported args are preserved on the type but ignored by the simple substitution map.
    let type_args = args.iter().filter_map(body_generic_arg_ty);

    generics
        .types
        .iter()
        .zip(type_args)
        .map(|(param, ty)| (param.name.clone(), ty))
        .collect()
}

pub(super) fn body_generic_arg_ty(arg: &GenericArg) -> Option<Ty> {
    arg.as_ty().cloned()
}

pub(super) fn generic_arg_type_ref(arg: &ItemGenericArg) -> Option<&TypeRef> {
    match arg {
        ItemGenericArg::Type(ty) => Some(ty),
        ItemGenericArg::Lifetime(_)
        | ItemGenericArg::Const(_)
        | ItemGenericArg::AssocType { .. }
        | ItemGenericArg::Unsupported(_) => None,
    }
}

pub(super) fn type_param_name_from_type_ref(ty: &TypeRef) -> Option<Name> {
    let TypeRef::Path(path) = ty else {
        return None;
    };

    let path = Path::from_type_path(path);
    let name = path.single_name()?;
    path.segments.iter().find_map(|segment| match segment {
        rg_def_map::PathSegment::Name(segment_name) if segment_name.as_str() == name => {
            Some(segment_name.clone())
        }
        _ => None,
    })
}

pub(super) fn substitute_type_param(path: &Path, subst: &TypeSubst) -> Option<Ty> {
    // Only plain identifiers can be generic type parameters. Qualified paths like `module::T`
    // remain ordinary type paths and are resolved through DefMap/Semantic IR.
    let name = path.single_name()?;
    subst.get(name).cloned()
}

pub(super) fn type_ref_is_self(ty: &TypeRef) -> bool {
    Path::from_type_ref(ty).is_some_and(|path| path.is_self_type())
}

fn generic_args_from_type_path_in_context(
    def_map: &DefMapReadTxn<'_>,
    semantic_ir: &SemanticIrReadTxn<'_>,
    type_path: &rg_item_tree::TypePath,
    context: TypePathContext,
    subst: &TypeSubst,
) -> Result<Vec<GenericArg>, PackageStoreError> {
    // Rust generic args belong to the final path segment for the cases we model here, e.g.
    // `crate::Wrapper<User>` stores `User` on `Wrapper`.
    let Some(segment) = type_path.segments.last() else {
        return Ok(Vec::new());
    };

    let mut generic_args = Vec::new();
    for arg in &segment.args {
        let generic_arg = match arg {
            ItemGenericArg::Type(ty) => GenericArg::Type(Box::new(ty_from_type_ref_in_context(
                def_map,
                semantic_ir,
                ty,
                context,
                Ty::syntax(ty.clone()),
                subst,
            )?)),
            ItemGenericArg::Lifetime(lifetime) => GenericArg::Lifetime(lifetime.clone()),
            ItemGenericArg::Const(value) => GenericArg::Const(value.clone()),
            ItemGenericArg::AssocType { name, ty } => GenericArg::AssocType {
                name: name.clone(),
                ty: match ty {
                    Some(ty) => Some(Box::new(ty_from_type_ref_in_context(
                        def_map,
                        semantic_ir,
                        ty,
                        context,
                        Ty::syntax(ty.clone()),
                        subst,
                    )?)),
                    None => None,
                },
            },
            ItemGenericArg::Unsupported(text) => GenericArg::Unsupported(text.clone()),
        };

        generic_args.push(generic_arg);
    }
    Ok(generic_args)
}
