#[lang = "deref"]
pub trait Deref {
    #[lang = "deref_target"]
    type Target: ?crate::marker::Sized;
}

// These fixtures exercise output projections. Residual conversion and branching methods are
// intentionally omitted because they are not needed to declare those relationships.
#[lang = "Try"]
pub trait Try {
    type Output;
}
