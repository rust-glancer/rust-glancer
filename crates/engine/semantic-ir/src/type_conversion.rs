//! Small type conversion helpers over item/path query providers.
//!
//! These helpers preserve known nominal/generic facts. They do not infer missing generic
//! arguments, solve bounds, or inspect expression bodies to discover return types.

use rg_def_map::{DefMapSource, Path};
use rg_ir_model::TypePathResolution;
use rg_item_tree::{GenericArg as ItemGenericArg, GenericParams, Mutability, TypeRef};
use rg_package_store::PackageStoreError;
use rg_text::Name;
use rg_ty::{GenericArg, NominalTy, Ty, TypeSubst};

use crate::{ItemPathQuery, ItemStoreSource, TypePathContext};

/// Converts syntax-level type data into the shared type vocabulary in one module/impl
/// context, applying direct generic substitutions where they are already known.
pub fn ty_from_type_ref_in_context<'a, D, I>(
    item_paths: &ItemPathQuery<'a, D, I>,
    ty: &TypeRef,
    context: TypePathContext,
    unresolved_path_fallback: Ty,
    subst: &TypeSubst,
) -> Result<Ty, PackageStoreError>
where
    D: DefMapSource<Error = PackageStoreError>,
    I: ItemStoreSource<'a, Error = PackageStoreError>,
{
    match ty {
        TypeRef::Unit => Ok(Ty::Unit),
        TypeRef::Never => Ok(Ty::Never),
        TypeRef::Path(type_path) => {
            let path = Path::from_type_path(type_path);
            if let Some(ty) = substitute_type_param(&path, subst) {
                return Ok(ty);
            }

            let args =
                generic_args_from_type_path_in_context(item_paths, type_path, context, subst)?;
            let resolution = item_paths.resolve_type_path(context, &path)?;
            let fallback = if matches!(resolution, TypePathResolution::Unknown) {
                path.single_name()
                    .and_then(rg_ty::PrimitiveTy::from_name)
                    .map(Ty::Primitive)
                    .unwrap_or(unresolved_path_fallback)
            } else {
                unresolved_path_fallback
            };
            Ok(ty_from_type_path_resolution(resolution, fallback, args))
        }
        TypeRef::Reference {
            mutability, inner, ..
        } => Ok(Ty::reference(
            match mutability {
                Mutability::Shared => rg_ty::RefMutability::Shared,
                Mutability::Mutable => rg_ty::RefMutability::Mutable,
            },
            ty_from_type_ref_in_context(
                item_paths,
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

pub fn ty_from_type_path_resolution(
    resolution: TypePathResolution,
    fallback: Ty,
    args: Vec<GenericArg>,
) -> Ty {
    // Attach the generic arguments from the source path to whichever nominal definition the path
    // resolved to. Ambiguous multi-target resolution keeps the same args on every candidate.
    match resolution {
        TypePathResolution::SelfType(types) => Ty::self_ty(
            types
                .into_iter()
                .map(|def| NominalTy {
                    def,
                    args: args.clone(),
                })
                .collect(),
        ),
        TypePathResolution::TypeDefs(types) => Ty::nominal(
            types
                .into_iter()
                .map(|def| NominalTy {
                    def,
                    args: args.clone(),
                })
                .collect(),
        ),
        TypePathResolution::TypeAliases(_) => fallback,
        TypePathResolution::Traits(_) => fallback,
        TypePathResolution::Unknown => fallback,
    }
}

pub fn subst_from_generics(generics: &GenericParams, args: &[GenericArg]) -> TypeSubst {
    // We only substitute type parameters. Lifetimes, const args, associated type args, and
    // unsupported args are preserved on the type but ignored by the simple substitution map.
    let type_args = args.iter().filter_map(generic_arg_ty);

    generics
        .types
        .iter()
        .zip(type_args)
        .map(|(param, ty)| (param.name.clone(), ty))
        .collect()
}

pub(crate) fn generic_arg_ty(arg: &GenericArg) -> Option<Ty> {
    arg.as_ty().cloned()
}

pub(crate) fn generic_arg_type_ref(arg: &ItemGenericArg) -> Option<&TypeRef> {
    match arg {
        ItemGenericArg::Type(ty) => Some(ty),
        ItemGenericArg::Lifetime(_)
        | ItemGenericArg::Const(_)
        | ItemGenericArg::AssocType { .. }
        | ItemGenericArg::Unsupported(_) => None,
    }
}

pub(crate) fn type_param_name_from_type_ref(ty: &TypeRef) -> Option<Name> {
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

pub fn substitute_type_param(path: &Path, subst: &TypeSubst) -> Option<Ty> {
    // Only plain identifiers can be generic type parameters. Qualified paths like `module::T`
    // remain ordinary type paths and are resolved through DefMap/Semantic IR.
    let name = path.single_name()?;
    subst.get(name).cloned()
}

pub fn type_ref_is_self(ty: &TypeRef) -> bool {
    Path::from_type_ref(ty).is_some_and(|path| path.is_self_type())
}

fn generic_args_from_type_path_in_context<'a, D, I>(
    item_paths: &ItemPathQuery<'a, D, I>,
    type_path: &rg_item_tree::TypePath,
    context: TypePathContext,
    subst: &TypeSubst,
) -> Result<Vec<GenericArg>, PackageStoreError>
where
    D: DefMapSource<Error = PackageStoreError>,
    I: ItemStoreSource<'a, Error = PackageStoreError>,
{
    // Rust generic args belong to the final path segment for the cases we model here, e.g.
    // `crate::Wrapper<User>` stores `User` on `Wrapper`.
    let Some(segment) = type_path.segments.last() else {
        return Ok(Vec::new());
    };

    let mut generic_args = Vec::new();
    for arg in &segment.args {
        let generic_arg = match arg {
            ItemGenericArg::Type(ty) => GenericArg::Type(Box::new(ty_from_type_ref_in_context(
                item_paths,
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
                        item_paths,
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
