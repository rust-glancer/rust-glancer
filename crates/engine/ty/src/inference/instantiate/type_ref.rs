use rg_ir_model::items::{GenericParams, TypeRef};
use rg_text::Name;

use super::super::{family::TypeRefInferenceProjector, model::InferTy, table::InferenceTable};
use crate::Ty;

/// Instantiates function type params as variables inside a projected call return.
///
/// ```text
/// fn id<T>(value: T) -> T
/// id(missing())       // resolved return: <unknown>, declared return: T
///                     // inference return: ?T
///
/// fn make_vec<T>() -> Vec<T>
/// make_vec()          // resolved return: Vec<unknown>, declared return: Vec<T>
///                     // inference return: Vec<?T>
/// ```
pub struct GenericReturnInstantiationBuilder<'table> {
    table: &'table mut InferenceTable,
    params: Vec<(Name, Option<InferTy>)>,
    used_type_vars: bool,
}

impl<'table> GenericReturnInstantiationBuilder<'table> {
    pub fn new(table: &'table mut InferenceTable, generics: &GenericParams) -> Self {
        Self {
            table,
            params: generics
                .types
                .iter()
                .map(|param| (param.name.clone(), None))
                .collect(),
            used_type_vars: false,
        }
    }

    pub fn used_type_vars(&self) -> bool {
        self.used_type_vars
    }

    pub fn ty_from_return(&mut self, ret_ty: &TypeRef, resolved_ty: &Ty) -> InferTy {
        self.project_ty(ret_ty, resolved_ty)
    }

    fn var_for_plain_type_param(&mut self, ret_ty: &TypeRef) -> Option<InferTy> {
        let name = ret_ty.type_param_name()?;
        let idx = self
            .params
            .iter()
            .position(|(param, _)| param.as_str() == name.as_str())?;

        if self.params[idx].1.is_none() {
            self.params[idx].1 = Some(self.table.new_type_var());
        }
        self.used_type_vars = true;
        self.params[idx].1.clone()
    }
}

impl TypeRefInferenceProjector for GenericReturnInstantiationBuilder<'_> {
    /// Instantiate return type params such as `T` in `fn make<T>() -> T`.
    fn replace_written_ty(&mut self, written_ty: &TypeRef) -> Option<InferTy> {
        self.var_for_plain_type_param(written_ty)
    }
}

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
    pub fn ty_from_arg(&mut self, arg_ty: &TypeRef, resolved_ty: &Ty) -> InferTy {
        self.project_ty(arg_ty, resolved_ty)
    }
}

impl TypeRefInferenceProjector for ExplicitTypeArgInstantiationBuilder<'_> {
    /// Instantiate written `_` slots in explicit args such as `make::<Vec<_>>()`.
    fn replace_written_ty(&mut self, written_ty: &TypeRef) -> Option<InferTy> {
        if matches!(written_ty, TypeRef::Infer) {
            self.used_type_vars = true;
            return Some(self.table.new_type_var());
        }

        None
    }
}
