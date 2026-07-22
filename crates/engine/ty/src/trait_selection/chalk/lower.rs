//! Canonical rust-glancer type data lowered into Chalk's solver vocabulary.
//!
//! Definition syntax is interpreted by `TypeLoweringSession` before it reaches this module. Chalk
//! therefore sees the same owner-scoped parameters, full-arity arguments, aliases, and clauses as
//! body inference. Returning `None` here means that the bounded solver does not model a semantic
//! shape; it must never trigger another walk over `TypeRef`.

use std::collections::HashMap;
use std::sync::Arc;

use chalk_ir::cast::Cast;
use chalk_ir::fold::Shift;
use chalk_ir::{
    AdtId, AliasEq, AliasTy as ChalkAliasTy, AssocTypeId, Binders, BoundVar, ConcreteConst,
    ConstData, ConstValue as ChalkConstValue, DebruijnIndex, DomainGoal, FnDefId, FnPointer, FnSig,
    FnSubst, GenericArg as ChalkGenericArg, GenericArgData, Goal, GoalData, LifetimeData,
    Mutability as ChalkMutability, Normalize, OpaqueTyId, ProjectionTy as ChalkProjectionTy,
    QuantifiedWhereClause, QuantifierKind, Safety, Scalar, Substitution as ChalkSubstitution,
    TraitId, TraitRef as ChalkTraitRef, TyKind, TyVariableKind, UintTy, VariableKind,
    VariableKinds, WhereClause,
};
use chalk_solve::rust_ir::{
    AdtDatum, AdtDatumBound, AdtFlags, AdtKind, AdtVariantDatum, AssociatedTyDatum,
    AssociatedTyDatumBound, AssociatedTyValue, AssociatedTyValueBound, AssociatedTyValueId,
    FnDefDatum, FnDefDatumBound, FnDefInputsAndOutputDatum, ImplDatum, ImplDatumBound, ImplType,
    OpaqueTyDatum, OpaqueTyDatumBound, Polarity, TraitDatum, TraitDatumBound, TraitFlags,
    WellKnownTrait,
};
use rg_ir_model::{
    FunctionRef, GenericParamRef, ImplRef, Mutability, TraitDefRef, TypeAliasRef, TypeDefId,
    TypeDefRef,
};
use rg_semantic_ir::{Generics, TypeAliasData};

use super::evidence::{ProjectionAliasLowering, SolverVariableEnv};
use super::interner::{ChalkDefId, RgChalkInterner};
use crate::inference::{InferVarKind, InferenceTable};
use crate::signature::TraitHeader;
use crate::trait_selection::TraitGoal;
use crate::{
    AliasTy, CallableSignature, Clause, ConstValue, FloatTy, GenericArg, ImplHeader, Lifetime,
    PrimitiveTy, SignedIntTy, TraitApplication, TraitRefLowering, Ty, UnsignedIntTy,
};

pub(super) type ChalkTy = chalk_ir::Ty<RgChalkInterner>;
pub(super) type ChalkGoal = Goal<RgChalkInterner>;

const INTER: RgChalkInterner = RgChalkInterner;

/// One conjunction whose existential variables correspond to a project inference table.
///
/// Predicate clauses must be solved together. For example, `I: Iterator<Item = T>, T: Copy`
/// uses the projection equality to determine the same `T` that the second clause checks. Splitting
/// those clauses into independent closed goals would discard precisely that inference evidence.
pub(super) struct PredicateGoalLowering {
    pub(super) goal: ChalkGoal,
    pub(super) variables: SolverVariableEnv,
}

/// Maps one semantic generic binder to Chalk's positional bound-variable representation.
#[derive(Debug, Clone)]
pub(super) struct GenericBinderEnv {
    bindings: Vec<GenericBinding>,
    indices: HashMap<GenericParamRef, usize>,
}

#[derive(Debug, Clone, Copy)]
enum GenericBinding {
    Type,
    Lifetime,
    Const,
}

impl GenericBinderEnv {
    pub(super) fn for_generics(generics: &Generics<'_>) -> Self {
        let mut bindings = Vec::with_capacity(generics.len());
        let mut indices = HashMap::with_capacity(generics.len());
        for param in generics.iter() {
            let param = param.param();
            indices.insert(param, bindings.len());
            bindings.push(match param {
                GenericParamRef::Lifetime(_) => GenericBinding::Lifetime,
                GenericParamRef::Type(_) => GenericBinding::Type,
                GenericParamRef::Const(_) => GenericBinding::Const,
            });
        }
        Self { bindings, indices }
    }

    pub(super) fn empty() -> Self {
        Self {
            bindings: Vec::new(),
            indices: HashMap::new(),
        }
    }

    pub(super) fn variable_kinds(&self) -> VariableKinds<RgChalkInterner> {
        VariableKinds::from_iter(
            INTER,
            self.bindings.iter().map(|binding| match binding {
                GenericBinding::Type => VariableKind::Ty(TyVariableKind::General),
                GenericBinding::Lifetime => VariableKind::Lifetime,
                // The project keeps scalar const identity but does not retain a semantic const
                // parameter type yet. Integer consts are the supported subset, so use `usize`
                // consistently instead of recovering source syntax inside this adapter.
                GenericBinding::Const => VariableKind::Const(usize_ty()),
            }),
        )
    }

    fn bound_var(&self, param: GenericParamRef) -> Option<BoundVar> {
        self.indices
            .get(&param)
            .copied()
            .map(|index| BoundVar::new(DebruijnIndex::INNERMOST, index))
    }
}

/// Stateless conversion scoped by the semantic binder and solver-supported definitions.
///
/// Associated types with required bounds or their own generics are deliberately omitted from the
/// Chalk program until their extra binder and predicate layers can be represented faithfully.
/// Keeping the supported registry here makes every nested projection decline at the same boundary
/// instead of giving Chalk an ID whose datum does not exist.
pub(super) struct ChalkLowerer<'lower> {
    binders: &'lower GenericBinderEnv,
    associated_tys: Option<&'lower HashMap<TypeAliasRef, Arc<AssociatedTyDatum<RgChalkInterner>>>>,
    functions: Option<&'lower HashMap<FunctionRef, Arc<FnDefDatum<RgChalkInterner>>>>,
}

impl<'lower> ChalkLowerer<'lower> {
    pub(super) fn new(binders: &'lower GenericBinderEnv) -> Self {
        Self {
            binders,
            associated_tys: None,
            functions: None,
        }
    }

    pub(super) fn with_associated_tys(
        mut self,
        associated_tys: &'lower HashMap<TypeAliasRef, Arc<AssociatedTyDatum<RgChalkInterner>>>,
    ) -> Self {
        self.associated_tys = Some(associated_tys);
        self
    }

    pub(super) fn with_functions(
        mut self,
        functions: &'lower HashMap<FunctionRef, Arc<FnDefDatum<RgChalkInterner>>>,
    ) -> Self {
        self.functions = Some(functions);
        self
    }

    pub(super) fn trait_datum(
        &self,
        header: &TraitHeader,
        associated_ty_ids: Vec<AssocTypeId<RgChalkInterner>>,
        well_known: Option<WellKnownTrait>,
    ) -> Option<TraitDatum<RgChalkInterner>> {
        let where_clauses = self.lower_quantified_clauses(&header.clauses)?;
        Some(TraitDatum {
            id: chalk_trait_id(header.owner),
            binders: chalk_ir::Binders::new(
                self.binders.variable_kinds(),
                TraitDatumBound { where_clauses },
            ),
            flags: TraitFlags {
                auto: false,
                marker: false,
                upstream: false,
                fundamental: false,
                non_enumerable: false,
                coinductive: false,
            },
            associated_ty_ids,
            well_known,
        })
    }

    /// Lower one function item whose semantic callable signature is representable by this adapter.
    ///
    /// An `async fn` is callable, but its real output is an anonymous future rather than the
    /// written return type retained by `CallableSignature`. Decline it until that desugared type
    /// exists instead of teaching Chalk a plausible but false `FnOnce::Output` equality.
    pub(super) fn fn_def_datum(
        &self,
        function: rg_ir_model::FunctionRef,
        signature: &CallableSignature,
    ) -> Option<FnDefDatum<RgChalkInterner>> {
        if signature.qualifiers.is_async {
            return None;
        }
        let argument_types = signature
            .params
            .iter()
            .map(|param| {
                self.lower_ty(param, None, None)
                    .map(|ty| ty.shifted_in(INTER))
            })
            .collect::<Option<Vec<_>>>()?;
        let return_type = self.lower_ty(&signature.ret, None, None)?.shifted_in(INTER);
        let where_clauses = self.lower_quantified_clauses(&signature.clauses)?;
        Some(FnDefDatum {
            id: FnDefId(ChalkDefId::Function(function)),
            sig: FnSig {
                abi: (),
                safety: if signature.qualifiers.is_unsafe {
                    Safety::Unsafe
                } else {
                    Safety::Safe
                },
                variadic: false,
            },
            binders: Binders::new(
                self.binders.variable_kinds(),
                FnDefDatumBound {
                    inputs_and_output: Binders::empty(
                        INTER,
                        FnDefInputsAndOutputDatum {
                            argument_types,
                            return_type,
                        },
                    ),
                    where_clauses,
                },
            ),
        })
    }

    pub(super) fn associated_ty_datum(
        &self,
        trait_ref: TraitDefRef,
        type_alias_ref: TypeAliasRef,
        type_alias_data: &TypeAliasData,
    ) -> Option<AssociatedTyDatum<RgChalkInterner>> {
        if !Self::supports_associated_ty_declaration(type_alias_data) {
            return None;
        }

        Some(AssociatedTyDatum {
            trait_id: chalk_trait_id(trait_ref),
            id: chalk_assoc_type_id(type_alias_ref),
            name: type_alias_data.name.to_string(),
            binders: chalk_ir::Binders::new(
                self.binders.variable_kinds(),
                AssociatedTyDatumBound {
                    bounds: Vec::new(),
                    where_clauses: Vec::new(),
                },
            ),
        })
    }

    pub(super) fn impl_datum(
        &self,
        header: &ImplHeader,
        associated_ty_value_ids: Vec<AssociatedTyValueId<RgChalkInterner>>,
    ) -> Option<ImplDatum<RgChalkInterner>> {
        let trait_ref = header.trait_ref.as_ref()?;
        let trait_ref = self.lower_trait_application(&trait_ref.application, None, None)?;
        let where_clauses = self.lower_quantified_clauses(&header.clauses)?;
        Some(ImplDatum {
            polarity: Polarity::Positive,
            binders: chalk_ir::Binders::new(
                self.binders.variable_kinds(),
                ImplDatumBound {
                    trait_ref,
                    where_clauses,
                },
            ),
            impl_type: ImplType::Local,
            associated_ty_value_ids,
        })
    }

    pub(super) fn associated_ty_value(
        &self,
        impl_ref: ImplRef,
        associated_ty_ref: TypeAliasRef,
        type_alias_data: &TypeAliasData,
        ty: &Ty,
    ) -> Option<AssociatedTyValue<RgChalkInterner>> {
        if !Self::supports_associated_ty_declaration(type_alias_data) {
            return None;
        }

        Some(AssociatedTyValue {
            impl_id: chalk_impl_id(impl_ref),
            associated_ty_id: chalk_assoc_type_id(associated_ty_ref),
            value: chalk_ir::Binders::new(
                self.binders.variable_kinds(),
                AssociatedTyValueBound {
                    ty: self.lower_ty(ty, None, None)?,
                },
            ),
        })
    }

    /// Checks whether an associated type fits Chalk's binder-free datum shape.
    ///
    /// GAT parameters and required bounds need additional Chalk binders or predicates. A relaxed
    /// bound such as `?Sized` only suppresses Rust's implicit `Sized` requirement; rust-glancer
    /// does not introduce that implicit requirement, so there is no predicate to lower here.
    pub(super) fn supports_associated_ty_declaration(data: &TypeAliasData) -> bool {
        data.signature.generics().is_none()
            && data
                .signature
                .bounds()
                .iter()
                .all(rg_item_tree::TypeBound::is_relaxed_trait)
    }

    pub(super) fn opaque_ty_datum(
        &self,
        opaque: &crate::OpaqueTy,
        bounds: &[TraitRefLowering],
    ) -> Option<OpaqueTyDatum<RgChalkInterner>> {
        let clauses = bounds
            .iter()
            .map(|bound| self.lower_opaque_bound(bound))
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect();
        Some(OpaqueTyDatum {
            opaque_ty_id: chalk_opaque_ty_id(opaque.opaque),
            bound: chalk_ir::Binders::new(
                self.binders.variable_kinds(),
                OpaqueTyDatumBound {
                    bounds: chalk_ir::Binders::new(
                        VariableKinds::from_iter(
                            INTER,
                            [VariableKind::Ty(TyVariableKind::General)],
                        ),
                        clauses,
                    ),
                    where_clauses: chalk_ir::Binders::empty(INTER, Vec::new()),
                },
            ),
        })
    }

    /// Lower an opaque predicate under Chalk's dedicated `Self` binder.
    ///
    /// Semantic predicates name the opaque identity in their first trait argument. Chalk instead
    /// expects every opaque bound to quantify a fresh `Self`, which it later substitutes with the
    /// opaque placeholder. Owner parameters move out one De Bruijn level under that binder.
    fn lower_opaque_bound(
        &self,
        bound: &TraitRefLowering,
    ) -> Option<Vec<QuantifiedWhereClause<RgChalkInterner>>> {
        let self_ty = BoundVar::new(DebruijnIndex::INNERMOST, 0)
            .to_ty::<RgChalkInterner>(INTER)
            .shifted_in(INTER)
            .cast(INTER);
        let mut args = vec![self_ty];
        args.extend(
            bound
                .application
                .args
                .iter()
                .skip(1)
                .map(|arg| {
                    self.lower_arg(arg, None, None)
                        .map(|arg| arg.shifted_in(INTER).shifted_in(INTER))
                })
                .collect::<Option<Vec<_>>>()?,
        );
        let substitution = ChalkSubstitution::from_iter(INTER, args);
        let mut clauses = vec![chalk_ir::Binders::empty(
            INTER,
            WhereClause::Implemented(ChalkTraitRef {
                trait_id: chalk_trait_id(bound.application.def),
                substitution: substitution.clone(),
            }),
        )];
        for binding in &bound.associated_types {
            if !self.supports_associated_ty(binding.associated_ty) {
                return None;
            }
            clauses.push(chalk_ir::Binders::empty(
                INTER,
                WhereClause::AliasEq(AliasEq {
                    alias: ChalkAliasTy::Projection(ChalkProjectionTy {
                        associated_ty_id: chalk_assoc_type_id(binding.associated_ty),
                        substitution: substitution.clone(),
                    }),
                    ty: self
                        .lower_ty(&binding.ty, None, None)?
                        .shifted_in(INTER)
                        .shifted_in(INTER),
                }),
            ));
        }
        Some(clauses)
    }

    pub(super) fn projection_alias(
        &self,
        assoc_type_ref: TypeAliasRef,
        goal: &TraitGoal,
        table: &InferenceTable,
    ) -> Option<ProjectionAliasLowering> {
        if !self.supports_associated_ty(assoc_type_ref) {
            return None;
        }
        let variables = SolverVariableEnv::from_goal(goal, table);
        let substitution = goal
            .application
            .args
            .iter()
            .map(|arg| self.lower_arg(arg, Some(table), Some(&variables)))
            .collect::<Option<Vec<_>>>()?;
        let alias = ChalkAliasTy::Projection(ChalkProjectionTy {
            associated_ty_id: chalk_assoc_type_id(assoc_type_ref),
            substitution: ChalkSubstitution::from_iter(INTER, substitution),
        });
        Some(ProjectionAliasLowering { alias, variables })
    }

    /// Lower the selected impl arguments in the same existential environment as its projection.
    pub(super) fn selected_impl_args(
        &self,
        args: &[GenericArg],
        table: &InferenceTable,
        projection_vars: &SolverVariableEnv,
    ) -> Option<ChalkSubstitution<RgChalkInterner>> {
        self.lower_args(args, Some(table), Some(projection_vars))
    }

    pub(super) fn predicate_goal(
        &self,
        clauses: &[Clause],
        table: &InferenceTable,
    ) -> Option<PredicateGoalLowering> {
        let variables = SolverVariableEnv::from_clauses(clauses, table);
        let mut goals = clauses
            .iter()
            .map(|clause| {
                // Definition predicates are stored as where-clauses, but an active associated
                // equality is a normalization question. Asking `Normalize` here lets Chalk return
                // the projected value as evidence for another clause in this conjunction.
                match clause {
                    Clause::Implemented(application) => Some(
                        DomainGoal::Holds(WhereClause::Implemented(self.lower_trait_application(
                            application,
                            Some(table),
                            Some(&variables),
                        )?))
                        .cast(INTER),
                    ),
                    Clause::AliasEq { alias, ty } => {
                        if !self.supports_associated_ty(alias.associated_ty) {
                            return None;
                        }
                        Some(
                            DomainGoal::Normalize(Normalize {
                                alias: ChalkAliasTy::Projection(ChalkProjectionTy {
                                    associated_ty_id: chalk_assoc_type_id(alias.associated_ty),
                                    substitution: self.lower_args(
                                        &alias.args,
                                        Some(table),
                                        Some(&variables),
                                    )?,
                                }),
                                ty: self.lower_ty(ty, Some(table), Some(&variables))?,
                            })
                            .cast(INTER),
                        )
                    }
                }
            })
            .collect::<Option<Vec<_>>>()?;

        // Chalk selects the last pending condition, so reverse source order for the same reason as
        // declaration predicates: earlier projection equalities should feed later trait checks.
        goals.reverse();
        let goal = GoalData::Quantified(
            QuantifierKind::Exists,
            Binders::new(variables.variable_kinds(), Goal::all(INTER, goals)),
        )
        .intern(INTER);
        Some(PredicateGoalLowering { goal, variables })
    }

    fn lower_quantified_clauses(
        &self,
        clauses: &[Clause],
    ) -> Option<Vec<QuantifiedWhereClause<RgChalkInterner>>> {
        let mut clauses = clauses
            .iter()
            .map(|clause| {
                // The empty quantified binder sits inside the trait/impl binder. Shift references
                // to owner parameters out one level so Chalk does not treat them as local here.
                Some(chalk_ir::Binders::empty(
                    INTER,
                    self.lower_clause(clause, None, None)?.shifted_in(INTER),
                ))
            })
            .collect::<Option<Vec<_>>>()?;

        // Chalk's SLG engine selects the last pending condition. Store the conjunction in reverse
        // so source-order prerequisites are still evaluated first. This matters for bounds such as
        // `I: Iterator<Item = T>, T: Copy`: the projection equality should determine `T` before
        // impl lookup is asked to enumerate `Copy` candidates for it.
        clauses.reverse();
        Some(clauses)
    }

    fn lower_clause(
        &self,
        clause: &Clause,
        table: Option<&InferenceTable>,
        projection_vars: Option<&SolverVariableEnv>,
    ) -> Option<WhereClause<RgChalkInterner>> {
        match clause {
            Clause::Implemented(application) => Some(WhereClause::Implemented(
                self.lower_trait_application(application, table, projection_vars)?,
            )),
            Clause::AliasEq { alias, ty } => {
                if !self.supports_associated_ty(alias.associated_ty) {
                    return None;
                }
                Some(WhereClause::AliasEq(AliasEq {
                    alias: ChalkAliasTy::Projection(ChalkProjectionTy {
                        associated_ty_id: chalk_assoc_type_id(alias.associated_ty),
                        substitution: self.lower_args(&alias.args, table, projection_vars)?,
                    }),
                    ty: self.lower_ty(ty, table, projection_vars)?,
                }))
            }
        }
    }

    fn lower_trait_application(
        &self,
        application: &TraitApplication,
        table: Option<&InferenceTable>,
        projection_vars: Option<&SolverVariableEnv>,
    ) -> Option<ChalkTraitRef<RgChalkInterner>> {
        Some(ChalkTraitRef {
            trait_id: chalk_trait_id(application.def),
            substitution: self.lower_args(&application.args, table, projection_vars)?,
        })
    }

    fn lower_args(
        &self,
        args: &[GenericArg],
        table: Option<&InferenceTable>,
        projection_vars: Option<&SolverVariableEnv>,
    ) -> Option<ChalkSubstitution<RgChalkInterner>> {
        args.iter()
            .map(|arg| self.lower_arg(arg, table, projection_vars))
            .collect::<Option<Vec<_>>>()
            .map(|args| ChalkSubstitution::from_iter(INTER, args))
    }

    fn lower_arg(
        &self,
        arg: &GenericArg,
        table: Option<&InferenceTable>,
        projection_vars: Option<&SolverVariableEnv>,
    ) -> Option<ChalkGenericArg<RgChalkInterner>> {
        match arg {
            GenericArg::Type(ty) => self
                .lower_ty(ty, table, projection_vars)
                .map(|ty| GenericArgData::Ty(ty).intern(INTER)),
            GenericArg::Lifetime(lifetime) => self
                .lower_lifetime(*lifetime)
                .map(|lifetime| GenericArgData::Lifetime(lifetime).intern(INTER)),
            GenericArg::Const(value) => self
                .lower_const(*value)
                .map(|value| GenericArgData::Const(value).intern(INTER)),
        }
    }

    fn lower_ty(
        &self,
        ty: &Ty,
        table: Option<&InferenceTable>,
        projection_vars: Option<&SolverVariableEnv>,
    ) -> Option<ChalkTy> {
        let canonical;
        let ty = if let Some(table) = table {
            canonical = table.canonicalize(ty);
            &canonical
        } else {
            ty
        };

        match ty {
            Ty::Unit => Some(unit_ty()),
            Ty::Never => Some(TyKind::Never.intern(INTER)),
            Ty::Primitive(primitive) => Some(Self::lower_primitive(*primitive)),
            Ty::Tuple(fields) => {
                let fields = fields
                    .iter()
                    .map(|field| {
                        self.lower_ty(field, table, projection_vars)
                            .map(|ty| GenericArgData::Ty(ty).intern(INTER))
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(
                    TyKind::Tuple(fields.len(), ChalkSubstitution::from_iter(INTER, fields))
                        .intern(INTER),
                )
            }
            Ty::Array { inner, len } => Some(
                TyKind::Array(
                    self.lower_ty(inner, table, projection_vars)?,
                    self.lower_const(*len)?,
                )
                .intern(INTER),
            ),
            Ty::Slice(inner) => {
                Some(TyKind::Slice(self.lower_ty(inner, table, projection_vars)?).intern(INTER))
            }
            Ty::Reference {
                lifetime,
                mutability,
                inner,
            } => Some(
                TyKind::Ref(
                    Self::lower_mutability(*mutability),
                    self.lower_lifetime(*lifetime)?,
                    self.lower_ty(inner, table, projection_vars)?,
                )
                .intern(INTER),
            ),
            Ty::RawPointer { mutability, inner } => Some(
                TyKind::Raw(
                    Self::lower_mutability(*mutability),
                    self.lower_ty(inner, table, projection_vars)?,
                )
                .intern(INTER),
            ),
            Ty::FnPointer { params, ret } => {
                // Chalk represents every function pointer signature under a binder, even when
                // `num_binders` is zero. Move query-owned variables through that layer so its
                // empty substitution cannot mistake `^0` for a late-bound function parameter.
                let mut signature = params
                    .iter()
                    .map(|param| {
                        self.lower_ty(param, table, projection_vars)
                            .map(|ty| GenericArgData::Ty(ty.shifted_in(INTER)).intern(INTER))
                    })
                    .collect::<Option<Vec<_>>>()?;
                signature.push(
                    GenericArgData::Ty(
                        self.lower_ty(ret, table, projection_vars)?
                            .shifted_in(INTER),
                    )
                    .intern(INTER),
                );
                Some(
                    TyKind::Function(FnPointer {
                        num_binders: 0,
                        sig: FnSig {
                            abi: (),
                            safety: Safety::Safe,
                            variadic: false,
                        },
                        substitution: FnSubst(ChalkSubstitution::from_iter(INTER, signature)),
                    })
                    .intern(INTER),
                )
            }
            Ty::Adt(adt) => Some(
                TyKind::Adt(
                    AdtId(adt.def),
                    self.lower_args(&adt.args, table, projection_vars)?,
                )
                .intern(INTER),
            ),
            Ty::Param(param) => self
                .binders
                .bound_var(GenericParamRef::Type(*param))
                .map(|param| param.to_ty::<RgChalkInterner>(INTER)),
            Ty::Alias(AliasTy::Projection(alias)) => {
                if !self.supports_associated_ty(alias.associated_ty) {
                    return None;
                }
                Some(
                    TyKind::Alias(ChalkAliasTy::Projection(ChalkProjectionTy {
                        associated_ty_id: chalk_assoc_type_id(alias.associated_ty),
                        substitution: self.lower_args(&alias.args, table, projection_vars)?,
                    }))
                    .intern(INTER),
                )
            }
            Ty::Alias(AliasTy::Opaque(alias)) => Some(
                TyKind::OpaqueType(
                    chalk_opaque_ty_id(alias.opaque),
                    self.lower_args(&alias.args, table, projection_vars)?,
                )
                .intern(INTER),
            ),
            Ty::Closure(closure) => {
                let mut signature = closure
                    .params
                    .iter()
                    .map(|param| {
                        self.lower_ty(param, table, projection_vars)
                            .map(|ty| GenericArgData::Ty(ty).intern(INTER))
                    })
                    .collect::<Option<Vec<_>>>()?;
                signature.push(
                    GenericArgData::Ty(self.lower_ty(&closure.ret, table, projection_vars)?)
                        .intern(INTER),
                );
                Some(
                    TyKind::Closure(
                        chalk_ir::ClosureId(ChalkDefId::Closure(closure.id)),
                        ChalkSubstitution::from_iter(INTER, signature),
                    )
                    .intern(INTER),
                )
            }
            Ty::FnDef(function) => {
                let datum = self.functions?.get(&function.def)?;
                // Chalk substitutes function-item arguments into the datum without checking its
                // arity first. Decline malformed or partially instantiated items at this adapter
                // boundary instead of letting the solver index past the supplied arguments.
                if datum.binders.len(INTER) != function.args.len() {
                    return None;
                }
                Some(
                    TyKind::FnDef(
                        FnDefId(ChalkDefId::Function(function.def)),
                        self.lower_args(&function.args, table, projection_vars)?,
                    )
                    .intern(INTER),
                )
            }
            Ty::InferVar {
                kind: InferVarKind::Type,
                id,
            } => projection_vars?.chalk_ty_for_var(*id),
            Ty::InferVar { .. } | Ty::Unknown => None,
        }
    }

    fn lower_lifetime(&self, lifetime: Lifetime) -> Option<chalk_ir::Lifetime<RgChalkInterner>> {
        Some(match lifetime {
            Lifetime::Static => LifetimeData::Static.intern(INTER),
            Lifetime::Erased => LifetimeData::Erased.intern(INTER),
            Lifetime::Param(param) => {
                LifetimeData::BoundVar(self.binders.bound_var(GenericParamRef::Lifetime(param))?)
                    .intern(INTER)
            }
        })
    }

    fn lower_const(&self, value: ConstValue) -> Option<chalk_ir::Const<RgChalkInterner>> {
        let value = match value {
            ConstValue::Scalar(value) => ChalkConstValue::Concrete(ConcreteConst {
                interned: value.to_string(),
            }),
            ConstValue::Param(param) => {
                ChalkConstValue::BoundVar(self.binders.bound_var(GenericParamRef::Const(param))?)
            }
            ConstValue::Unknown => return None,
        };
        Some(
            ConstData {
                ty: usize_ty(),
                value,
            }
            .intern(INTER),
        )
    }

    fn lower_primitive(primitive: PrimitiveTy) -> ChalkTy {
        match primitive {
            PrimitiveTy::Str => TyKind::Str.intern(INTER),
            PrimitiveTy::Bool => TyKind::Scalar(Scalar::Bool).intern(INTER),
            PrimitiveTy::Char => TyKind::Scalar(Scalar::Char).intern(INTER),
            PrimitiveTy::SignedInt(kind) => TyKind::Scalar(Scalar::Int(match kind {
                SignedIntTy::I8 => chalk_ir::IntTy::I8,
                SignedIntTy::I16 => chalk_ir::IntTy::I16,
                SignedIntTy::I32 => chalk_ir::IntTy::I32,
                SignedIntTy::I64 => chalk_ir::IntTy::I64,
                SignedIntTy::I128 => chalk_ir::IntTy::I128,
                SignedIntTy::Isize => chalk_ir::IntTy::Isize,
            }))
            .intern(INTER),
            PrimitiveTy::UnsignedInt(kind) => TyKind::Scalar(Scalar::Uint(match kind {
                UnsignedIntTy::U8 => chalk_ir::UintTy::U8,
                UnsignedIntTy::U16 => chalk_ir::UintTy::U16,
                UnsignedIntTy::U32 => chalk_ir::UintTy::U32,
                UnsignedIntTy::U64 => chalk_ir::UintTy::U64,
                UnsignedIntTy::U128 => chalk_ir::UintTy::U128,
                UnsignedIntTy::Usize => UintTy::Usize,
            }))
            .intern(INTER),
            PrimitiveTy::Float(kind) => TyKind::Scalar(Scalar::Float(match kind {
                FloatTy::F32 => chalk_ir::FloatTy::F32,
                FloatTy::F64 => chalk_ir::FloatTy::F64,
            }))
            .intern(INTER),
        }
    }

    fn lower_mutability(mutability: Mutability) -> ChalkMutability {
        match mutability {
            Mutability::Shared => ChalkMutability::Not,
            Mutability::Mutable => ChalkMutability::Mut,
        }
    }

    fn supports_associated_ty(&self, associated_ty: TypeAliasRef) -> bool {
        self.associated_tys
            .is_some_and(|associated_tys| associated_tys.contains_key(&associated_ty))
    }
}

pub(super) fn chalk_trait_id(trait_ref: TraitDefRef) -> TraitId<RgChalkInterner> {
    TraitId(ChalkDefId::Trait(trait_ref))
}

pub(super) fn chalk_impl_id(impl_ref: ImplRef) -> chalk_ir::ImplId<RgChalkInterner> {
    chalk_ir::ImplId(ChalkDefId::Impl(impl_ref))
}

pub(super) fn chalk_assoc_type_id(type_alias_ref: TypeAliasRef) -> AssocTypeId<RgChalkInterner> {
    AssocTypeId(ChalkDefId::AssocType(type_alias_ref))
}

pub(super) fn chalk_assoc_type_value_id(
    type_alias_ref: TypeAliasRef,
) -> AssociatedTyValueId<RgChalkInterner> {
    AssociatedTyValueId(ChalkDefId::AssocTypeValue(type_alias_ref))
}

pub(super) fn chalk_opaque_ty_id(opaque: rg_ir_model::OpaqueTyRef) -> OpaqueTyId<RgChalkInterner> {
    OpaqueTyId(ChalkDefId::Opaque(opaque))
}

pub(super) fn stub_trait_datum(
    trait_ref: TraitDefRef,
    parameter_count: usize,
) -> TraitDatum<RgChalkInterner> {
    let binders = VariableKinds::from_iter(
        INTER,
        (0..parameter_count).map(|_| VariableKind::Ty(TyVariableKind::General)),
    );
    TraitDatum {
        id: chalk_trait_id(trait_ref),
        binders: chalk_ir::Binders::new(
            binders,
            TraitDatumBound {
                where_clauses: Vec::new(),
            },
        ),
        flags: TraitFlags {
            auto: false,
            marker: false,
            upstream: false,
            fundamental: false,
            non_enumerable: false,
            coinductive: false,
        },
        associated_ty_ids: Vec::new(),
        well_known: None,
    }
}

pub(super) fn adt_datum(
    type_def: TypeDefRef,
    generics: Option<&Generics<'_>>,
) -> AdtDatum<RgChalkInterner> {
    let binders = generics
        .map(GenericBinderEnv::for_generics)
        .unwrap_or_else(GenericBinderEnv::empty);
    let kind = match type_def.id {
        TypeDefId::Struct(_) => AdtKind::Struct,
        TypeDefId::Enum(_) => AdtKind::Enum,
        TypeDefId::Union(_) => AdtKind::Union,
    };
    AdtDatum {
        binders: chalk_ir::Binders::new(
            binders.variable_kinds(),
            AdtDatumBound {
                variants: match kind {
                    AdtKind::Struct | AdtKind::Union => {
                        vec![AdtVariantDatum { fields: Vec::new() }]
                    }
                    AdtKind::Enum => Vec::new(),
                },
                where_clauses: Vec::new(),
            },
        ),
        id: AdtId(type_def),
        flags: AdtFlags {
            upstream: false,
            fundamental: false,
            phantom_data: false,
        },
        kind,
    }
}

pub(super) fn unit_ty() -> ChalkTy {
    TyKind::Tuple(0, ChalkSubstitution::empty(INTER)).intern(INTER)
}

fn usize_ty() -> ChalkTy {
    TyKind::Scalar(Scalar::Uint(UintTy::Usize)).intern(INTER)
}
