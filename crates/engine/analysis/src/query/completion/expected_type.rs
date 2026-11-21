//! Finds the expected type and uses it to rank value completions.
//!
//! At `send(us$0)`, resolve the parameter type once for the site. Then compare it with
//! the types produced by visible candidates. For enums, the expected type can also
//! supply variants that an unqualified name lookup would miss.

use rg_ir_model::{
    PrimitiveTy, TypeDefId, TypeDefRef,
    identity::{DeclarationRef, LexicalScopeRef},
};
use rg_ir_view::{
    member::{ConstructorShape, MemberView},
    source::{IndexedQualifiedPathScope, IndexedUnqualifiedNameScope},
    ty::{ExpectedCallResult, IndexedCallArguments, IndexedType, TyView, locals::CallTarget},
};

use super::{
    CompletionContext,
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
    scope: LexicalScopeRef,
    parameter_name: Option<String>,
    expression: ValueExpression,
    member_args: Option<IndexedCallArguments>,
    enum_qualifier: Option<IndexedType>,
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
            CompletionSite::Dot(site) => (site.source().scope(), None),
            _ => return Ok(None),
        };

        let types = TyView::new(analysis.view_db());
        let body = scope.body_ir();
        let mut parameter_name = None;
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
                let call = active.call();
                let expected = types.call_parameter_ty(call, active.argument_index(), argument)?;
                if expected.as_ref().is_some_and(IndexedType::is_known)
                    && let CallTarget::Function(function) = call.target()
                {
                    // Dot calls omit `self` from the written arguments.
                    parameter_name = MemberView::new(analysis.view_db())
                        .function(*function)?
                        .and_then(|function| {
                            function.parameter(active.argument_index() + call.param_offset())
                        })
                        .and_then(|parameter| parameter.simple_binding_name())
                        .map(str::to_owned);
                }
                expected
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

        let enum_qualifier = match (qualifier, expected.adt_def()) {
            (Some(qualifier), Some(owner)) if matches!(owner.id, TypeDefId::Enum(_)) => {
                types.nominal_qualifier_ty(scope, qualifier)?
            }
            _ => None,
        };
        Ok(Some(Self {
            expected,
            scope,
            parameter_name,
            expression: context.expression,
            member_args: context.member_args,
            enum_qualifier,
        }))
    }

    pub(super) fn field_result_type(&self) -> Option<&IndexedType> {
        (matches!(self.expression, ValueExpression::Path) && self.member_args.is_none())
            .then_some(&self.expected)
    }

    /// The expected value type, when the syntax and insertion policy produce a call.
    pub(super) fn call_result_type(&self) -> Option<ExpectedCallResult<'_>> {
        (!matches!(self.expression, ValueExpression::Record)).then_some(ExpectedCallResult {
            ty: &self.expected,
            explicit_args: self.member_args.as_ref(),
        })
    }

    /// At a bare name, the completion can supply the entire constructor.
    /// If the user has already supplied delimiters, keep only their matching variant shape.
    pub(super) fn enum_constructor_site(
        &self,
    ) -> Option<(&IndexedType, TypeDefRef, ValueExpression)> {
        let owner = self.expected.adt_def()?;
        (self.member_args.is_none() && matches!(owner.id, TypeDefId::Enum(_))).then_some((
            &self.expected,
            owner,
            self.expression,
        ))
    }

    /// Compare free function returns together in the caller's scope. Both plain and
    /// explicit calls may return an associated type that needs to be normalized.
    pub(super) fn rank_free_functions(
        &self,
        types: TyView<'_, '_>,
        items: &mut [CompletionItem],
    ) -> anyhow::Result<()> {
        let Some(expected) = self.call_result_type() else {
            return Ok(());
        };
        let functions = items.iter().filter_map(|item| {
            let CompletionTarget::Function(function) = item.target else {
                return None;
            };
            (item.import.is_none() && item.applicability == CompletionApplicability::Known)
                .then_some(function)
        });
        let matches = types.free_functions_with_matching_return(self.scope, functions, expected)?;
        for item in items {
            if let CompletionTarget::Function(function) = item.target
                && matches.contains(&function)
                && item.import.is_none()
                && item.applicability == CompletionApplicability::Known
            {
                self.promote_match(item);
            }
        }
        Ok(())
    }

    /// Rank values and constructors after visibility and shadowing checks.
    pub(super) fn rank(
        &self,
        analysis: &Analysis<'_>,
        items: &mut [CompletionItem],
    ) -> anyhow::Result<()> {
        let db = analysis.view_db();
        let types = TyView::new(db);
        let members = MemberView::new(db);
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
                    if self.member_args.is_some() {
                        continue;
                    }
                    let Some(ty) = types.ty_for_declaration(declaration)? else {
                        continue;
                    };
                    let qualifier = match declaration {
                        DeclarationRef::EnumVariant(_) => self.enum_qualifier.as_ref(),
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
                self.promote_match(item);
            }
        }
        Ok(())
    }

    /// Apply a match from member lookup if the completed function will be called.
    pub(super) fn promote_call_match(&self, item: &mut CompletionItem) {
        if item.applicability == CompletionApplicability::Known {
            self.promote_match(item);
        }
    }

    pub(super) fn promote_match(&self, item: &mut CompletionItem) {
        item.relevance.set_inference_priority(true);
        let label = item.label.strip_prefix("r#").unwrap_or(&item.label);
        item.relevance.set_parameter_name_priority(
            self.parameter_name
                .as_deref()
                .is_some_and(|name| name.strip_prefix("r#").unwrap_or(name) == label),
        );
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
