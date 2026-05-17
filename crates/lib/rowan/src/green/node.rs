use std::{
    borrow::{Borrow, Cow},
    cell::Cell,
    fmt, hash,
    iter::{self, FusedIterator},
    ops, ptr, slice,
    sync::Arc,
};

use crate::{
    green::{GreenArena, GreenElement, GreenElementRef, SyntaxKind},
    GreenToken, GreenTokenData, NodeOrToken, TextRange, TextSize,
};

/// Child pointer stored inside a green arena.
#[derive(Clone, Copy)]
pub(crate) enum GreenChild {
    Node {
        rel_offset: TextSize,
        node: ptr::NonNull<GreenNodeData>,
    },
    Token {
        rel_offset: TextSize,
        token: ptr::NonNull<GreenTokenData>,
    },
}

/// Internal node data stored inside a green arena.
pub struct GreenNodeData {
    pub(crate) arena: ptr::NonNull<GreenArena>,
    kind: SyntaxKind,
    text_len: TextSize,
    parent: Cell<Option<ptr::NonNull<GreenNodeData>>>,
    index: Cell<u32>,
    rel_offset: Cell<TextSize>,
    children: &'static [GreenChild],
}

impl PartialEq for GreenNodeData {
    fn eq(&self, other: &Self) -> bool {
        self.kind() == other.kind()
            && self.text_len() == other.text_len()
            && self.slice() == other.slice()
    }
}

impl Eq for GreenNodeData {}

impl hash::Hash for GreenNodeData {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.kind().hash(state);
        self.text_len().hash(state);
        self.slice().hash(state);
    }
}

/// Internal node in the immutable tree.
/// It has other nodes and tokens as children.
#[repr(transparent)]
pub struct GreenNode {
    repr: GreenNodeRepr,
}

struct GreenNodeRepr {
    ptr: ptr::NonNull<GreenNodeData>,
    arena: Arc<GreenArena>,
}

unsafe impl Send for GreenNode {}
unsafe impl Sync for GreenNode {}

impl Clone for GreenNode {
    #[inline]
    fn clone(&self) -> Self {
        GreenNode {
            repr: GreenNodeRepr {
                ptr: self.repr.ptr,
                arena: self.repr.arena.clone(),
            },
        }
    }
}

impl PartialEq for GreenNode {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl Eq for GreenNode {}

impl hash::Hash for GreenNode {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        (**self).hash(state)
    }
}

impl ToOwned for GreenNodeData {
    type Owned = GreenNode;

    #[inline]
    fn to_owned(&self) -> GreenNode {
        GreenNode::new_subtree(self)
    }
}

impl Borrow<GreenNodeData> for GreenNode {
    #[inline]
    fn borrow(&self) -> &GreenNodeData {
        self
    }
}

impl From<Cow<'_, GreenNodeData>> for GreenNode {
    #[inline]
    fn from(cow: Cow<'_, GreenNodeData>) -> Self {
        cow.into_owned()
    }
}

impl fmt::Debug for GreenNodeData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GreenNode")
            .field("kind", &self.kind())
            .field("text_len", &self.text_len())
            .field("n_children", &self.children().len())
            .finish()
    }
}

impl fmt::Debug for GreenNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let data: &GreenNodeData = self;
        fmt::Debug::fmt(data, f)
    }
}

impl fmt::Display for GreenNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let data: &GreenNodeData = self;
        fmt::Display::fmt(data, f)
    }
}

impl fmt::Display for GreenNodeData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for child in self.children() {
            write!(f, "{}", child)?;
        }
        Ok(())
    }
}

impl GreenNodeData {
    #[inline]
    fn slice(&self) -> &[GreenChild] {
        self.children
    }

    /// Kind of this node.
    #[inline]
    pub fn kind(&self) -> SyntaxKind {
        self.kind
    }

    /// Returns the length of the text covered by this node.
    #[inline]
    pub fn text_len(&self) -> TextSize {
        self.text_len
    }

    /// Children of this node.
    #[inline]
    pub fn children(&self) -> Children<'_> {
        Children {
            raw: self.slice().iter(),
        }
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

    pub(crate) fn child_at_range(
        &self,
        rel_range: TextRange,
    ) -> Option<(usize, TextSize, GreenElementRef<'_>)> {
        let idx = self
            .slice()
            .binary_search_by(|it| {
                let child_range = it.rel_range();
                TextRange::ordering(child_range, rel_range)
            })
            // XXX: this handles empty ranges
            .unwrap_or_else(|it| it.saturating_sub(1));
        let child = &self
            .slice()
            .get(idx)
            .filter(|it| it.rel_range().contains_range(rel_range))?;
        Some((idx, child.rel_offset(), child.as_ref()))
    }

    #[must_use]
    pub fn replace_child(&self, index: usize, new_child: GreenElement) -> GreenNode {
        let mut replacement = Some(new_child);
        let children = self.children().enumerate().map(|(i, child)| {
            if i == index {
                replacement
                    .take()
                    .expect("replacement should be used exactly once")
            } else {
                child.to_owned()
            }
        });
        GreenNode::new(self.kind(), children)
    }

    #[must_use]
    pub fn insert_child(&self, index: usize, new_child: GreenElement) -> GreenNode {
        // https://github.com/rust-lang/rust/issues/34433
        self.splice_children(index..index, iter::once(new_child))
    }

    #[must_use]
    pub fn remove_child(&self, index: usize) -> GreenNode {
        self.splice_children(index..=index, iter::empty())
    }

    #[must_use]
    pub fn splice_children<R, I>(&self, range: R, replace_with: I) -> GreenNode
    where
        R: ops::RangeBounds<usize>,
        I: IntoIterator<Item = GreenElement>,
    {
        let mut children: Vec<_> = self.children().map(|it| it.to_owned()).collect();
        children.splice(range, replace_with);
        GreenNode::new(self.kind(), children)
    }
}

impl ops::Deref for GreenNode {
    type Target = GreenNodeData;

    #[inline]
    fn deref(&self) -> &GreenNodeData {
        unsafe { self.repr.ptr.as_ref() }
    }
}

impl GreenNode {
    /// Creates new Node.
    #[inline]
    pub fn new<I>(kind: SyntaxKind, children: I) -> GreenNode
    where
        I: IntoIterator<Item = GreenElement>,
        I::IntoIter: ExactSizeIterator,
    {
        let arena = GreenArena::new();
        let children = children
            .into_iter()
            .map(|child| GreenNode::clone_element_into(&arena, child.as_deref()));
        GreenNode::new_in(&arena, kind, children)
    }

    pub(crate) fn new_in<I>(arena: &Arc<GreenArena>, kind: SyntaxKind, children: I) -> GreenNode
    where
        I: IntoIterator<Item = GreenElement>,
        I::IntoIter: ExactSizeIterator,
    {
        let mut text_len: TextSize = 0.into();
        let children = children
            .into_iter()
            .enumerate()
            .map(|(index, el)| {
                let rel_offset = text_len;
                text_len += el.text_len();
                match el {
                    NodeOrToken::Node(node) => {
                        node.set_parent(None, index as u32, rel_offset);
                        GreenChild::Node {
                            rel_offset,
                            node: node.ptr(),
                        }
                    }
                    NodeOrToken::Token(token) => {
                        token.set_parent(None, index as u32, rel_offset);
                        GreenChild::Token {
                            rel_offset,
                            token: token.ptr(),
                        }
                    }
                }
            })
            .collect::<Vec<_>>();

        let children = arena.alloc_slice_copy(&children);
        let ptr = arena.alloc(GreenNodeData {
            arena: GreenArena::raw(arena),
            kind,
            text_len,
            parent: Cell::new(None),
            index: Cell::new(0),
            rel_offset: Cell::new(0.into()),
            children,
        });

        unsafe {
            for (index, child) in ptr.as_ref().slice().iter().enumerate() {
                child.set_parent(Some(ptr), index as u32);
            }
        }

        GreenNode {
            repr: GreenNodeRepr {
                ptr,
                arena: arena.clone(),
            },
        }
    }

    pub(crate) fn clone_into(arena: &Arc<GreenArena>, node: &GreenNodeData) -> GreenNode {
        let children = node
            .children()
            .map(|child| GreenNode::clone_element_into(arena, child));
        GreenNode::new_in(arena, node.kind(), children)
    }

    /// Copies a green subtree into a new bump arena.
    ///
    /// Public APIs such as `SyntaxNode::clone_subtree` and
    /// `GreenNodeData::to_owned` promise an independent tree. Reconstructing a
    /// handle to the existing data would keep the whole source arena alive,
    /// even when the user only wants a tiny subtree.
    pub(crate) fn new_subtree(node: &GreenNodeData) -> GreenNode {
        let arena = GreenArena::new();
        GreenNode::clone_into(&arena, node)
    }

    #[inline]
    pub(crate) fn ptr(&self) -> ptr::NonNull<GreenNodeData> {
        self.repr.ptr
    }

    #[inline]
    pub(crate) unsafe fn from_data(ptr: ptr::NonNull<GreenNodeData>) -> GreenNode {
        GreenNode {
            repr: GreenNodeRepr {
                ptr,
                arena: GreenArena::clone_from_raw(ptr.as_ref().arena),
            },
        }
    }

    fn clone_element_into(arena: &Arc<GreenArena>, element: GreenElementRef<'_>) -> GreenElement {
        match element {
            NodeOrToken::Node(node) => GreenNode::clone_into(arena, node).into(),
            NodeOrToken::Token(token) => GreenToken::clone_into(arena, token).into(),
        }
    }

    #[inline]
    fn set_parent(
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
    #[inline]
    fn set_parent(
        &self,
        parent: Option<ptr::NonNull<GreenNodeData>>,
        index: u32,
        rel_offset: TextSize,
    ) {
        (**self).set_parent(parent, index, rel_offset);
    }
}

impl GreenChild {
    #[inline]
    pub(crate) fn as_ref(&self) -> GreenElementRef<'_> {
        match self {
            GreenChild::Node { node, .. } => NodeOrToken::Node(unsafe { node.as_ref() }),
            GreenChild::Token { token, .. } => NodeOrToken::Token(unsafe { token.as_ref() }),
        }
    }

    #[inline]
    pub(crate) fn rel_offset(&self) -> TextSize {
        match self {
            GreenChild::Node { rel_offset, .. } | GreenChild::Token { rel_offset, .. } => {
                *rel_offset
            }
        }
    }

    #[inline]
    fn rel_range(&self) -> TextRange {
        let len = self.as_ref().text_len();
        TextRange::at(self.rel_offset(), len)
    }

    #[inline]
    fn set_parent(&self, parent: Option<ptr::NonNull<GreenNodeData>>, index: u32) {
        match self {
            GreenChild::Node { rel_offset, node } => unsafe {
                node.as_ref().set_parent(parent, index, *rel_offset);
            },
            GreenChild::Token { rel_offset, token } => unsafe {
                token.as_ref().set_parent(parent, index, *rel_offset);
            },
        }
    }
}

impl fmt::Debug for GreenChild {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.as_ref() {
            NodeOrToken::Node(node) => f
                .debug_struct("Node")
                .field("rel_offset", &self.rel_offset())
                .field("node", node)
                .finish(),
            NodeOrToken::Token(token) => f
                .debug_struct("Token")
                .field("rel_offset", &self.rel_offset())
                .field("token", token)
                .finish(),
        }
    }
}

impl PartialEq for GreenChild {
    fn eq(&self, other: &Self) -> bool {
        self.rel_offset() == other.rel_offset() && self.as_ref() == other.as_ref()
    }
}

impl Eq for GreenChild {}

impl hash::Hash for GreenChild {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.rel_offset().hash(state);
        match self.as_ref() {
            NodeOrToken::Node(node) => node.hash(state),
            NodeOrToken::Token(token) => token.hash(state),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Children<'a> {
    pub(crate) raw: slice::Iter<'a, GreenChild>,
}

// NB: forward everything stable that iter::Slice specializes as of Rust 1.39.0
impl ExactSizeIterator for Children<'_> {
    #[inline(always)]
    fn len(&self) -> usize {
        self.raw.len()
    }
}

impl<'a> Iterator for Children<'a> {
    type Item = GreenElementRef<'a>;

    #[inline]
    fn next(&mut self) -> Option<GreenElementRef<'a>> {
        self.raw.next().map(GreenChild::as_ref)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.raw.size_hint()
    }

    #[inline]
    fn count(self) -> usize
    where
        Self: Sized,
    {
        self.raw.count()
    }

    #[inline]
    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        self.raw.nth(n).map(GreenChild::as_ref)
    }

    #[inline]
    fn last(mut self) -> Option<Self::Item>
    where
        Self: Sized,
    {
        self.next_back()
    }

    #[inline]
    fn fold<Acc, Fold>(mut self, init: Acc, mut f: Fold) -> Acc
    where
        Fold: FnMut(Acc, Self::Item) -> Acc,
    {
        let mut accum = init;
        while let Some(x) = self.next() {
            accum = f(accum, x);
        }
        accum
    }
}

impl<'a> DoubleEndedIterator for Children<'a> {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        self.raw.next_back().map(GreenChild::as_ref)
    }

    #[inline]
    fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
        self.raw.nth_back(n).map(GreenChild::as_ref)
    }

    #[inline]
    fn rfold<Acc, Fold>(mut self, init: Acc, mut f: Fold) -> Acc
    where
        Fold: FnMut(Acc, Self::Item) -> Acc,
    {
        let mut accum = init;
        while let Some(x) = self.next_back() {
            accum = f(accum, x);
        }
        accum
    }
}

impl FusedIterator for Children<'_> {}
