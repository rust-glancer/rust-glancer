use rg_ir_model::hir::items::ImplData;

pub(super) struct SupportedImplHeader;

impl SupportedImplHeader {
    /// Keep the first slice on direct impl headers. More complex headers need recursive goals.
    pub(super) fn accepts(impl_data: &ImplData) -> bool {
        impl_data.generics.where_predicates.is_empty()
            && impl_data
                .generics
                .lifetimes
                .iter()
                .all(|param| param.bounds.is_empty())
            && impl_data
                .generics
                .types
                .iter()
                .all(|param| param.bounds.is_empty() && param.default.is_none())
            && impl_data.generics.consts.is_empty()
    }
}
