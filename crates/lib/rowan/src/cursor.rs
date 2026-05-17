//! Read-only cursors for convenient access to syntax trees.
//!
//! This fork is optimized for batch analysis. Green trees are immutable and
//! arena-owned, so cursors are just cheap views over green nodes/tokens plus an
//! absolute offset and a root boundary. APIs that depended on rowan's
//! interactive mutable zipper remain present for compatibility, but panic when
//! called.

use std::{
    borrow::Cow,
    fmt,
    hash::{Hash, Hasher},
    iter,
    ops::Range,
    ptr,
};

use crate::{
    green::{GreenElementRef, GreenNodeData, GreenTokenData, SyntaxKind},
    Direction, GreenNode, GreenToken, NodeOrToken, SyntaxText, TextRange, TextSize, TokenAtOffset,
    WalkEvent,
};

pub type SyntaxElement = NodeOrToken<SyntaxNode, SyntaxToken>;

#[derive(Clone)]
pub struct SyntaxNode {
    green: GreenNode,
    offset: TextSize,
    root: ptr::NonNull<GreenNodeData>,
}

#[derive(Clone, Debug)]
pub struct SyntaxToken {
    green: GreenToken,
    offset: TextSize,
    root: ptr::NonNull<GreenNodeData>,
}

impl SyntaxNode {
    pub fn new_root(green: GreenNode) -> SyntaxNode {
        let root = green.ptr();
        SyntaxNode {
            green,
            offset: 0.into(),
            root,
        }
    }

    pub fn new_root_mut(_green: GreenNode) -> SyntaxNode {
        panic!("rowan bump arena fork is read-only: mutable roots are unsupported")
    }

    fn new_child(
        green: &GreenNodeData,
        parent: SyntaxNode,
        _index: u32,
        offset: TextSize,
    ) -> SyntaxNode {
        let green = unsafe { GreenNode::from_data(green.into()) };
        SyntaxNode {
            green,
            offset,
            root: parent.root,
        }
    }

    pub fn is_mutable(&self) -> bool {
        false
    }

    pub fn clone_for_update(&self) -> SyntaxNode {
        panic!("rowan bump arena fork is read-only: clone_for_update is unsupported")
    }

    pub fn clone_subtree(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green_ref().to_owned())
    }

    pub fn replace_with(&self, replacement: GreenNode) -> GreenNode {
        assert_eq!(self.kind(), replacement.kind());
        match self.parent() {
            None => replacement,
            Some(parent) => {
                let new_parent = parent
                    .green_ref()
                    .replace_child(self.index(), replacement.into());
                parent.replace_with(new_parent)
            }
        }
    }

    #[inline]
    pub fn kind(&self) -> SyntaxKind {
        self.green_ref().kind()
    }

    #[inline]
    fn offset(&self) -> TextSize {
        self.offset
    }

    #[inline]
    pub fn text_range(&self) -> TextRange {
        TextRange::at(self.offset(), self.green_ref().text_len())
    }

    #[inline]
    pub fn index(&self) -> usize {
        if self.green.ptr() == self.root {
            0
        } else {
            self.green_ref().index() as usize
        }
    }

    #[inline]
    pub fn text(&self) -> SyntaxText {
        SyntaxText::new(self.clone())
    }

    #[inline]
    pub fn green(&self) -> Cow<'_, GreenNodeData> {
        Cow::Borrowed(self.green_ref())
    }

    #[inline]
    fn green_ref(&self) -> &GreenNodeData {
        &self.green
    }

    #[inline]
    pub fn parent(&self) -> Option<SyntaxNode> {
        if self.green.ptr() == self.root {
            return None;
        }
        let parent_ptr = self.green_ref().parent_ptr()?;
        let green = unsafe { GreenNode::from_data(parent_ptr) };
        Some(SyntaxNode {
            green,
            offset: self.offset() - self.green_ref().rel_offset(),
            root: self.root,
        })
    }

    #[inline]
    pub fn ancestors(&self) -> impl Iterator<Item = SyntaxNode> {
        iter::successors(Some(self.clone()), SyntaxNode::parent)
    }

    #[inline]
    pub fn children(&self) -> SyntaxNodeChildren {
        SyntaxNodeChildren::new(self.clone())
    }

    #[inline]
    pub fn children_with_tokens(&self) -> SyntaxElementChildren {
        SyntaxElementChildren::new(self.clone())
    }

    pub fn first_child(&self) -> Option<SyntaxNode> {
        self.green_ref()
            .children()
            .raw
            .enumerate()
            .find_map(|(index, child)| {
                child.as_ref().into_node().map(|green| {
                    SyntaxNode::new_child(
                        green,
                        self.clone(),
                        index as u32,
                        self.offset() + child.rel_offset(),
                    )
                })
            })
    }

    pub fn last_child(&self) -> Option<SyntaxNode> {
        self.green_ref()
            .children()
            .raw
            .enumerate()
            .rev()
            .find_map(|(index, child)| {
                child.as_ref().into_node().map(|green| {
                    SyntaxNode::new_child(
                        green,
                        self.clone(),
                        index as u32,
                        self.offset() + child.rel_offset(),
                    )
                })
            })
    }

    pub fn first_child_or_token(&self) -> Option<SyntaxElement> {
        self.green_ref().children().raw.next().map(|child| {
            SyntaxElement::new(
                child.as_ref(),
                self.clone(),
                0,
                self.offset() + child.rel_offset(),
            )
        })
    }

    pub fn last_child_or_token(&self) -> Option<SyntaxElement> {
        self.green_ref()
            .children()
            .raw
            .enumerate()
            .next_back()
            .map(|(index, child)| {
                SyntaxElement::new(
                    child.as_ref(),
                    self.clone(),
                    index as u32,
                    self.offset() + child.rel_offset(),
                )
            })
    }

    pub fn next_sibling(&self) -> Option<SyntaxNode> {
        let parent = self.parent()?;
        let mut siblings = parent.green_ref().children().raw.enumerate();
        siblings.nth(self.index());
        siblings.find_map(|(index, child)| {
            child.as_ref().into_node().map(|green| {
                SyntaxNode::new_child(
                    green,
                    parent.clone(),
                    index as u32,
                    parent.offset() + child.rel_offset(),
                )
            })
        })
    }

    pub fn prev_sibling(&self) -> Option<SyntaxNode> {
        let parent = self.parent()?;
        let mut rev_siblings = parent.green_ref().children().raw.enumerate().rev();
        let index = rev_siblings.len().checked_sub(self.index() + 1)?;
        rev_siblings.nth(index);
        rev_siblings.find_map(|(index, child)| {
            child.as_ref().into_node().map(|green| {
                SyntaxNode::new_child(
                    green,
                    parent.clone(),
                    index as u32,
                    parent.offset() + child.rel_offset(),
                )
            })
        })
    }

    pub fn next_sibling_or_token(&self) -> Option<SyntaxElement> {
        let parent = self.parent()?;
        let mut siblings = parent.green_ref().children().raw.enumerate();
        let index = self.index() + 1;
        siblings.nth(index).map(|(index, child)| {
            SyntaxElement::new(
                child.as_ref(),
                parent.clone(),
                index as u32,
                parent.offset() + child.rel_offset(),
            )
        })
    }

    pub fn prev_sibling_or_token(&self) -> Option<SyntaxElement> {
        let parent = self.parent()?;
        let mut siblings = parent.green_ref().children().raw.enumerate();
        let index = self.index().checked_sub(1)?;
        siblings.nth(index).map(|(index, child)| {
            SyntaxElement::new(
                child.as_ref(),
                parent.clone(),
                index as u32,
                parent.offset() + child.rel_offset(),
            )
        })
    }

    pub fn first_token(&self) -> Option<SyntaxToken> {
        self.first_child_or_token()?.first_token()
    }

    pub fn last_token(&self) -> Option<SyntaxToken> {
        self.last_child_or_token()?.last_token()
    }

    #[inline]
    pub fn siblings(&self, direction: Direction) -> impl Iterator<Item = SyntaxNode> {
        iter::successors(Some(self.clone()), move |node| match direction {
            Direction::Next => node.next_sibling(),
            Direction::Prev => node.prev_sibling(),
        })
    }

    #[inline]
    pub fn siblings_with_tokens(
        &self,
        direction: Direction,
    ) -> impl Iterator<Item = SyntaxElement> {
        let me: SyntaxElement = self.clone().into();
        iter::successors(Some(me), move |el| match direction {
            Direction::Next => el.next_sibling_or_token(),
            Direction::Prev => el.prev_sibling_or_token(),
        })
    }

    #[inline]
    pub fn descendants(&self) -> impl Iterator<Item = SyntaxNode> {
        self.preorder().filter_map(|event| match event {
            WalkEvent::Enter(node) => Some(node),
            WalkEvent::Leave(_) => None,
        })
    }

    #[inline]
    pub fn descendants_with_tokens(&self) -> impl Iterator<Item = SyntaxElement> {
        self.preorder_with_tokens().filter_map(|event| match event {
            WalkEvent::Enter(it) => Some(it),
            WalkEvent::Leave(_) => None,
        })
    }

    #[inline]
    pub fn preorder(&self) -> Preorder {
        Preorder::new(self.clone())
    }

    #[inline]
    pub fn preorder_with_tokens(&self) -> PreorderWithTokens {
        PreorderWithTokens::new(self.clone())
    }

    pub fn token_at_offset(&self, offset: TextSize) -> TokenAtOffset<SyntaxToken> {
        // TODO: this could be faster if we first drill down to the node, and
        // only then switch to token search.
        let range = self.text_range();
        assert!(
            range.start() <= offset && offset <= range.end(),
            "Bad offset: range {:?} offset {:?}",
            range,
            offset
        );
        if range.is_empty() {
            return TokenAtOffset::None;
        }

        let mut children = self.children_with_tokens().filter(|child| {
            let child_range = child.text_range();
            !child_range.is_empty()
                && (child_range.start() <= offset && offset <= child_range.end())
        });

        let left = children.next().unwrap();
        let right = children.next();
        assert!(children.next().is_none());

        if let Some(right) = right {
            match (left.token_at_offset(offset), right.token_at_offset(offset)) {
                (TokenAtOffset::Single(left), TokenAtOffset::Single(right)) => {
                    TokenAtOffset::Between(left, right)
                }
                _ => unreachable!(),
            }
        } else {
            left.token_at_offset(offset)
        }
    }

    pub fn covering_element(&self, range: TextRange) -> SyntaxElement {
        let mut res: SyntaxElement = self.clone().into();
        loop {
            assert!(
                res.text_range().contains_range(range),
                "Bad range: node range {:?}, range {:?}",
                res.text_range(),
                range,
            );
            res = match &res {
                NodeOrToken::Token(_) => return res,
                NodeOrToken::Node(node) => match node.child_or_token_at_range(range) {
                    Some(it) => it,
                    None => return res,
                },
            };
        }
    }

    pub fn child_or_token_at_range(&self, range: TextRange) -> Option<SyntaxElement> {
        let rel_range = range - self.offset();
        self.green_ref()
            .child_at_range(rel_range)
            .map(|(index, rel_offset, green)| {
                SyntaxElement::new(
                    green,
                    self.clone(),
                    index as u32,
                    self.offset() + rel_offset,
                )
            })
    }

    pub fn splice_children(&self, _to_delete: Range<usize>, _to_insert: Vec<SyntaxElement>) {
        panic!("rowan bump arena fork is read-only: splice_children is unsupported")
    }

    pub fn detach(&self) {
        panic!("rowan bump arena fork is read-only: detach is unsupported")
    }

    fn key(&self) -> (*const (), TextSize) {
        (self.green.ptr().as_ptr().cast::<()>(), self.offset())
    }
}

impl SyntaxToken {
    fn new(
        green: &GreenTokenData,
        parent: SyntaxNode,
        _index: u32,
        offset: TextSize,
    ) -> SyntaxToken {
        let green = unsafe { GreenToken::from_data(green.into()) };
        SyntaxToken {
            green,
            offset,
            root: parent.root,
        }
    }

    pub fn replace_with(&self, replacement: GreenToken) -> GreenNode {
        assert_eq!(self.kind(), replacement.kind());
        let parent = self.parent().unwrap();
        let new_parent = parent
            .green_ref()
            .replace_child(self.index(), replacement.into());
        parent.replace_with(new_parent)
    }

    #[inline]
    pub fn kind(&self) -> SyntaxKind {
        self.green.kind()
    }

    #[inline]
    pub fn text_range(&self) -> TextRange {
        TextRange::at(self.offset, self.green.text_len())
    }

    #[inline]
    pub fn index(&self) -> usize {
        self.green.index() as usize
    }

    #[inline]
    pub fn text(&self) -> &str {
        self.green.text()
    }

    #[inline]
    pub fn green(&self) -> &GreenTokenData {
        &self.green
    }

    #[inline]
    pub fn parent(&self) -> Option<SyntaxNode> {
        let parent_ptr = self.green.parent_ptr()?;
        let green = unsafe { GreenNode::from_data(parent_ptr) };
        Some(SyntaxNode {
            green,
            offset: self.offset - self.green.rel_offset(),
            root: self.root,
        })
    }

    #[inline]
    pub fn ancestors(&self) -> impl Iterator<Item = SyntaxNode> {
        std::iter::successors(self.parent(), SyntaxNode::parent)
    }

    pub fn next_sibling_or_token(&self) -> Option<SyntaxElement> {
        let parent = self.parent()?;
        let mut siblings = parent.green_ref().children().raw.enumerate();
        let index = self.index() + 1;
        siblings.nth(index).map(|(index, child)| {
            SyntaxElement::new(
                child.as_ref(),
                parent.clone(),
                index as u32,
                parent.offset() + child.rel_offset(),
            )
        })
    }

    pub fn prev_sibling_or_token(&self) -> Option<SyntaxElement> {
        let parent = self.parent()?;
        let mut siblings = parent.green_ref().children().raw.enumerate();
        let index = self.index().checked_sub(1)?;
        siblings.nth(index).map(|(index, child)| {
            SyntaxElement::new(
                child.as_ref(),
                parent.clone(),
                index as u32,
                parent.offset() + child.rel_offset(),
            )
        })
    }

    #[inline]
    pub fn siblings_with_tokens(
        &self,
        direction: Direction,
    ) -> impl Iterator<Item = SyntaxElement> {
        let me: SyntaxElement = self.clone().into();
        iter::successors(Some(me), move |el| match direction {
            Direction::Next => el.next_sibling_or_token(),
            Direction::Prev => el.prev_sibling_or_token(),
        })
    }

    pub fn next_token(&self) -> Option<SyntaxToken> {
        match self.next_sibling_or_token() {
            Some(element) => element.first_token(),
            None => self
                .ancestors()
                .find_map(|it| it.next_sibling_or_token())
                .and_then(|element| element.first_token()),
        }
    }

    pub fn prev_token(&self) -> Option<SyntaxToken> {
        match self.prev_sibling_or_token() {
            Some(element) => element.last_token(),
            None => self
                .ancestors()
                .find_map(|it| it.prev_sibling_or_token())
                .and_then(|element| element.last_token()),
        }
    }

    pub fn detach(&self) {
        panic!("rowan bump arena fork is read-only: detach is unsupported")
    }

    fn key(&self) -> (*const (), TextSize) {
        (self.green.ptr().as_ptr().cast::<()>(), self.offset)
    }
}

impl SyntaxElement {
    fn new(
        element: GreenElementRef<'_>,
        parent: SyntaxNode,
        index: u32,
        offset: TextSize,
    ) -> SyntaxElement {
        match element {
            NodeOrToken::Node(node) => SyntaxNode::new_child(node, parent, index, offset).into(),
            NodeOrToken::Token(token) => SyntaxToken::new(token, parent, index, offset).into(),
        }
    }

    #[inline]
    pub fn text_range(&self) -> TextRange {
        match self {
            NodeOrToken::Node(it) => it.text_range(),
            NodeOrToken::Token(it) => it.text_range(),
        }
    }

    #[inline]
    pub fn index(&self) -> usize {
        match self {
            NodeOrToken::Node(it) => it.index(),
            NodeOrToken::Token(it) => it.index(),
        }
    }

    #[inline]
    pub fn kind(&self) -> SyntaxKind {
        match self {
            NodeOrToken::Node(it) => it.kind(),
            NodeOrToken::Token(it) => it.kind(),
        }
    }

    #[inline]
    pub fn parent(&self) -> Option<SyntaxNode> {
        match self {
            NodeOrToken::Node(it) => it.parent(),
            NodeOrToken::Token(it) => it.parent(),
        }
    }

    #[inline]
    pub fn ancestors(&self) -> impl Iterator<Item = SyntaxNode> {
        let first = match self {
            NodeOrToken::Node(it) => Some(it.clone()),
            NodeOrToken::Token(it) => it.parent(),
        };
        iter::successors(first, SyntaxNode::parent)
    }

    pub fn first_token(&self) -> Option<SyntaxToken> {
        match self {
            NodeOrToken::Node(it) => it.first_token(),
            NodeOrToken::Token(it) => Some(it.clone()),
        }
    }

    pub fn last_token(&self) -> Option<SyntaxToken> {
        match self {
            NodeOrToken::Node(it) => it.last_token(),
            NodeOrToken::Token(it) => Some(it.clone()),
        }
    }

    pub fn next_sibling_or_token(&self) -> Option<SyntaxElement> {
        match self {
            NodeOrToken::Node(it) => it.next_sibling_or_token(),
            NodeOrToken::Token(it) => it.next_sibling_or_token(),
        }
    }

    pub fn prev_sibling_or_token(&self) -> Option<SyntaxElement> {
        match self {
            NodeOrToken::Node(it) => it.prev_sibling_or_token(),
            NodeOrToken::Token(it) => it.prev_sibling_or_token(),
        }
    }

    fn token_at_offset(&self, offset: TextSize) -> TokenAtOffset<SyntaxToken> {
        assert!(self.text_range().start() <= offset && offset <= self.text_range().end());
        match self {
            NodeOrToken::Token(token) => TokenAtOffset::Single(token.clone()),
            NodeOrToken::Node(node) => node.token_at_offset(offset),
        }
    }

    pub fn detach(&self) {
        panic!("rowan bump arena fork is read-only: detach is unsupported")
    }
}

// region: impls

// Identity semantics for hash & eq
impl PartialEq for SyntaxNode {
    #[inline]
    fn eq(&self, other: &SyntaxNode) -> bool {
        self.key() == other.key()
    }
}

impl Eq for SyntaxNode {}

impl Hash for SyntaxNode {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key().hash(state);
    }
}

impl fmt::Debug for SyntaxNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SyntaxNode")
            .field("kind", &self.kind())
            .field("text_range", &self.text_range())
            .finish()
    }
}

impl fmt::Display for SyntaxNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.preorder_with_tokens()
            .filter_map(|event| match event {
                WalkEvent::Enter(NodeOrToken::Token(token)) => Some(token),
                _ => None,
            })
            .try_for_each(|it| fmt::Display::fmt(&it, f))
    }
}

// Identity semantics for hash & eq
impl PartialEq for SyntaxToken {
    #[inline]
    fn eq(&self, other: &SyntaxToken) -> bool {
        self.key() == other.key()
    }
}

impl Eq for SyntaxToken {}

impl Hash for SyntaxToken {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key().hash(state);
    }
}

impl fmt::Display for SyntaxToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.text(), f)
    }
}

impl From<SyntaxNode> for SyntaxElement {
    #[inline]
    fn from(node: SyntaxNode) -> SyntaxElement {
        NodeOrToken::Node(node)
    }
}

impl From<SyntaxToken> for SyntaxElement {
    #[inline]
    fn from(token: SyntaxToken) -> SyntaxElement {
        NodeOrToken::Token(token)
    }
}

// endregion

// region: iterators

#[derive(Clone, Debug)]
pub struct SyntaxNodeChildren {
    next: Option<SyntaxNode>,
}

impl SyntaxNodeChildren {
    fn new(parent: SyntaxNode) -> SyntaxNodeChildren {
        SyntaxNodeChildren {
            next: parent.first_child(),
        }
    }
}

impl Iterator for SyntaxNodeChildren {
    type Item = SyntaxNode;

    fn next(&mut self) -> Option<SyntaxNode> {
        self.next.take().map(|next| {
            self.next = next.next_sibling();
            next
        })
    }
}

#[derive(Clone, Debug)]
pub struct SyntaxElementChildren {
    next: Option<SyntaxElement>,
}

impl SyntaxElementChildren {
    fn new(parent: SyntaxNode) -> SyntaxElementChildren {
        SyntaxElementChildren {
            next: parent.first_child_or_token(),
        }
    }
}

impl Iterator for SyntaxElementChildren {
    type Item = SyntaxElement;

    fn next(&mut self) -> Option<SyntaxElement> {
        self.next.take().map(|next| {
            self.next = next.next_sibling_or_token();
            next
        })
    }
}

#[derive(Debug, Clone)]
pub struct Preorder {
    start: SyntaxNode,
    next: Option<WalkEvent<SyntaxNode>>,
    skip_subtree: bool,
}

impl Preorder {
    fn new(start: SyntaxNode) -> Preorder {
        let next = Some(WalkEvent::Enter(start.clone()));
        Preorder {
            start,
            next,
            skip_subtree: false,
        }
    }

    pub fn skip_subtree(&mut self) {
        self.skip_subtree = true;
    }

    #[cold]
    fn do_skip(&mut self) {
        self.next = self.next.take().map(|next| match next {
            WalkEvent::Enter(node) if node == self.start => WalkEvent::Leave(node),
            WalkEvent::Enter(first_child) => WalkEvent::Leave(first_child.parent().unwrap()),
            WalkEvent::Leave(parent) => WalkEvent::Leave(parent),
        })
    }
}

impl Iterator for Preorder {
    type Item = WalkEvent<SyntaxNode>;

    fn next(&mut self) -> Option<WalkEvent<SyntaxNode>> {
        if self.skip_subtree {
            self.do_skip();
            self.skip_subtree = false;
        }
        let next = self.next.take();
        self.next = next.as_ref().and_then(|next| {
            Some(match next {
                WalkEvent::Enter(node) => match node.first_child() {
                    Some(child) => WalkEvent::Enter(child),
                    None => WalkEvent::Leave(node.clone()),
                },
                WalkEvent::Leave(node) => {
                    if node == &self.start {
                        return None;
                    }
                    match node.next_sibling() {
                        Some(sibling) => WalkEvent::Enter(sibling),
                        None => WalkEvent::Leave(node.parent()?),
                    }
                }
            })
        });
        next
    }
}

#[derive(Debug, Clone)]
pub struct PreorderWithTokens {
    start: SyntaxElement,
    next: Option<WalkEvent<SyntaxElement>>,
    skip_subtree: bool,
}

impl PreorderWithTokens {
    fn new(start: SyntaxNode) -> PreorderWithTokens {
        let next = Some(WalkEvent::Enter(start.clone().into()));
        PreorderWithTokens {
            start: start.into(),
            next,
            skip_subtree: false,
        }
    }

    pub fn skip_subtree(&mut self) {
        self.skip_subtree = true;
    }

    #[cold]
    fn do_skip(&mut self) {
        self.next = self.next.take().map(|next| match next {
            WalkEvent::Enter(el) if el == self.start => WalkEvent::Leave(el),
            WalkEvent::Enter(first_child) => WalkEvent::Leave(first_child.parent().unwrap().into()),
            WalkEvent::Leave(parent) => WalkEvent::Leave(parent),
        })
    }
}

impl Iterator for PreorderWithTokens {
    type Item = WalkEvent<SyntaxElement>;

    fn next(&mut self) -> Option<WalkEvent<SyntaxElement>> {
        if self.skip_subtree {
            self.do_skip();
            self.skip_subtree = false;
        }
        let next = self.next.take();
        self.next = next.as_ref().and_then(|next| {
            Some(match next {
                WalkEvent::Enter(el) => match el {
                    NodeOrToken::Node(node) => match node.first_child_or_token() {
                        Some(child) => WalkEvent::Enter(child),
                        None => WalkEvent::Leave(node.clone().into()),
                    },
                    NodeOrToken::Token(token) => WalkEvent::Leave(token.clone().into()),
                },
                WalkEvent::Leave(el) if el == &self.start => return None,
                WalkEvent::Leave(el) => match el.next_sibling_or_token() {
                    Some(sibling) => WalkEvent::Enter(sibling),
                    None => WalkEvent::Leave(el.parent()?.into()),
                },
            })
        });
        next
    }
}

// endregion

#[cfg(test)]
mod tests {
    use crate::{GreenNodeBuilder, SyntaxKind};

    use super::SyntaxNode;

    #[test]
    fn clone_subtree_uses_fresh_arena() {
        let mut builder = GreenNodeBuilder::new();
        builder.start_node(SyntaxKind(0));
        builder.start_node(SyntaxKind(1));
        builder.token(SyntaxKind(2), "child");
        builder.finish_node();
        builder.token(SyntaxKind(3), "tail");
        builder.finish_node();

        let root = SyntaxNode::new_root(builder.finish());
        let child = root
            .first_child()
            .expect("root should contain the child subtree");
        let cloned = child.clone_subtree();

        assert!(cloned.parent().is_none());
        assert_eq!(cloned.to_string(), child.to_string());
        assert_ne!(
            cloned.green_ref().arena.as_ptr(),
            child.green_ref().arena.as_ptr()
        );
    }
}
