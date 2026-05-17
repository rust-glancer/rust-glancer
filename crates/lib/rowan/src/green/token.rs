use std::{borrow::Borrow, cell::Cell, fmt, hash, ops, ptr, sync::Arc};

use crate::{green::GreenArena, SyntaxKind, TextSize};

use super::GreenNodeData;

/// Leaf data stored inside a green arena.
pub struct GreenTokenData {
    pub(crate) arena: ptr::NonNull<GreenArena>,
    kind: SyntaxKind,
    text: &'static str,
    parent: Cell<Option<ptr::NonNull<GreenNodeData>>>,
    index: Cell<u32>,
    rel_offset: Cell<TextSize>,
}

impl PartialEq for GreenTokenData {
    fn eq(&self, other: &Self) -> bool {
        self.kind() == other.kind() && self.text() == other.text()
    }
}

impl Eq for GreenTokenData {}

impl hash::Hash for GreenTokenData {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.kind().hash(state);
        self.text().hash(state);
    }
}

/// Leaf node in the immutable tree.
#[repr(transparent)]
pub struct GreenToken {
    repr: GreenTokenRepr,
}

struct GreenTokenRepr {
    ptr: ptr::NonNull<GreenTokenData>,
    arena: Arc<GreenArena>,
}

unsafe impl Send for GreenToken {}
unsafe impl Sync for GreenToken {}

impl Clone for GreenToken {
    #[inline]
    fn clone(&self) -> Self {
        GreenToken {
            repr: GreenTokenRepr {
                ptr: self.repr.ptr,
                arena: self.repr.arena.clone(),
            },
        }
    }
}

impl PartialEq for GreenToken {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl Eq for GreenToken {}

impl hash::Hash for GreenToken {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        (**self).hash(state)
    }
}

impl ToOwned for GreenTokenData {
    type Owned = GreenToken;

    #[inline]
    fn to_owned(&self) -> GreenToken {
        GreenToken::new_owned(self)
    }
}

impl Borrow<GreenTokenData> for GreenToken {
    #[inline]
    fn borrow(&self) -> &GreenTokenData {
        self
    }
}

impl fmt::Debug for GreenTokenData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GreenToken")
            .field("kind", &self.kind())
            .field("text", &self.text())
            .finish()
    }
}

impl fmt::Debug for GreenToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let data: &GreenTokenData = self;
        fmt::Debug::fmt(data, f)
    }
}

impl fmt::Display for GreenToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let data: &GreenTokenData = self;
        fmt::Display::fmt(data, f)
    }
}

impl fmt::Display for GreenTokenData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.text())
    }
}

impl GreenTokenData {
    /// Kind of this Token.
    #[inline]
    pub fn kind(&self) -> SyntaxKind {
        self.kind
    }

    /// Text of this Token.
    #[inline]
    pub fn text(&self) -> &str {
        self.text
    }

    /// Returns the length of the text covered by this token.
    #[inline]
    pub fn text_len(&self) -> TextSize {
        TextSize::of(self.text())
    }

    #[inline]
    pub(crate) fn parent_ptr(&self) -> Option<ptr::NonNull<GreenNodeData>> {
        self.parent.get()
    }

    #[inline]
    pub(crate) fn index(&self) -> u32 {
        self.index.get()
    }

    #[inline]
    pub(crate) fn rel_offset(&self) -> TextSize {
        self.rel_offset.get()
    }

    #[inline]
    pub(crate) fn set_parent(
        &self,
        parent: Option<ptr::NonNull<GreenNodeData>>,
        index: u32,
        rel_offset: TextSize,
    ) {
        self.parent.set(parent);
        self.index.set(index);
        self.rel_offset.set(rel_offset);
    }
}

impl GreenToken {
    /// Creates new Token.
    #[inline]
    pub fn new(kind: SyntaxKind, text: &str) -> GreenToken {
        let arena = GreenArena::new();
        GreenToken::new_in(&arena, kind, text)
    }

    pub(crate) fn new_in(arena: &Arc<GreenArena>, kind: SyntaxKind, text: &str) -> GreenToken {
        let text = arena.alloc_str(text);
        let ptr = arena.alloc(GreenTokenData {
            arena: GreenArena::raw(arena),
            kind,
            text,
            parent: Cell::new(None),
            index: Cell::new(0),
            rel_offset: Cell::new(0.into()),
        });
        GreenToken {
            repr: GreenTokenRepr {
                ptr,
                arena: arena.clone(),
            },
        }
    }

    pub(crate) fn clone_into(arena: &Arc<GreenArena>, token: &GreenTokenData) -> GreenToken {
        GreenToken::new_in(arena, token.kind(), token.text())
    }

    /// Copies a token into a new bump arena.
    ///
    /// Token `ToOwned` follows the same ownership contract as node `ToOwned`:
    /// the returned handle must not keep the original tree arena alive.
    pub(crate) fn new_owned(token: &GreenTokenData) -> GreenToken {
        let arena = GreenArena::new();
        GreenToken::clone_into(&arena, token)
    }

    #[inline]
    pub(crate) fn ptr(&self) -> ptr::NonNull<GreenTokenData> {
        self.repr.ptr
    }

    #[inline]
    pub(crate) unsafe fn from_data(ptr: ptr::NonNull<GreenTokenData>) -> GreenToken {
        GreenToken {
            repr: GreenTokenRepr {
                ptr,
                arena: GreenArena::clone_from_raw(ptr.as_ref().arena),
            },
        }
    }
}

impl ops::Deref for GreenToken {
    type Target = GreenTokenData;

    #[inline]
    fn deref(&self) -> &GreenTokenData {
        unsafe { self.repr.ptr.as_ref() }
    }
}

#[cfg(test)]
mod tests {
    use crate::{GreenToken, SyntaxKind};

    #[test]
    fn token_to_owned_uses_fresh_arena() {
        let token = GreenToken::new(SyntaxKind(1), "token");
        let token_data: &crate::GreenTokenData = &token;
        let cloned = token_data.to_owned();

        assert_eq!(cloned.kind(), token.kind());
        assert_eq!(cloned.text(), token.text());
        assert_ne!(cloned.arena.as_ptr(), token.arena.as_ptr());
    }
}
