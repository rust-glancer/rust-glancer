//! Defines [`SyntaxKind`] -- a fieldless enum of all possible syntactic
//! constructs of the Rust language.

#[rustfmt::skip]
#[cfg_attr(
    dylint_lib = "rust_glancer_lints",
    allow(
        rust_glancer_impl_helpers,
        rust_glancer_implicit_local_imports,
        rust_glancer_non_adjacent_impls,
        rust_glancer_pub_in
    )
)]
mod generated;

#[allow(unreachable_pub)]
pub use self::generated::SyntaxKind;
use crate::Edition;

impl From<u16> for SyntaxKind {
    #[inline]
    fn from(d: u16) -> SyntaxKind {
        assert!(d <= (SyntaxKind::__LAST as u16));
        unsafe { std::mem::transmute::<u16, SyntaxKind>(d) }
    }
}

impl From<SyntaxKind> for u16 {
    #[inline]
    fn from(k: SyntaxKind) -> u16 {
        k as u16
    }
}

impl SyntaxKind {
    #[inline]
    pub fn is_trivia(self) -> bool {
        matches!(self, SyntaxKind::WHITESPACE | SyntaxKind::COMMENT)
    }

    /// Returns true if this is an identifier or a keyword.
    #[inline]
    pub fn is_any_identifier(self) -> bool {
        // Assuming no edition removed keywords...
        self == SyntaxKind::IDENT || self.is_keyword(Edition::LATEST)
    }
}
