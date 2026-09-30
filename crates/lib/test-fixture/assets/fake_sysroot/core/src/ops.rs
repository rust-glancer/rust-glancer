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

#[lang = "Range"]
pub struct Range<Idx> {
    pub start: Idx,
    pub end: Idx,
}

#[lang = "RangeFrom"]
pub struct RangeFrom<Idx> {
    pub start: Idx,
}

#[lang = "RangeTo"]
pub struct RangeTo<Idx> {
    pub end: Idx,
}

#[lang = "RangeInclusive"]
pub struct RangeInclusive<Idx> {
    // The endpoints are private in core. Iterator state is unnecessary for type inference.
    start: Idx,
    end: Idx,
}

#[lang = "RangeToInclusive"]
pub struct RangeToInclusive<Idx> {
    pub end: Idx,
}

#[lang = "RangeFull"]
pub struct RangeFull;
