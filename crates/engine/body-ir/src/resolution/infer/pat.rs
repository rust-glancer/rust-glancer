//! Pass the type of a matched value through patterns into their bindings.
//!
//! For `let (item, ready) = pair`, a pair type `(?T, bool)` gives `item` the same `?T` slot.
//! Evidence from a later use of `item` can then reach `pair`. If the outer shape is not known yet,
//! defer just that pattern projection; expressions owned by the pattern are still visited once.

use anyhow::Context as _;
use rg_def_map::DefMapSource;
use rg_ir_model::{FieldKey, Mutability, PatId};
use rg_item_tree::SelfParamKind;
use rg_package_store::PackageStoreError;
use rg_semantic_ir::ItemStoreSource;
use rg_ty::{ExpectedAdtTyExt, Ty};

use crate::body::{BindingKind, BodyPath, PatKind, RecordPatField};

use super::{InferenceContext, fulfill::DeferredKind};

impl<'query, D, I> InferenceContext<'query, D, I>
where
    D: DefMapSource<Error = PackageStoreError> + Copy,
    I: ItemStoreSource<'query, Error = PackageStoreError> + Copy,
{
    /// Seed bindings from the function signature before any body expression can read them.
    /// Destructured parameters then project that signature type into their individual bindings.
    pub(super) fn infer_parameters(&mut self) -> anyhow::Result<()> {
        let signature = self
            .body
            .owner()
            .function()
            .map(|function| self.context.signatures().function(function))
            .transpose()
            .context("resolve function signature")?
            .flatten();
        // A missing arrow has no body expectation, even though the semantic signature uses unit.
        if let Some(function) = self.body.owner().function()
            && self
                .context
                .item_query()
                .function_data(function)
                .context("read function return annotation")?
                .is_some_and(|data| data.signature.ret_ty().is_some())
        {
            self.return_ty = signature
                .as_ref()
                .map(|signature| signature.ret.clone())
                .unwrap_or(Ty::Unknown);
        }
        for (index, param) in self.body.function_params().iter().enumerate() {
            let parameter_ty = signature
                .as_ref()
                .and_then(|signature| signature.params.get(index));
            // `self` and incomplete ordinary parameters can have bindings without a pattern.
            // Seed them here, using the position already known from this declaration walk.
            for binding in &param.bindings {
                rg_std::check_cancel!(self.context, "infer parameter binding");
                let data = self.body.binding_unchecked(*binding);
                let ty = if param.bindings.len() == 1
                    && let Some(ty) = parameter_ty.filter(|ty| !matches!(ty, Ty::Unknown))
                {
                    ty.clone()
                } else if let Some(annotation) = &data.annotation {
                    self.context
                        .type_refs(data.scope)
                        .resolve(annotation)
                        .context("resolve parameter binding annotation")?
                } else if let BindingKind::SelfParam(kind) = data.kind
                    && data.name.as_deref() == Some("self")
                    && let Some(function) = self.body.owner().function()
                {
                    let ty = self
                        .context
                        .functions()
                        .self_adt_ty(function)
                        .context("resolve self parameter")?
                        .into_adt_ty();
                    match kind {
                        SelfParamKind::Value => ty,
                        SelfParamKind::Reference { mutability } => Ty::reference(mutability, ty),
                        SelfParamKind::Explicit => Ty::Unknown,
                    }
                } else {
                    Ty::Unknown
                };
                self.inference.set_binding_ty(*binding, &ty);
            }
            let Some(pat) = param.pat else { continue };
            let ty = match parameter_ty {
                Some(ty) => ty.clone(),
                None => match &param.annotation {
                    Some(annotation) => self
                        .context
                        .type_refs(self.body.param_scope())
                        .resolve(annotation)
                        .context("resolve parameter annotation")?,
                    None => Ty::Unknown,
                },
            };
            self.infer_pattern(pat, &ty)
                .context("infer parameter pattern")?;
        }
        Ok(())
    }

    /// Patterns can own expressions even when no type shape is available, as in `const { KEY }`.
    /// Visit those expressions at the introduction site, independently of deferred projection.
    pub(super) fn infer_pattern(&mut self, pat: PatId, expected: &Ty) -> anyhow::Result<()> {
        self.infer_pattern_exprs(pat)
            .context("infer pattern expressions")?;
        self.infer_pat(pat, expected).context("infer pattern type")
    }

    fn infer_pattern_exprs(&mut self, pat: PatId) -> anyhow::Result<()> {
        rg_std::check_cancel!(self.context, "infer pattern expressions");
        let body = self.body;
        let Some(data) = body.pat(pat) else {
            return Ok(());
        };
        if let PatKind::ConstBlock { expr: Some(expr) } = data.kind {
            self.infer_expr(expr, &Ty::Unknown)
                .context("infer const pattern")?;
        }
        for child in data.kind.child_pats() {
            self.infer_pattern_exprs(child)
                .context("infer nested pattern expression")?;
        }
        Ok(())
    }

    /// Project the available type and queue this node if its structure still needs more evidence.
    /// Recursive pattern work enters here without revisiting pattern-owned expressions.
    pub(super) fn infer_pat(
        &mut self,
        pat: PatId,
        expected_ty: &Ty,
    ) -> Result<(), PackageStoreError> {
        if !self.try_infer_pat(pat, expected_ty)? {
            self.defer(
                DeferredKind::Pattern {
                    pat,
                    expected: expected_ty.clone(),
                },
                None,
            );
        }
        Ok(())
    }

    /// Link this node's bindings and children to the expected type. Return false when we need its
    /// outer shape first: a simple binding can share `?T` immediately, but `(left, right)` needs
    /// a tuple before we can give each child its field type. Children may queue their own work.
    pub(super) fn try_infer_pat(
        &mut self,
        pat: PatId,
        expected_ty: &Ty,
    ) -> Result<bool, PackageStoreError> {
        crate::profile::metric::PATTERN_VISITS.inc();
        let expected_ty = self.inference.root_resolved_ty(expected_ty);
        if matches!(expected_ty, Ty::Unknown) {
            // A raw unknown has no link to future evidence, so retrying it cannot reveal a shape.
            return Ok(true);
        }

        let body = self.body;
        let Some(data) = body.pat(pat) else {
            return Ok(true);
        };

        if matches!(expected_ty, Ty::InferVar { .. } | Ty::Alias(_))
            && matches!(
                data.kind,
                PatKind::TupleStruct { .. }
                    | PatKind::Record { .. }
                    | PatKind::Tuple { .. }
                    | PatKind::Slice { .. }
                    | PatKind::Ref { .. }
            )
        {
            return Ok(false);
        }
        match data.kind {
            PatKind::Binding {
                binding, subpat, ..
            } => {
                if let Some(binding) = binding {
                    self.inference
                        .set_binding_infer_ty(binding, expected_ty.clone());
                }
                if let Some(subpat) = subpat {
                    self.infer_pat(subpat, &expected_ty)?;
                }
                Ok(())
            }
            PatKind::TupleStruct {
                ref path,
                ref fields,
            } => self.link_tuple_variant(path.as_ref(), fields, &expected_ty),
            PatKind::Record {
                ref path,
                ref fields,
                ..
            } => self.link_record_pat(path.as_ref(), fields, &expected_ty),
            PatKind::Tuple { ref fields } => self.link_tuple_pat(fields, &expected_ty),
            PatKind::Slice { ref fields } => self.link_slice_pat(fields, &expected_ty),
            PatKind::Or { ref pats } => {
                for pat in pats {
                    self.infer_pat(*pat, &expected_ty)?;
                }
                Ok(())
            }
            PatKind::Ref { mutability, pat } => self.link_ref_pat(pat, mutability, &expected_ty),
            PatKind::Box { pat } => self.infer_pat(pat, &expected_ty),
            PatKind::Path { .. }
            | PatKind::Rest
            | PatKind::Literal { .. }
            | PatKind::Range { .. }
            | PatKind::ConstBlock { .. }
            | PatKind::Wildcard
            | PatKind::Unsupported => Ok(()),
        }?;
        Ok(true)
    }

    /// Project tuple fields by position, e.g. `(left, right): (User, bool)`.
    fn link_tuple_pat(
        &mut self,
        fields: &[PatId],
        expected_ty: &Ty,
    ) -> Result<(), PackageStoreError> {
        let Ty::Tuple(field_tys) = expected_ty else {
            return Ok(());
        };
        if fields.len() != field_tys.len() {
            return Ok(());
        }

        for (field_pat, field_ty) in fields.iter().zip(field_tys) {
            self.infer_pat(*field_pat, field_ty)?;
        }
        Ok(())
    }

    /// Give every non-rest slice pattern the container's element type.
    fn link_slice_pat(
        &mut self,
        fields: &[PatId],
        expected_ty: &Ty,
    ) -> Result<(), PackageStoreError> {
        let element_ty = match expected_ty {
            Ty::Array { inner, .. } | Ty::Slice(inner) => inner.as_ref(),
            _ => return Ok(()),
        };

        for field in fields {
            if self
                .body
                .pat(*field)
                .is_some_and(|pat| matches!(&pat.kind, PatKind::Rest))
            {
                continue;
            }
            self.infer_pat(*field, element_ty)?;
        }
        Ok(())
    }

    /// Peel only the reference written by the pattern, preserving its mutability contract.
    fn link_ref_pat(
        &mut self,
        pat: PatId,
        pat_mutability: Mutability,
        expected_ty: &Ty,
    ) -> Result<(), PackageStoreError> {
        let Some((inner_ty, mutability)) = expected_ty.reference_inner() else {
            return Ok(());
        };
        if mutability != pat_mutability {
            return Ok(());
        }

        self.infer_pat(pat, inner_ty)
    }

    /// Project tuple-variant payload fields from the expected enum instantiation.
    fn link_tuple_variant(
        &mut self,
        path: Option<&BodyPath>,
        fields: &[PatId],
        expected_ty: &Ty,
    ) -> Result<(), PackageStoreError> {
        for (index, field_pat) in fields.iter().enumerate() {
            let field_key = FieldKey::Tuple(index);
            if let Some(field_ty) =
                self.context
                    .fields()
                    .pattern_field_ty(path, expected_ty, &field_key)?
            {
                self.infer_pat(*field_pat, &field_ty)?;
            }
        }
        Ok(())
    }

    /// Project named pattern fields from structs, unions, or record enum variants.
    fn link_record_pat(
        &mut self,
        path: Option<&BodyPath>,
        fields: &[RecordPatField],
        expected_ty: &Ty,
    ) -> Result<(), PackageStoreError> {
        for field in fields {
            if let Some(field_ty) =
                self.context
                    .fields()
                    .pattern_field_ty(path, expected_ty, &field.key)?
            {
                self.infer_pat(field.pat, &field_ty)?;
            }
        }
        Ok(())
    }
}
