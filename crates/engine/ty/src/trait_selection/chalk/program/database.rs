//! Chalk's read-only view of the materialized semantic program.
//!
//! The build phase stores project identities and already-lowered datums. This module only answers
//! Chalk callbacks from those maps. It does not resolve source paths or lower `TypeRef` again.
//!
//! `RustIrDatabase` also requires callbacks for language features outside rust-glancer's bounded
//! solver model. Unsupported features retain inert data, while callable functions and closures
//! are backed by canonical signatures materialized at the query boundary.

use std::sync::Arc;

use chalk_ir::fold::Shift;
use chalk_ir::{
    AdtId, AssocTypeId, Binders, CanonicalVarKinds, ClosureId, CoroutineId, FnDefId, GenericArg,
    GenericArgData, OpaqueTyId, ProgramClause, ProgramClauses, Substitution, Ty, TyKind,
    UnificationDatabase, Variance, Variances,
};
use chalk_solve::RustIrDatabase;
use chalk_solve::rust_ir::{
    AdtRepr, AdtSizeAlign, AssociatedTyDatum, AssociatedTyValue, AssociatedTyValueBound,
    AssociatedTyValueId, ClosureKind, CoroutineDatum, CoroutineInputOutputDatum,
    CoroutineWitnessDatum, FnDefDatum, FnDefInputsAndOutputDatum, ImplDatum, Movability,
    OpaqueTyDatum, OpaqueTyDatumBound, Polarity, TraitDatum, WellKnownAssocType, WellKnownTrait,
};
use rg_ir_model::{ImplRef, TraitDefRef, TypeAliasRef, TypeDefRef};

use super::super::interner::{ChalkDefId, RgChalkInterner};
use super::super::lower::{
    adt_datum, chalk_assoc_type_value_id, chalk_impl_id, chalk_trait_id, stub_trait_datum, unit_ty,
};
use super::ChalkProgram;

const INTER: RgChalkInterner = RgChalkInterner;
const UNKNOWN_ADT_VARIANCE_SLOTS: usize = 32;

/// Coarse outer shape used to discard impls that cannot match Chalk's concrete `Self` hint.
///
/// This is deliberately less precise than type equality. A missing head means "keep the impl":
/// parameters, aliases, and other solver-owned shapes may still match after normalization.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ChalkTyHead {
    Adt(TypeDefRef),
    Tuple(usize),
    Array,
    Slice,
    Reference,
    RawPointer,
    Scalar,
    Str,
    Never,
    Function,
    FnDef,
    Closure,
    Coroutine,
    CoroutineWitness,
}

impl ChalkTyHead {
    fn from_arg(arg: &GenericArg<RgChalkInterner>) -> Option<Self> {
        let GenericArgData::Ty(ty) = arg.data(INTER) else {
            return None;
        };
        Self::from_ty(ty)
    }

    fn from_ty(ty: &Ty<RgChalkInterner>) -> Option<Self> {
        Some(match ty.kind(INTER) {
            TyKind::Adt(adt, _) => Self::Adt(adt.0),
            TyKind::Tuple(arity, _) => Self::Tuple(*arity),
            TyKind::Array(_, _) => Self::Array,
            TyKind::Slice(_) => Self::Slice,
            TyKind::Ref(_, _, _) => Self::Reference,
            TyKind::Raw(_, _) => Self::RawPointer,
            TyKind::Scalar(_) => Self::Scalar,
            TyKind::Str => Self::Str,
            TyKind::Never => Self::Never,
            TyKind::Function(_) => Self::Function,
            TyKind::FnDef(_, _) => Self::FnDef,
            TyKind::Closure(_, _) => Self::Closure,
            TyKind::Coroutine(_, _) => Self::Coroutine,
            TyKind::CoroutineWitness(_, _) => Self::CoroutineWitness,
            TyKind::AssociatedType(_, _)
            | TyKind::OpaqueType(_, _)
            | TyKind::Alias(_)
            | TyKind::Foreign(_)
            | TyKind::Error
            | TyKind::Placeholder(_)
            | TyKind::Dyn(_)
            | TyKind::BoundVar(_)
            | TyKind::InferenceVar(_, _) => return None,
        })
    }
}

impl ChalkProgram {
    fn stub_trait(&self, trait_ref: TraitDefRef) -> Arc<TraitDatum<RgChalkInterner>> {
        let arity = self.trait_arities.get(&trait_ref).copied().unwrap_or(1);
        Arc::new(stub_trait_datum(trait_ref, arity))
    }

    fn stub_adt(
        &self,
        type_def: TypeDefRef,
    ) -> Arc<chalk_solve::rust_ir::AdtDatum<RgChalkInterner>> {
        Arc::new(adt_datum(type_def, None))
    }
}

impl UnificationDatabase<RgChalkInterner> for ChalkProgram {
    fn fn_def_variance(&self, _fn_def_id: FnDefId<RgChalkInterner>) -> Variances<RgChalkInterner> {
        Variances::empty(INTER)
    }

    fn adt_variance(&self, adt_id: AdtId<RgChalkInterner>) -> Variances<RgChalkInterner> {
        self.adt_variances
            .get(&adt_id.0)
            .cloned()
            .unwrap_or_else(|| {
                Variances::from_iter(
                    INTER,
                    (0..UNKNOWN_ADT_VARIANCE_SLOTS).map(|_| Variance::Invariant),
                )
            })
    }
}

impl RustIrDatabase<RgChalkInterner> for ChalkProgram {
    fn custom_clauses(&self) -> Vec<ProgramClause<RgChalkInterner>> {
        Vec::new()
    }

    fn associated_ty_data(
        &self,
        ty: AssocTypeId<RgChalkInterner>,
    ) -> Arc<AssociatedTyDatum<RgChalkInterner>> {
        let ChalkDefId::AssocType(type_alias_ref) = ty.0 else {
            unreachable!("Chalk associated-type callbacks should carry associated-type IDs");
        };
        self.associated_tys
            .get(&type_alias_ref)
            .cloned()
            .expect("Chalk lowering should reject associated types without solver data")
    }

    fn trait_datum(
        &self,
        trait_id: chalk_ir::TraitId<RgChalkInterner>,
    ) -> Arc<TraitDatum<RgChalkInterner>> {
        let ChalkDefId::Trait(trait_ref) = trait_id.0 else {
            return self.stub_trait(TraitDefRef {
                origin: rg_ir_model::DefMapRef::Crate(rg_ir_model::CrateRef {
                    package: rg_ir_model::PackageSlot(0),
                    crate_id: rg_ir_model::CrateId(0),
                }),
                id: rg_ir_model::TraitId(0),
            });
        };
        self.traits
            .get(&trait_ref)
            .cloned()
            .unwrap_or_else(|| self.stub_trait(trait_ref))
    }

    fn adt_datum(
        &self,
        adt_id: AdtId<RgChalkInterner>,
    ) -> Arc<chalk_solve::rust_ir::AdtDatum<RgChalkInterner>> {
        self.adts
            .get(&adt_id.0)
            .cloned()
            .unwrap_or_else(|| self.stub_adt(adt_id.0))
    }

    fn coroutine_datum(
        &self,
        coroutine_id: CoroutineId<RgChalkInterner>,
    ) -> Arc<CoroutineDatum<RgChalkInterner>> {
        let _ = coroutine_id;
        Arc::new(CoroutineDatum {
            movability: Movability::Static,
            input_output: Binders::empty(
                INTER,
                CoroutineInputOutputDatum {
                    resume_type: unit_ty(),
                    yield_type: unit_ty(),
                    return_type: unit_ty(),
                    upvars: Vec::new(),
                },
            ),
        })
    }

    fn coroutine_witness_datum(
        &self,
        coroutine_id: CoroutineId<RgChalkInterner>,
    ) -> Arc<CoroutineWitnessDatum<RgChalkInterner>> {
        let _ = coroutine_id;
        Arc::new(CoroutineWitnessDatum {
            inner_types: Binders::empty(
                INTER,
                chalk_solve::rust_ir::CoroutineWitnessExistential {
                    types: Binders::empty(INTER, Vec::new()),
                },
            ),
        })
    }

    fn adt_repr(&self, _id: AdtId<RgChalkInterner>) -> Arc<AdtRepr<RgChalkInterner>> {
        Arc::new(AdtRepr {
            c: false,
            packed: false,
            int: None,
        })
    }

    fn adt_size_align(&self, _id: AdtId<RgChalkInterner>) -> Arc<AdtSizeAlign> {
        Arc::new(AdtSizeAlign::from_one_zst(false))
    }

    fn fn_def_datum(
        &self,
        fn_def_id: FnDefId<RgChalkInterner>,
    ) -> Arc<FnDefDatum<RgChalkInterner>> {
        let ChalkDefId::Function(function) = fn_def_id.0 else {
            panic!("Chalk function callbacks should carry function IDs");
        };
        self.functions
            .get(&function)
            .cloned()
            .expect("function types should be materialized before solving")
    }

    fn impl_datum(
        &self,
        impl_id: chalk_ir::ImplId<RgChalkInterner>,
    ) -> Arc<ImplDatum<RgChalkInterner>> {
        let ChalkDefId::Impl(impl_ref) = impl_id.0 else {
            return Arc::new(ImplDatum {
                polarity: Polarity::Negative,
                binders: Binders::empty(
                    INTER,
                    chalk_solve::rust_ir::ImplDatumBound {
                        trait_ref: chalk_solve::rust_ir::TraitBound {
                            trait_id: chalk_trait_id(TraitDefRef {
                                origin: rg_ir_model::DefMapRef::Crate(rg_ir_model::CrateRef {
                                    package: rg_ir_model::PackageSlot(0),
                                    crate_id: rg_ir_model::CrateId(0),
                                }),
                                id: rg_ir_model::TraitId(0),
                            }),
                            args_no_self: Vec::new(),
                        }
                        .as_trait_ref(INTER, unit_ty()),
                        where_clauses: Vec::new(),
                    },
                ),
                impl_type: chalk_solve::rust_ir::ImplType::External,
                associated_ty_value_ids: Vec::new(),
            });
        };
        self.impls.get(&impl_ref).cloned().unwrap_or_else(|| {
            Arc::new(ImplDatum {
                polarity: Polarity::Negative,
                binders: Binders::empty(
                    INTER,
                    chalk_solve::rust_ir::ImplDatumBound {
                        trait_ref: chalk_ir::TraitRef {
                            trait_id: chalk_trait_id(TraitDefRef {
                                origin: impl_ref.origin,
                                id: rg_ir_model::TraitId(0),
                            }),
                            substitution: Substitution::from_iter(
                                INTER,
                                [chalk_ir::GenericArgData::Ty(unit_ty()).intern(INTER)],
                            ),
                        },
                        where_clauses: Vec::new(),
                    },
                ),
                impl_type: chalk_solve::rust_ir::ImplType::External,
                associated_ty_value_ids: Vec::new(),
            })
        })
    }

    fn associated_ty_from_impl(
        &self,
        impl_id: chalk_ir::ImplId<RgChalkInterner>,
        assoc_type_id: AssocTypeId<RgChalkInterner>,
    ) -> Option<AssociatedTyValueId<RgChalkInterner>> {
        let (ChalkDefId::Impl(impl_ref), ChalkDefId::AssocType(assoc_type_ref)) =
            (impl_id.0, assoc_type_id.0)
        else {
            return None;
        };
        self.associated_ty_value_by_impl
            .get(&(impl_ref, assoc_type_ref))
            .copied()
            .map(chalk_assoc_type_value_id)
    }

    fn associated_ty_value(
        &self,
        id: AssociatedTyValueId<RgChalkInterner>,
    ) -> Arc<AssociatedTyValue<RgChalkInterner>> {
        if let ChalkDefId::AssocTypeValue(type_alias_ref) = id.0
            && let Some(value) = self.associated_ty_values.get(&type_alias_ref)
        {
            return value.clone();
        }

        Arc::new(AssociatedTyValue {
            impl_id: chalk_impl_id(ImplRef {
                origin: rg_ir_model::DefMapRef::Crate(rg_ir_model::CrateRef {
                    package: rg_ir_model::PackageSlot(0),
                    crate_id: rg_ir_model::CrateId(0),
                }),
                id: rg_ir_model::ImplId(0),
            }),
            associated_ty_id: AssocTypeId(ChalkDefId::AssocType(TypeAliasRef {
                origin: rg_ir_model::DefMapRef::Crate(rg_ir_model::CrateRef {
                    package: rg_ir_model::PackageSlot(0),
                    crate_id: rg_ir_model::CrateId(0),
                }),
                id: rg_ir_model::TypeAliasId(0),
            })),
            value: Binders::empty(INTER, AssociatedTyValueBound { ty: unit_ty() }),
        })
    }

    fn opaque_ty_data(
        &self,
        id: OpaqueTyId<RgChalkInterner>,
    ) -> Arc<OpaqueTyDatum<RgChalkInterner>> {
        if let ChalkDefId::Opaque(opaque) = id.0
            && let Some(datum) = self.opaque_tys.get(&opaque)
        {
            return datum.clone();
        }
        Arc::new(OpaqueTyDatum {
            opaque_ty_id: id,
            bound: Binders::empty(
                INTER,
                OpaqueTyDatumBound {
                    bounds: Binders::empty(INTER, Vec::new()),
                    where_clauses: Binders::empty(INTER, Vec::new()),
                },
            ),
        })
    }

    fn hidden_opaque_type(
        &self,
        _id: OpaqueTyId<RgChalkInterner>,
    ) -> chalk_ir::Ty<RgChalkInterner> {
        unit_ty()
    }

    fn impls_for_trait(
        &self,
        trait_id: chalk_ir::TraitId<RgChalkInterner>,
        parameters: &[GenericArg<RgChalkInterner>],
        _binders: &CanonicalVarKinds<RgChalkInterner>,
    ) -> Vec<chalk_ir::ImplId<RgChalkInterner>> {
        let ChalkDefId::Trait(trait_ref) = trait_id.0 else {
            return Vec::new();
        };
        let self_head = parameters.first().and_then(ChalkTyHead::from_arg);
        let Some(impls) = self.impls_by_trait.get(&trait_ref) else {
            return Vec::new();
        };
        impls
            .iter()
            .copied()
            .filter(|impl_ref| {
                let Some(self_head) = self_head else {
                    return true;
                };
                let impl_head = self
                    .impls
                    .get(impl_ref)
                    .and_then(|datum| {
                        datum
                            .binders
                            .skip_binders()
                            .trait_ref
                            .substitution
                            .as_slice(INTER)
                            .first()
                    })
                    .and_then(ChalkTyHead::from_arg);

                // Chalk accepts any superset of applicable impls. Concrete mismatched heads are
                // impossible candidates; a generic or otherwise unclassified head stays in the
                // set so blanket impls and normalization-dependent matches remain visible.
                impl_head.is_none_or(|impl_head| impl_head == self_head)
            })
            .map(chalk_impl_id)
            .collect()
    }

    fn local_impls_to_coherence_check(
        &self,
        trait_id: chalk_ir::TraitId<RgChalkInterner>,
    ) -> Vec<chalk_ir::ImplId<RgChalkInterner>> {
        self.impls_for_trait(trait_id, &[], &CanonicalVarKinds::empty(INTER))
    }

    fn impl_provided_for(
        &self,
        _auto_trait_id: chalk_ir::TraitId<RgChalkInterner>,
        _ty: &TyKind<RgChalkInterner>,
    ) -> bool {
        false
    }

    fn well_known_trait_id(
        &self,
        well_known_trait: WellKnownTrait,
    ) -> Option<chalk_ir::TraitId<RgChalkInterner>> {
        self.known_items
            .trait_ref(well_known_trait)
            .map(chalk_trait_id)
    }

    fn well_known_assoc_type_id(
        &self,
        _assoc_type: WellKnownAssocType,
    ) -> Option<AssocTypeId<RgChalkInterner>> {
        None
    }

    fn program_clauses_for_env(
        &self,
        environment: &chalk_ir::Environment<RgChalkInterner>,
    ) -> ProgramClauses<RgChalkInterner> {
        chalk_solve::program_clauses_for_env(self, environment)
    }

    fn interner(&self) -> RgChalkInterner {
        INTER
    }

    fn is_object_safe(&self, _trait_id: chalk_ir::TraitId<RgChalkInterner>) -> bool {
        false
    }

    // Capture analysis is not modeled yet. Treat closures as `Fn`, the most capable callable kind,
    // preserving the bounded behavior that they may satisfy `Fn`, `FnMut`, and `FnOnce`.
    fn closure_kind(
        &self,
        _closure_id: ClosureId<RgChalkInterner>,
        _substs: &Substitution<RgChalkInterner>,
    ) -> ClosureKind {
        ClosureKind::Fn
    }

    fn closure_inputs_and_output(
        &self,
        _closure_id: ClosureId<RgChalkInterner>,
        substs: &Substitution<RgChalkInterner>,
    ) -> Binders<FnDefInputsAndOutputDatum<RgChalkInterner>> {
        let (return_type, params) = substs
            .as_slice(INTER)
            .split_last()
            .expect("closure substitutions always contain an output type");
        let argument_types = params
            .iter()
            .map(|arg| {
                let GenericArgData::Ty(ty) = arg.data(INTER) else {
                    panic!("closure signatures contain only type arguments");
                };
                // The callback returns a binder even though this bounded model has no late-bound
                // closure variables. Move query-owned variables through that empty binder so
                // Chalk does not mistake them for parameters that the callback failed to supply.
                ty.clone().shifted_in(INTER)
            })
            .collect();
        let GenericArgData::Ty(return_type) = return_type.data(INTER) else {
            panic!("closure output should be a type argument");
        };
        Binders::empty(
            INTER,
            FnDefInputsAndOutputDatum {
                argument_types,
                return_type: return_type.clone().shifted_in(INTER),
            },
        )
    }

    fn closure_upvars(
        &self,
        _closure_id: ClosureId<RgChalkInterner>,
        _substs: &Substitution<RgChalkInterner>,
    ) -> Binders<chalk_ir::Ty<RgChalkInterner>> {
        Binders::empty(INTER, unit_ty())
    }

    fn closure_fn_substitution(
        &self,
        _closure_id: ClosureId<RgChalkInterner>,
        _substs: &Substitution<RgChalkInterner>,
    ) -> Substitution<RgChalkInterner> {
        // Closure signatures contain no late-bound variables in the supported model.
        Substitution::empty(INTER)
    }

    fn unification_database(&self) -> &dyn UnificationDatabase<RgChalkInterner> {
        self
    }

    fn trait_name(&self, trait_id: chalk_ir::TraitId<RgChalkInterner>) -> String {
        match trait_id.0 {
            ChalkDefId::Trait(trait_ref) => format!("{trait_ref:?}"),
            other => format!("{other:?}"),
        }
    }

    fn adt_name(&self, adt_id: AdtId<RgChalkInterner>) -> String {
        format!("{:?}", adt_id.0)
    }

    fn assoc_type_name(&self, assoc_ty_id: AssocTypeId<RgChalkInterner>) -> String {
        format!("{:?}", assoc_ty_id.0)
    }

    fn opaque_type_name(&self, opaque_ty_id: OpaqueTyId<RgChalkInterner>) -> String {
        format!("{:?}", opaque_ty_id.0)
    }

    fn fn_def_name(&self, fn_def_id: FnDefId<RgChalkInterner>) -> String {
        format!("{:?}", fn_def_id.0)
    }

    fn discriminant_type(
        &self,
        _ty: chalk_ir::Ty<RgChalkInterner>,
    ) -> chalk_ir::Ty<RgChalkInterner> {
        TyKind::Scalar(chalk_ir::Scalar::Uint(chalk_ir::UintTy::Usize)).intern(INTER)
    }
}
