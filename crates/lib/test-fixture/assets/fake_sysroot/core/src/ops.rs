#[lang = "deref"]
pub trait Deref {
    #[lang = "deref_target"]
    type Target: ?crate::marker::Sized;
}
