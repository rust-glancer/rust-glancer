use rg_ir_model::items::TypeRef;

use super::super::{table::InferenceTable, traversal::TypeRefInferenceProjector};
use crate::Ty;

/// Instantiates explicit `_` type args as variables.
///
/// ```text
/// make::<_>()         // resolved arg: <unknown>
///                     // inference arg: ?T
///
/// make::<Vec<_>>()    // resolved arg: Vec<unknown>
///                     // inference arg: Vec<?T>
/// ```
pub struct ExplicitTypeArgInstantiationBuilder<'table> {
    table: &'table mut InferenceTable,
    used_type_vars: bool,
}

impl<'table> ExplicitTypeArgInstantiationBuilder<'table> {
    pub fn new(table: &'table mut InferenceTable) -> Self {
        Self {
            table,
            used_type_vars: false,
        }
    }

    pub fn used_type_vars(&self) -> bool {
        self.used_type_vars
    }

    /// Convert one explicit type arg into an inference-aware type.
    pub fn ty_from_arg(&mut self, arg_ty: &TypeRef, resolved_ty: &Ty) -> Ty {
        self.project_ty(arg_ty, resolved_ty)
    }
}

impl TypeRefInferenceProjector for ExplicitTypeArgInstantiationBuilder<'_> {
    /// Instantiate written `_` slots in explicit args such as `make::<Vec<_>>()`.
    fn replace_written_ty(&mut self, written_ty: &TypeRef) -> Option<Ty> {
        if matches!(written_ty, TypeRef::Infer) {
            self.used_type_vars = true;
            return Some(self.table.new_type_var());
        }

        None
    }
}
