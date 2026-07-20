use super::{table::InferenceTable, traversal::InferenceTyFolder};
use crate::Ty;

/// Instantiates unknowns nested inside a known return shape.
///
/// ```text
/// impl<T> Vec<T> { fn new() -> Self }
/// Vec::new()          // selected Self: Vec<unknown>, resolved return: Vec<unknown>
///                     // inference return: Vec<?T>
///
/// missing()           // resolved return: <unknown>
///                     // inference return: <unknown>
/// ```
pub struct UnknownTypeInstantiationBuilder<'table> {
    table: &'table mut InferenceTable,
    used_type_vars: bool,
}

impl<'table> UnknownTypeInstantiationBuilder<'table> {
    pub fn new(table: &'table mut InferenceTable) -> Self {
        Self {
            table,
            used_type_vars: false,
        }
    }

    pub fn used_type_vars(&self) -> bool {
        self.used_type_vars
    }

    pub fn ty_from_ty(&mut self, ty: &Ty) -> Ty {
        // We don't instantiate root unknown.
        if matches!(ty, Ty::Unknown) {
            return Ty::Unknown;
        }

        // For whatever unknowns exist inside of `Ty`, replace them with `?T`.
        self.fold_ty(ty)
    }
}

impl InferenceTyFolder for UnknownTypeInstantiationBuilder<'_> {
    fn fold_unknown(&mut self) -> Ty {
        // Within a known `Ty` shape, replace each `Ty::Unknown` with a new infer variable.
        self.used_type_vars = true;
        self.table.new_type_var()
    }
}
