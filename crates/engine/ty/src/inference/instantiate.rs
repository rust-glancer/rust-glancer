use super::{table::InferenceTable, traversal::InferenceTyFolder};
use crate::Ty;

struct UnknownTypeInstantiation<'table> {
    table: &'table mut InferenceTable,
}

impl InferenceTyFolder for UnknownTypeInstantiation<'_> {
    fn fold_unknown(&mut self) -> Ty {
        self.table.new_type_var()
    }
}

impl InferenceTable {
    /// Give unknown children of a known shape their own live slots: `Vec<unknown>` becomes
    /// `Vec<?T>`. A root `Unknown` has no producer shape, so it stays unknown.
    pub fn instantiate_nested_unknowns(&mut self, ty: &Ty) -> Ty {
        if matches!(ty, Ty::Unknown) {
            return Ty::Unknown;
        }
        UnknownTypeInstantiation { table: self }.fold_ty(ty)
    }

    /// Replacing only projections preserves useful outer shapes such as `Vec<Iterator::Item>` while
    /// the associated value is pending. Each returned pair is one normalization request and its
    /// live destination; the caller owns retry and finalization policy.
    ///
    /// For example, `Vec<S::Item>` becomes `Vec<?Item>` with the pair `(S::Item, ?Item)`. Retain
    /// that pair across retries so consumers of the vector keep sharing the same destination.
    pub fn instantiate_projections(&mut self, ty: &Ty) -> (Ty, Vec<(crate::ProjectionTy, Ty)>) {
        let mut builder = ProjectionInstantiationBuilder {
            table: self,
            projections: Vec::new(),
        };
        let ty = builder.fold_ty(ty);
        (ty, builder.projections)
    }
}

struct ProjectionInstantiationBuilder<'table> {
    table: &'table mut InferenceTable,
    projections: Vec<(crate::ProjectionTy, Ty)>,
}

impl InferenceTyFolder for ProjectionInstantiationBuilder<'_> {
    fn fold_projection(&mut self, projection: &crate::ProjectionTy) -> Ty {
        let slot = self.table.new_type_var();
        self.projections.push((projection.clone(), slot.clone()));
        slot
    }
}
