//! Call lookup retains the trial context that established a receiver's applicability.
//!
//! For `value.method()`, compare the method's self parameter with the receiver adjustments.
//! An impl for `Widget` can take `&Widget`, while an impl for `&Widget` can take `&&Widget`.
//! The first matching adjustment wins; inherent methods take precedence at the same adjustment.
//! The chosen target retains its receiver bindings so its signature uses the same variables as
//! the body.

use rg_def_map::DefMapSource;
use rg_ir_model::{
    ExprId, FunctionRef, GenericDefRef, ScopeId, SemanticItemRef, identity::DeclarationRef,
};
use rg_item_tree::{FieldList, GenericArg};
use rg_semantic_ir::{Generics, SemanticItemSource};
use rg_source::ReadError;
use rg_ty::{
    ConstructorRef, FnDefRef, Ty as StoredTy,
    lowering::{TypeLoweringAnchor, TypeLoweringEnv, TypeLoweringQuery},
    solver::{DefId, FnDefTy, InferenceSubstitution, InferenceTable, Outcome, Ty},
};

use super::{FunctionLookup, LiveBodyQuery, member::MemberCandidate};
use crate::{
    BodyAssociatedPathPrefix,
    body::{ExprKind, facts::BodyResolution},
};

/// A function candidate together with any receiver evidence found while considering it.
/// Member lookup keeps that evidence in a separate table so candidates cannot constrain one
/// another. A directly resolved function needs no trial; its signature uses the body's table.
pub(crate) struct LiveCallTarget<'s> {
    pub function: FunctionRef,
    pub subst: InferenceSubstitution<'s>,
    // For a dot call, this includes the borrow or dereference that matched the self parameter.
    pub receiver: Option<Ty<'s>>,
    pub table: Option<InferenceTable<'s>>,
    // Lookup can retain a declaration for navigation even when its trial cannot supply types.
    can_infer: bool,
}

impl<'s> LiveCallTarget<'s> {
    fn from_member(candidate: MemberCandidate<'s>) -> Self {
        Self {
            function: candidate.function,
            subst: candidate.subst,
            receiver: None,
            table: Some(candidate.table),
            // A possible proof can learn from call arguments later. Missing callback data
            // still leaves a navigation candidate, but cannot supply inference evidence.
            can_infer: matches!(candidate.outcome, Outcome::Proven | Outcome::Ambiguous),
        }
    }

    pub(crate) fn unique_usable(targets: Vec<Self>) -> Option<Self> {
        let mut usable = targets.into_iter().filter(|target| target.can_infer);
        match (usable.next(), usable.next()) {
            (Some(target), None) => Some(target),
            _ => None,
        }
    }
}

impl<'query, D, I> LiveBodyQuery<'query, D, I>
where
    D: DefMapSource + Copy,
    I: SemanticItemSource<'query> + Copy,
{
    /// Apply arguments written on the callable. The impl or trait's arguments belong to the
    /// receiver or qualifier and must keep the bindings established by lookup.
    pub(crate) fn apply_function_args<'s>(
        &self,
        scope: ScopeId,
        generics: &Generics<'_>,
        explicit: &[GenericArg],
        table: &InferenceTable<'s>,
        subst: &mut InferenceSubstitution<'s>,
    ) -> Result<(), ReadError> {
        if !explicit.is_empty() {
            let args = self.generic_args(scope, generics, explicit, table)?;
            for (param, arg) in generics
                .iter_self()
                .zip(args.iter().skip(generics.parent_len()))
            {
                subst.insert(param.param(), arg);
            }
        }
        Ok(())
    }

    /// Compare the instantiated return type with the expected type. For `fn pair<U>() -> (T, U)`
    /// on `Packet<T>`, `Packet::<u8>::pair::<u32>()` gets `T = u8` from the qualifier and
    /// `U = u32` from the turbofish.
    /// Arguments still missing after lookup and syntax lowering stay unknown.
    pub(crate) fn function_return_matches<'s>(
        &self,
        scope: ScopeId,
        function: FunctionRef,
        table: &InferenceTable<'s>,
        subst: &InferenceSubstitution<'s>,
        expected: (&StoredTy, &[GenericArg]),
    ) -> Result<bool, ReadError> {
        let (expected, explicit) = expected;
        let trial;
        let mut explicit_subst;
        let (table, subst) = if explicit.is_empty() {
            (table, subst)
        } else {
            let generics = self
                .context
                .item_paths()
                .generics()
                .generics(GenericDefRef::Function(function))?;
            if generics.iter_self().next().is_none() {
                return Ok(false);
            }
            // An argument such as `Vec<_>` creates variables for this candidate's check.
            trial = table.probe();
            explicit_subst = subst.clone();
            self.apply_function_args(scope, &generics, explicit, &trial, &mut explicit_subst)?;
            (&trial, &explicit_subst)
        };
        let cx = table.interner();
        let params = cx.params(DefId::Function(function));
        Ok(FnDefTy {
            def: function.into(),
            args: subst.args_for(cx, params.iter().copied()),
        }
        .signature(cx)
        .is_some_and(|signature| table.resolves_to(signature.call_return, expected)))
    }

    /// Keep the identity and live generic arguments of a tuple constructor when it becomes a value.
    /// For `let make = Wrapper; make(true)`, the parameter and `Wrapper` result share `?T`.
    pub(crate) fn constructor_value_ty<'s>(
        &self,
        ty: Ty<'s>,
        resolution: &BodyResolution,
        table: &InferenceTable<'s>,
    ) -> Result<Ty<'s>, ReadError> {
        let (Some(adt), BodyResolution::Declarations(declarations)) = (ty.as_adt(), resolution)
        else {
            return Ok(ty);
        };
        let items = self.context.item_query();
        let (fields, constructor) = match declarations.as_one().copied() {
            Some(DeclarationRef::Item(SemanticItemRef::TypeDef(def))) => (
                items.struct_fields_for_type_def(def)?,
                ConstructorRef::Struct(def),
            ),
            Some(DeclarationRef::EnumVariant(variant)) => (
                items
                    .enum_variant_data(variant)?
                    .map(|data| &data.variant.fields),
                ConstructorRef::Variant(variant),
            ),
            _ => return Ok(ty),
        };
        if constructor.owner() != adt.def || !matches!(fields, Some(FieldList::Tuple(_))) {
            // Unit structs and unit variants already have the type of the constructed value.
            return Ok(ty);
        }
        let adt = table
            .instantiate_nested_unknowns(ty)
            .as_adt()
            .expect("instantiation preserves the nominal type");
        Ok(table
            .interner()
            .fn_def(FnDefRef::Constructor(constructor), adt.args))
    }

    /// Find the best dot-call candidates in receiver adjustment order. Ties remain useful
    /// for navigation even when inference cannot choose a signature.
    pub(crate) fn method_targets<'s>(
        &self,
        call: ExprId,
        receiver: Ty<'s>,
        table: &InferenceTable<'s>,
    ) -> Result<Vec<LiveCallTarget<'s>>, ReadError> {
        let data = self.context.body().expr_unchecked(call);
        let ExprKind::MethodCall { method_name, .. } = &data.kind else {
            unreachable!("method lookup owns a dot call")
        };
        // The autoderef types tell us where to look for declarations. The adjustments
        // determine which self parameter wins, including the possible borrows of each type.
        let self_types = table.method_receivers(receiver).collect::<Vec<_>>();
        let receivers = table
            .method_receiver_adjustments(&self_types)
            .collect::<Vec<_>>();
        let items = self.context.item_query();
        let mut selected = Vec::new();
        let mut best_priority = None;
        for self_ty in self_types {
            for candidate in self.function_candidates(
                data.scope,
                self_ty,
                FunctionLookup::Method(method_name),
                table,
            )? {
                let Some(function) = items.function_data(candidate.function)? else {
                    continue;
                };
                let Some((priority, receiver)) =
                    candidate.match_receiver(function, self_ty, &receivers)
                else {
                    continue;
                };

                // Keep all ties for navigation. Only one usable target can supply the call
                // signature, with the receiver adjustment and bindings that matched it.
                if best_priority.is_some_and(|best| priority > best) {
                    continue;
                }
                if best_priority != Some(priority) {
                    selected.clear();
                    best_priority = Some(priority);
                }
                let mut target = LiveCallTarget::from_member(candidate);
                target.receiver = Some(receiver);
                selected.push(target);
            }
        }
        Ok(selected)
    }

    /// Resolve a path naming a function item, whether or not the item is called here.
    pub(crate) fn associated_function_targets<'s>(
        &self,
        scope: ScopeId,
        prefix: &BodyAssociatedPathPrefix,
        name: &str,
        table: &InferenceTable<'s>,
    ) -> Result<Vec<LiveCallTarget<'s>>, ReadError> {
        // `Widget::read` supplies Self = Widget; `<Widget as Read<u8>>::read` also fixes
        // the trait and its arguments. All of these constraints come from the path.
        let (receiver, qualification) = match prefix {
            BodyAssociatedPathPrefix::Type(ty) => (self.type_ref(scope, ty, table)?, None),
            BodyAssociatedPathPrefix::QualifiedTrait { self_ty, trait_ref } => {
                let paths = self.context.item_paths();
                let lowering = TypeLoweringQuery::new(&paths, &self.context);
                let mut session = lowering.session(
                    table.interner(),
                    TypeLoweringEnv::new(
                        self.context.body().owner().generic_def(),
                        TypeLoweringAnchor::Scope(scope),
                    ),
                )?;
                let receiver = session.lower_type_ref_with_inference(self_ty, table)?;
                let qualification = session
                    .lower_trait_ref(trait_ref, receiver)?
                    .map(|tr| tr.application);
                (receiver, qualification)
            }
        };
        if receiver.is_unknown() && qualification.is_none() {
            return Ok(Vec::new());
        }

        // A prefix such as `Container<_>` can leave holes for the item's use to fill.
        let receiver = table.instantiate_nested_unknowns(receiver);
        Ok(self
            .function_candidates(
                scope,
                receiver,
                FunctionLookup::Associated {
                    name,
                    qualification,
                },
                table,
            )?
            .into_iter()
            .map(LiveCallTarget::from_member)
            .collect())
    }

    pub(crate) fn declared_function_targets<'s>(
        &self,
        resolution: &BodyResolution,
    ) -> Result<Vec<LiveCallTarget<'s>>, ReadError> {
        // An ordinary function path, or an associated path with no target above, can use
        // its resolved declarations. There is no adjusted receiver or receiver trial here.
        let mut targets = Vec::new();
        if let BodyResolution::Declarations(declarations) = resolution {
            for declaration in declarations {
                if let Some(function) = self.declaration_function(*declaration)? {
                    targets.push(LiveCallTarget {
                        function,
                        subst: InferenceSubstitution::new(),
                        receiver: None,
                        table: None,
                        can_infer: true,
                    });
                }
            }
        }
        Ok(targets)
    }

    /// Keep only declarations that name functions.
    fn declaration_function(
        &self,
        declaration: DeclarationRef,
    ) -> Result<Option<FunctionRef>, ReadError> {
        match declaration {
            DeclarationRef::LocalDef(local_def) => Ok(
                match self
                    .context
                    .item_query()
                    .semantic_item_for_local_def(local_def)?
                {
                    Some(SemanticItemRef::Function(function)) => Some(function),
                    Some(_) | None => None,
                },
            ),
            DeclarationRef::Item(SemanticItemRef::Function(function_ref)) => Ok(Some(function_ref)),
            DeclarationRef::Module(_)
            | DeclarationRef::Item(
                SemanticItemRef::TypeDef(_)
                | SemanticItemRef::Trait(_)
                | SemanticItemRef::Impl(_)
                | SemanticItemRef::TypeAlias(_)
                | SemanticItemRef::Const(_)
                | SemanticItemRef::Static(_),
            )
            | DeclarationRef::Field(_)
            | DeclarationRef::EnumVariant(_)
            | DeclarationRef::BodyBinding(_) => Ok(None),
        }
    }
}
