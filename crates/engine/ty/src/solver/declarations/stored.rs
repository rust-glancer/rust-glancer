//! Declaration templates that can be shared between independent operations in one context.
//! Import and export happen here; inference variables never belong to a shared declaration.

use rg_ir_model::GenericParamRef;

use super::{Declaration, DeclarationKind, LangItem};
use crate::{
    Clause, Ty, signature,
    solver::{self, DefId, List, SolverInterner, types::AdtDef},
};

/// The owned form of a complete declaration template.
///
/// These types still use Glancer's owned representation. For example, a function's `T` is a
/// generic parameter here; each call gets its own inference variable when it is instantiated.
/// That lets several solver operations share a declaration without sharing their answers.
pub(crate) struct StoredDeclaration {
    pub name: String,
    pub generics: Vec<GenericParamRef>,
    pub parent_count: usize,
    pub parent: Option<DefId>,
    // Requirements on using the item, such as `T: Clone` on `fn copy<T: Clone>(...)`.
    pub predicates: Vec<Clause>,
    // Promises about an associated or opaque type, such as `type Item: Clone`.
    pub bounds: Vec<Clause>,
    pub lang_item: Option<LangItem>,
    pub kind: StoredDeclarationKind,
}

impl StoredDeclaration {
    pub(crate) fn lower<'s>(&self, cx: SolverInterner<'s>) -> Declaration<'s> {
        let lower_ty = |ty| cx.lower_ty(ty, &self.generics);
        let lower_clauses = |clauses: &[Clause]| {
            clauses
                .iter()
                .map(|clause| cx.lower_clause(clause, &self.generics))
                .collect::<Vec<_>>()
        };
        let kind = match &self.kind {
            StoredDeclarationKind::Adt { data, fields } => DeclarationKind::Adt {
                data: *data,
                fields: fields.iter().map(lower_ty).collect(),
            },
            StoredDeclarationKind::Trait {
                is_auto,
                is_unsafe,
                associated_types,
            } => DeclarationKind::Trait {
                is_auto: *is_auto,
                is_unsafe: *is_unsafe,
                associated_types: associated_types.clone(),
            },
            StoredDeclarationKind::Impl {
                header,
                associated_types,
            } => DeclarationKind::Impl {
                header: solver::ImplHeader {
                    owner: header.owner,
                    self_ty: lower_ty(&header.self_ty),
                    trait_ref: header
                        .trait_ref
                        .as_ref()
                        .map(|bound| solver::TraitRefLowering {
                            application: solver::TraitApplication {
                                def: bound.application.def,
                                args: cx.lower_args(&bound.application.args, &self.generics),
                            },
                            associated_types: bound
                                .associated_types
                                .iter()
                                .map(|binding| solver::AssocTypeBinding {
                                    associated_ty: binding.associated_ty,
                                    ty: lower_ty(&binding.ty),
                                })
                                .collect(),
                        }),
                    clauses: lower_clauses(&header.clauses),
                },
                associated_types: associated_types.clone(),
            },
            StoredDeclarationKind::Function(sig) => {
                DeclarationKind::Function(solver::CallableSignature {
                    params: List::new(cx, &sig.params.iter().map(lower_ty).collect::<Vec<_>>()),
                    ret: lower_ty(&sig.ret),
                    clauses: List::new(cx, &lower_clauses(&sig.clauses)),
                    qualifiers: sig.qualifiers,
                })
            }
            StoredDeclarationKind::Alias(ty) => DeclarationKind::Alias(ty.as_ref().map(lower_ty)),
            StoredDeclarationKind::Opaque => DeclarationKind::Opaque,
            StoredDeclarationKind::Unavailable => DeclarationKind::Unavailable,
        };
        Declaration {
            name: self.name.clone(),
            generics: self.generics.clone(),
            parent_count: self.parent_count,
            parent: self.parent,
            predicates: lower_clauses(&self.predicates),
            bounds: lower_clauses(&self.bounds),
            lang_item: self.lang_item,
            kind,
        }
    }

    pub(crate) fn raise<'s>(cx: SolverInterner<'s>, data: &Declaration<'s>) -> Self {
        let raise_ty = |ty| cx.raise_ty(ty).unwrap_or(Ty::Unknown);
        let raise_clauses = |clauses: &[solver::Clause<'s>]| {
            clauses
                .iter()
                .map(|&clause| cx.raise_clause(clause))
                .collect::<Vec<_>>()
        };
        let kind = match &data.kind {
            DeclarationKind::Adt { data, fields } => StoredDeclarationKind::Adt {
                data: *data,
                fields: fields.iter().copied().map(raise_ty).collect(),
            },
            DeclarationKind::Trait {
                is_auto,
                is_unsafe,
                associated_types,
            } => StoredDeclarationKind::Trait {
                is_auto: *is_auto,
                is_unsafe: *is_unsafe,
                associated_types: associated_types.clone(),
            },
            DeclarationKind::Impl {
                header,
                associated_types,
            } => StoredDeclarationKind::Impl {
                header: header.raise(cx),
                associated_types: associated_types.clone(),
            },
            DeclarationKind::Function(sig) => StoredDeclarationKind::Function(sig.raise(cx)),
            DeclarationKind::Alias(ty) => StoredDeclarationKind::Alias(ty.map(raise_ty)),
            DeclarationKind::Opaque => StoredDeclarationKind::Opaque,
            DeclarationKind::Unavailable => StoredDeclarationKind::Unavailable,
        };
        Self {
            name: data.name.clone(),
            generics: data.generics.clone(),
            parent_count: data.parent_count,
            parent: data.parent,
            predicates: raise_clauses(&data.predicates),
            bounds: raise_clauses(&data.bounds),
            lang_item: data.lang_item,
            kind,
        }
    }
}

pub(crate) enum StoredDeclarationKind {
    Adt {
        data: AdtDef,
        fields: Vec<Ty>,
    },
    Trait {
        is_auto: bool,
        is_unsafe: bool,
        associated_types: Vec<DefId>,
    },
    Impl {
        header: signature::ImplHeader,
        associated_types: Vec<(String, DefId)>,
    },
    Function(signature::CallableSignature),
    Alias(Option<Ty>),
    Opaque,
    Unavailable,
}
