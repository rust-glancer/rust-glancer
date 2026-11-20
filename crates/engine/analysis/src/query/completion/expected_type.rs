//! Finds the expected type and uses it to rank value completions.
//!
//! At `send(us$0)`, resolve the parameter type once for the site. Then compare it with
//! the types produced by visible candidates. For enums, the expected type can also
//! supply variants that an unqualified name lookup would miss.

use rg_ir_model::{FunctionRef, PrimitiveTy, TypeDefId, TypeDefRef, identity::DeclarationRef};
use rg_ir_view::{
    member::{ConstructorShape, MemberView},
    source::{IndexedQualifiedPathScope, IndexedUnqualifiedNameScope},
    ty::{IndexedType, TyView},
};

use super::{
    CompletionContext,
    render::CompletionSortPolicy,
    site::{CompletionSite, NameCompletionContext},
    syntax::{CompletionSyntaxContext, ExpectedTypeSite, ValueExpression},
};
use crate::{
    Analysis,
    model::{
        CompletionApplicability, CompletionItem, CompletionKind, CompletionTarget,
        KeywordCompletion,
    },
    query::active_call::{ActiveArgument, ActiveCall},
};

/// The expected type and the expression syntax we use to interpret each candidate.
/// For an enum variant, the qualifier can also fix the enum's generic arguments.
pub(super) struct ExpectedTypeRanking {
    expected: IndexedType,
    expression: ValueExpression,
    member_has_generic_args: bool,
    qualifier: Option<IndexedType>,
}

impl ExpectedTypeRanking {
    pub(super) fn at(
        analysis: &Analysis<'_>,
        query: CompletionContext<'_>,
        syntax: Option<&CompletionSyntaxContext<'_>>,
        site: &CompletionSite,
    ) -> anyhow::Result<Option<Self>> {
        let Some(context) = syntax.and_then(CompletionSyntaxContext::expected_type_context) else {
            return Ok(None);
        };
        // In `let _: User = model::Us$0 { id: 1 }`, the name is looked up in the
        // type namespace. The record expression as a whole supplies a value.
        let is_value_site = |site_context| {
            site_context == NameCompletionContext::Value
                || (site_context == NameCompletionContext::Type
                    && matches!(context.expression, ValueExpression::Record))
        };
        let (scope, qualifier) = match site {
            CompletionSite::Unqualified(site) if is_value_site(site.context()) => {
                let IndexedUnqualifiedNameScope::Body { scope, .. } = site.source().scope() else {
                    return Ok(None);
                };
                (*scope, None)
            }
            CompletionSite::Path(site) if is_value_site(site.context()) => {
                let IndexedQualifiedPathScope::Body { scope, .. } = site.source().scope() else {
                    return Ok(None);
                };
                (scope, site.source().associated_qualifier())
            }
            _ => return Ok(None),
        };

        let types = TyView::new(analysis.view_db());
        let body = scope.body_ir();
        let expected = match context.site {
            ExpectedTypeSite::Argument { call_start } => {
                // The marker parse and the original source both need to point to the
                // same call. Recovery in either tree can change which list owns the cursor.
                let Some(active) =
                    ActiveCall::at(analysis, query.crate_ref, query.file_id, query.offset)?
                        .filter(|active| active.span().start == call_start)
                else {
                    return Ok(None);
                };
                let argument = match active.argument() {
                    ActiveArgument::Empty => None,
                    ActiveArgument::Written(span) => Some(span),
                    ActiveArgument::Disabled => return Ok(None),
                };
                types.call_parameter_ty(active.call(), active.argument_index(), argument)?
            }
            ExpectedTypeSite::Let { statement_start } => {
                types.let_annotation_ty(body, query.file_id, statement_start)?
            }
            ExpectedTypeSite::Assignment { target } => {
                types.written_expr_ty(body, query.file_id, target)?
            }
            ExpectedTypeSite::FunctionReturn => types.function_body_output(body)?,
        };
        // Unit is common in block tails and tells us little about which item to offer first.
        let Some(expected) = expected.filter(|ty| ty.is_known() && !ty.is_unit()) else {
            return Ok(None);
        };

        let qualifier = match qualifier {
            Some(qualifier) => types.nominal_qualifier_ty(scope, qualifier)?,
            None => None,
        };
        Ok(Some(Self {
            expected,
            expression: context.expression,
            member_has_generic_args: context.member_has_generic_args,
            qualifier,
        }))
    }

    pub(super) fn is_call(&self) -> bool {
        matches!(self.expression, ValueExpression::Call)
    }

    /// The renderer knows if accepting the candidate inserts an argument list.
    /// The syntax context tells us if an argument list is already there.
    pub(super) fn rank_function(
        &self,
        types: TyView<'_, '_>,
        function: FunctionRef,
        inserts_call: bool,
        item: &mut CompletionItem,
    ) -> anyhow::Result<()> {
        let produces_call = match self.expression {
            ValueExpression::Call => true,
            ValueExpression::Path => inserts_call,
            ValueExpression::Record => false,
        };
        if produces_call
            && item.applicability == CompletionApplicability::Known
            && types.function_call_ty(function)?.as_ref() == Some(&self.expected)
        {
            CompletionSortPolicy::promote_expected_type(&mut item.sort_text);
        }
        Ok(())
    }

    /// At a bare name, the completion can supply the entire constructor.
    /// If the user has already supplied delimiters, keep only their matching variant shape.
    pub(super) fn enum_constructor_site(&self) -> Option<(TypeDefRef, ValueExpression)> {
        let owner = self.expected.adt_def()?;
        (!self.member_has_generic_args && matches!(owner.id, TypeDefId::Enum(_)))
            .then_some((owner, self.expression))
    }

    /// Rank values and constructors after visibility and shadowing checks. Function
    /// completions have already been ranked when their insertion text was chosen.
    pub(super) fn rank(
        &self,
        analysis: &Analysis<'_>,
        items: &mut [CompletionItem],
    ) -> anyhow::Result<()> {
        let db = analysis.view_db();
        let types = TyView::new(db);
        let members = MemberView::new(db);
        let mut changed = false;
        for item in items.iter_mut() {
            rg_std::check_cancel!(analysis, "rank completion candidate type");
            if item.import.is_some() || item.applicability != CompletionApplicability::Known {
                continue;
            }
            let target = match item.target {
                CompletionTarget::EnumVariant(variant) => {
                    CompletionTarget::Declaration(variant.into())
                }
                target => target,
            };
            let fits = match (target, item.kind) {
                (
                    CompletionTarget::Declaration(declaration),
                    CompletionKind::Variable | CompletionKind::Const | CompletionKind::Static,
                ) if matches!(self.expression, ValueExpression::Path) => {
                    types.value_ty(declaration)?.as_ref() == Some(&self.expected)
                }
                (
                    CompletionTarget::Keyword(KeywordCompletion::False | KeywordCompletion::True),
                    _,
                ) if matches!(self.expression, ValueExpression::Path) => {
                    self.expected.primitive() == Some(PrimitiveTy::Bool)
                }
                (
                    CompletionTarget::Declaration(declaration),
                    CompletionKind::Struct | CompletionKind::EnumVariant,
                ) => {
                    // TODO: Substitute generic arguments written on the constructor name.
                    if self.member_has_generic_args {
                        continue;
                    }
                    let Some(ty) = types.ty_for_declaration(declaration)? else {
                        continue;
                    };
                    let qualifier = match declaration {
                        DeclarationRef::EnumVariant(_) => self.qualifier.as_ref(),
                        _ => None,
                    };
                    if !self.expected.matches_constructor(&ty, qualifier) {
                        continue;
                    }

                    // Bare paths produce values for unit constructors. Tuple and
                    // record constructors need the corresponding syntax at the site.
                    let shape = members.constructor_shape_for_declaration(declaration)?;
                    shape.is_some_and(|shape| self.expression.matches_constructor(&shape))
                }
                _ => false,
            };

            if fits {
                CompletionSortPolicy::promote_expected_type(&mut item.sort_text);
                changed = true;
            }
        }
        if changed {
            items.sort_by(|left, right| left.sort_text.cmp(&right.sort_text));
        }
        Ok(())
    }
}

impl ValueExpression {
    pub(super) fn matches_constructor(self, shape: &ConstructorShape) -> bool {
        matches!(
            (self, shape),
            (Self::Path, ConstructorShape::Unit)
                | (Self::Call, ConstructorShape::Tuple { .. })
                | (Self::Record, ConstructorShape::Record { .. })
        )
    }
}
