//! Start independent lowering operations in the caller's storage or in temporary storage.
//!
//! Declaration entry points choose the owner and lookup context before creating a session. Keep
//! that session for all source types in the operation, such as every parameter and the return
//! type of a function. The session handles recursive source work and the query assembles the result.

mod signature;

use rg_def_map::DefMapSource;
use rg_item_tree::{GenericArg as ItemGenericArg, TypeRef};
use rg_semantic_ir::{Generics, ItemStoreSource};

use super::{TypeLoweringEnv, TypeLoweringSession, TypePathResolver};
use crate::{
    lookup::ItemPathQuery,
    solver::{GenericArgs, InferenceTable, SemanticDeclarations, SolverInterner, Ty},
};

/// Entry points for lowering types and declarations using the supplied sources and path resolver.
///
/// Each operation starts a session for its source walk. The query keeps no walk state between
/// operations; callers that need to lower several related source types can retain a session
/// themselves. Both forms can use the caller's solver storage.
pub struct TypeLoweringQuery<'lower, 'query, D, I, R> {
    item_paths: &'lower ItemPathQuery<'query, D, I>,
    resolver: &'lower R,
}

impl<'lower, 'query, D, I, R> TypeLoweringQuery<'lower, 'query, D, I, R>
where
    D: DefMapSource,
    I: ItemStoreSource<'query, Error = D::Error>,
    R: TypePathResolver<Error = D::Error>,
{
    pub fn new(item_paths: &'lower ItemPathQuery<'query, D, I>, resolver: &'lower R) -> Self {
        Self {
            item_paths,
            resolver,
        }
    }

    /// Start a source walk in the supplied storage and declaration context.
    /// Keep the returned session across related types, such as a signature's parameters and
    /// return type. Starting another session resets occurrence numbering and recursion tracking.
    pub fn session<'s>(
        &self,
        cx: SolverInterner<'s>,
        env: TypeLoweringEnv,
    ) -> Result<TypeLoweringSession<'s, 'lower, 'query, D, I, R>, D::Error> {
        TypeLoweringSession::new(self.item_paths, self.resolver, cx, env)
    }

    /// Own the temporary storage for an independent result. Declaration callbacks during this
    /// operation use this same storage; recursive source steps never create their own arenas.
    pub fn with_storage<T>(
        &self,
        run: impl for<'s> FnOnce(SolverInterner<'s>) -> Result<T, D::Error>,
    ) -> Result<T, D::Error> {
        let declarations = SemanticDeclarations::for_lowering(self.item_paths, self.resolver);
        declarations.with_storage(run)?
    }

    pub fn lower(&self, ty: &TypeRef, env: TypeLoweringEnv) -> Result<crate::Ty, D::Error> {
        self.with_storage(|cx| {
            let ty = self.session(cx, env)?.lower_type_ref(ty)?;
            Ok(cx.raise_ty(ty).unwrap_or(crate::Ty::Unknown))
        })
    }

    /// Interpret a body annotation in the caller's inference context. A written `_` belongs to
    /// this table, so later constraints on the result can fill it in.
    pub fn lower_inference_type<'s>(
        &self,
        ty: &TypeRef,
        env: TypeLoweringEnv,
        table: &InferenceTable<'s>,
    ) -> Result<Ty<'s>, D::Error> {
        self.session(table.interner(), env)?
            .lower_type_ref_with_inference(ty, table)
    }

    /// Interpret explicit arguments such as `collect::<Vec<_>>()` without exporting the
    /// caller's variables into an independent result.
    pub fn lower_inference_args<'s>(
        &self,
        generics: &Generics<'_>,
        args: &[ItemGenericArg],
        env: TypeLoweringEnv,
        table: &InferenceTable<'s>,
    ) -> Result<GenericArgs<'s>, D::Error> {
        self.session(table.interner(), env)?
            .lower_generic_args_for(generics, args, Some(table))
    }
}
