//! Compact immutable syntax tree storage and read-only cursors.
//!
//! The parser builds one table-backed tree per source file. Public AST wrappers hold cheap cursors
//! into that tree; no green-node cloning, editing, or incremental reparsing state is preserved.

use std::{
    fmt,
    hash::{Hash, Hasher},
    iter,
    sync::Arc,
};

pub use text_size::{TextRange, TextSize};

use crate::{SyntaxError, SyntaxKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RustLanguage {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct NodeId(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct TokenId(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ElementId(u32);

#[derive(Debug, Clone, PartialEq, Eq)]
struct NodeData {
    kind: SyntaxKind,
    parent: Option<NodeId>,
    index_in_parent: u32,
    first_child: u32,
    child_count: u32,
    text_range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TokenData {
    kind: SyntaxKind,
    parent: NodeId,
    index_in_parent: u32,
    text_range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyntaxTree {
    source: Box<str>,
    nodes: Box<[NodeData]>,
    tokens: Box<[TokenData]>,
    children: Box<[ElementId]>,
    errors: Box<[SyntaxError]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyntaxTreeMemoryUsage {
    pub source_bytes: usize,
    pub node_table_bytes: usize,
    pub token_table_bytes: usize,
    pub child_table_bytes: usize,
    pub error_bytes: usize,
}

#[derive(Clone)]
pub struct SyntaxNode {
    tree: Arc<SyntaxTree>,
    id: NodeId,
}

#[derive(Clone)]
pub struct SyntaxToken {
    tree: Arc<SyntaxTree>,
    id: TokenId,
}

pub type SyntaxElement = NodeOrToken<SyntaxNode, SyntaxToken>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NodeOrToken<N, T> {
    Node(N),
    Token(T),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Direction {
    Next,
    Prev,
}

#[derive(Debug, Copy, Clone)]
pub enum WalkEvent<T> {
    Enter(T),
    Leave(T),
}

#[derive(Clone, Debug)]
pub enum TokenAtOffset<T> {
    None,
    Single(T),
    Between(T, T),
}

#[derive(Debug, Clone)]
pub struct SyntaxNodeChildren {
    parent: SyntaxNode,
    next_index: u32,
}

#[derive(Debug, Clone)]
pub struct SyntaxElementChildren {
    parent: SyntaxNode,
    next_index: u32,
}

#[derive(Debug, Clone)]
pub struct Preorder {
    start: SyntaxNode,
    next: Option<WalkEvent<SyntaxNode>>,
    skip_subtree: bool,
}

#[derive(Debug, Clone)]
pub struct PreorderWithTokens {
    start: SyntaxElement,
    next: Option<WalkEvent<SyntaxElement>>,
    skip_subtree: bool,
}

#[derive(Clone)]
pub struct SyntaxText {
    tree: Arc<SyntaxTree>,
    range: TextRange,
}

#[derive(Default)]
pub(crate) struct SyntaxTreeBuilder {
    source: String,
    nodes: Vec<NodeData>,
    tokens: Vec<TokenData>,
    children: Vec<ElementId>,
    stack: Vec<OpenNode>,
    errors: Vec<SyntaxError>,
    root: Option<NodeId>,
    offset: TextSize,
}

struct OpenNode {
    id: NodeId,
    start: TextSize,
    children: Vec<ElementId>,
}

impl ElementId {
    fn node(id: NodeId) -> Self {
        ElementId(id.0 << 1)
    }

    fn token(id: TokenId) -> Self {
        ElementId((id.0 << 1) | 1)
    }

    fn to_node(self, tree: Arc<SyntaxTree>) -> Option<SyntaxNode> {
        if self.0 & 1 == 0 {
            Some(SyntaxNode {
                tree,
                id: NodeId(self.0 >> 1),
            })
        } else {
            None
        }
    }

    fn to_element(self, tree: Arc<SyntaxTree>) -> SyntaxElement {
        if self.0 & 1 == 0 {
            SyntaxElement::Node(SyntaxNode {
                tree,
                id: NodeId(self.0 >> 1),
            })
        } else {
            SyntaxElement::Token(SyntaxToken {
                tree,
                id: TokenId(self.0 >> 1),
            })
        }
    }
}

impl SyntaxTree {
    fn node(&self, id: NodeId) -> &NodeData {
        &self.nodes[id.0 as usize]
    }

    fn token(&self, id: TokenId) -> &TokenData {
        &self.tokens[id.0 as usize]
    }

    fn children(&self, id: NodeId) -> &[ElementId] {
        let node = self.node(id);
        let start = node.first_child as usize;
        let end = start + node.child_count as usize;
        &self.children[start..end]
    }

    fn child(&self, id: NodeId, index: u32) -> Option<ElementId> {
        self.children(id).get(index as usize).copied()
    }

    fn memory_usage(&self) -> SyntaxTreeMemoryUsage {
        SyntaxTreeMemoryUsage {
            source_bytes: self.source.len(),
            node_table_bytes: self.nodes.len() * std::mem::size_of::<NodeData>(),
            token_table_bytes: self.tokens.len() * std::mem::size_of::<TokenData>(),
            child_table_bytes: self.children.len() * std::mem::size_of::<ElementId>(),
            error_bytes: self.errors.iter().map(SyntaxError::memory_usage).sum(),
        }
    }
}

impl SyntaxNode {
    pub(crate) fn new_root(tree: Arc<SyntaxTree>) -> SyntaxNode {
        SyntaxNode {
            tree,
            id: NodeId(0),
        }
    }

    pub fn kind(&self) -> SyntaxKind {
        self.tree.node(self.id).kind
    }

    pub fn text_range(&self) -> TextRange {
        self.tree.node(self.id).text_range
    }

    pub fn index(&self) -> usize {
        self.tree.node(self.id).index_in_parent as usize
    }

    pub fn text(&self) -> SyntaxText {
        SyntaxText {
            tree: self.tree.clone(),
            range: self.text_range(),
        }
    }

    pub fn parent(&self) -> Option<SyntaxNode> {
        self.tree.node(self.id).parent.map(|id| SyntaxNode {
            tree: self.tree.clone(),
            id,
        })
    }

    pub fn ancestors(&self) -> impl Iterator<Item = SyntaxNode> + use<> {
        iter::successors(Some(self.clone()), SyntaxNode::parent)
    }

    pub fn children(&self) -> SyntaxNodeChildren {
        SyntaxNodeChildren {
            parent: self.clone(),
            next_index: 0,
        }
    }

    pub fn children_with_tokens(&self) -> SyntaxElementChildren {
        SyntaxElementChildren {
            parent: self.clone(),
            next_index: 0,
        }
    }

    pub fn first_child(&self) -> Option<SyntaxNode> {
        self.children().next()
    }

    pub fn last_child(&self) -> Option<SyntaxNode> {
        self.children().last()
    }

    pub fn first_child_or_token(&self) -> Option<SyntaxElement> {
        Some(self.tree.child(self.id, 0)?.to_element(self.tree.clone()))
    }

    pub fn last_child_or_token(&self) -> Option<SyntaxElement> {
        let count = self.tree.node(self.id).child_count;
        let index = count.checked_sub(1)?;
        Some(
            self.tree
                .child(self.id, index)?
                .to_element(self.tree.clone()),
        )
    }

    pub fn next_sibling(&self) -> Option<SyntaxNode> {
        let parent = self.parent()?;
        let first_index = self.tree.node(self.id).index_in_parent.checked_add(1)? as usize;

        parent.tree.children(parent.id)[first_index..]
            .iter()
            .copied()
            .find_map(|element| element.to_node(parent.tree.clone()))
    }

    pub fn prev_sibling(&self) -> Option<SyntaxNode> {
        let parent = self.parent()?;
        let last_index = self.tree.node(self.id).index_in_parent.checked_sub(1)? as usize;

        parent.tree.children(parent.id)[..=last_index]
            .iter()
            .rev()
            .copied()
            .find_map(|element| element.to_node(parent.tree.clone()))
    }

    pub fn next_sibling_or_token(&self) -> Option<SyntaxElement> {
        let parent = self.parent()?;
        let index = self.tree.node(self.id).index_in_parent.checked_add(1)?;
        Some(
            parent
                .tree
                .child(parent.id, index)?
                .to_element(parent.tree.clone()),
        )
    }

    pub fn prev_sibling_or_token(&self) -> Option<SyntaxElement> {
        let parent = self.parent()?;
        let index = self.tree.node(self.id).index_in_parent.checked_sub(1)?;
        Some(
            parent
                .tree
                .child(parent.id, index)?
                .to_element(parent.tree.clone()),
        )
    }

    pub fn first_token(&self) -> Option<SyntaxToken> {
        self.first_child_or_token()?.first_token()
    }

    pub fn last_token(&self) -> Option<SyntaxToken> {
        self.last_child_or_token()?.last_token()
    }

    pub fn siblings(&self, direction: Direction) -> impl Iterator<Item = SyntaxNode> + use<> {
        let me = Some(self.clone());
        iter::successors(me, move |node| match direction {
            Direction::Next => node.next_sibling(),
            Direction::Prev => node.prev_sibling(),
        })
    }

    pub fn siblings_with_tokens(
        &self,
        direction: Direction,
    ) -> impl Iterator<Item = SyntaxElement> + use<> {
        let me = Some(SyntaxElement::Node(self.clone()));
        iter::successors(me, move |element| match direction {
            Direction::Next => element.next_sibling_or_token(),
            Direction::Prev => element.prev_sibling_or_token(),
        })
    }

    pub fn descendants(&self) -> impl Iterator<Item = SyntaxNode> + use<> {
        self.preorder().filter_map(|event| match event {
            WalkEvent::Enter(node) => Some(node),
            WalkEvent::Leave(_) => None,
        })
    }

    pub fn descendants_with_tokens(&self) -> impl Iterator<Item = SyntaxElement> + use<> {
        self.preorder_with_tokens().filter_map(|event| match event {
            WalkEvent::Enter(element) => Some(element),
            WalkEvent::Leave(_) => None,
        })
    }

    pub fn preorder(&self) -> Preorder {
        Preorder {
            start: self.clone(),
            next: Some(WalkEvent::Enter(self.clone())),
            skip_subtree: false,
        }
    }

    pub fn preorder_with_tokens(&self) -> PreorderWithTokens {
        let start = SyntaxElement::Node(self.clone());
        PreorderWithTokens {
            start: start.clone(),
            next: Some(WalkEvent::Enter(start)),
            skip_subtree: false,
        }
    }

    pub fn token_at_offset(&self, offset: TextSize) -> TokenAtOffset<SyntaxToken> {
        let range = self.text_range();
        assert!(
            range.start() <= offset && offset <= range.end(),
            "Bad offset: range {:?} offset {:?}",
            range,
            offset,
        );
        if range.is_empty() {
            return TokenAtOffset::None;
        }

        let mut children = self.children_with_tokens().filter(|child| {
            let child_range = child.text_range();
            !child_range.is_empty() && child_range.start() <= offset && offset <= child_range.end()
        });

        let Some(left) = children.next() else {
            return TokenAtOffset::None;
        };
        let right = children.next();
        debug_assert!(children.next().is_none());

        if let Some(right) = right {
            match (left.token_at_offset(offset), right.token_at_offset(offset)) {
                (TokenAtOffset::Single(left), TokenAtOffset::Single(right)) => {
                    TokenAtOffset::Between(left, right)
                }
                _ => unreachable!("non-empty syntax elements should resolve to leaf tokens"),
            }
        } else {
            left.token_at_offset(offset)
        }
    }

    pub fn covering_element(&self, range: TextRange) -> SyntaxElement {
        let mut result = SyntaxElement::Node(self.clone());
        loop {
            assert!(
                result.text_range().contains_range(range),
                "Bad range: node range {:?}, range {:?}",
                result.text_range(),
                range,
            );
            result = match &result {
                SyntaxElement::Token(_) => return result,
                SyntaxElement::Node(node) => match node.child_or_token_at_range(range) {
                    Some(element) => element,
                    None => return result,
                },
            };
        }
    }

    pub fn child_or_token_at_range(&self, range: TextRange) -> Option<SyntaxElement> {
        self.children_with_tokens()
            .find(|child| child.text_range().contains_range(range))
    }

    pub fn tree_memory_usage(&self) -> SyntaxTreeMemoryUsage {
        self.tree.memory_usage()
    }

    pub(crate) fn parse_errors(&self) -> &[SyntaxError] {
        &self.tree.errors
    }
}

impl SyntaxToken {
    pub fn kind(&self) -> SyntaxKind {
        self.tree.token(self.id).kind
    }

    pub fn text_range(&self) -> TextRange {
        self.tree.token(self.id).text_range
    }

    pub fn index(&self) -> usize {
        self.tree.token(self.id).index_in_parent as usize
    }

    pub fn text(&self) -> &str {
        &self.tree.source[self.text_range()]
    }

    pub fn parent(&self) -> Option<SyntaxNode> {
        Some(SyntaxNode {
            tree: self.tree.clone(),
            id: self.tree.token(self.id).parent,
        })
    }

    #[deprecated = "use `SyntaxToken::parent_ancestors` instead"]
    pub fn ancestors(&self) -> impl Iterator<Item = SyntaxNode> + use<> {
        self.parent_ancestors()
    }

    pub fn parent_ancestors(&self) -> impl Iterator<Item = SyntaxNode> + use<> {
        iter::successors(self.parent(), SyntaxNode::parent)
    }

    pub fn next_sibling_or_token(&self) -> Option<SyntaxElement> {
        let parent = self.parent()?;
        let index = self.tree.token(self.id).index_in_parent.checked_add(1)?;
        Some(
            parent
                .tree
                .child(parent.id, index)?
                .to_element(parent.tree.clone()),
        )
    }

    pub fn prev_sibling_or_token(&self) -> Option<SyntaxElement> {
        let parent = self.parent()?;
        let index = self.tree.token(self.id).index_in_parent.checked_sub(1)?;
        Some(
            parent
                .tree
                .child(parent.id, index)?
                .to_element(parent.tree.clone()),
        )
    }

    pub fn siblings_with_tokens(
        &self,
        direction: Direction,
    ) -> impl Iterator<Item = SyntaxElement> + use<> {
        let me = Some(SyntaxElement::Token(self.clone()));
        iter::successors(me, move |element| match direction {
            Direction::Next => element.next_sibling_or_token(),
            Direction::Prev => element.prev_sibling_or_token(),
        })
    }

    pub fn next_token(&self) -> Option<SyntaxToken> {
        match self.next_sibling_or_token() {
            Some(element) => element.first_token(),
            None => self
                .parent_ancestors()
                .find_map(|node| node.next_sibling_or_token())
                .and_then(|element| element.first_token()),
        }
    }

    pub fn prev_token(&self) -> Option<SyntaxToken> {
        match self.prev_sibling_or_token() {
            Some(element) => element.last_token(),
            None => self
                .parent_ancestors()
                .find_map(|node| node.prev_sibling_or_token())
                .and_then(|element| element.last_token()),
        }
    }

    pub fn tree_memory_usage(&self) -> SyntaxTreeMemoryUsage {
        self.tree.memory_usage()
    }
}

impl SyntaxElement {
    pub fn text_range(&self) -> TextRange {
        match self {
            SyntaxElement::Node(node) => node.text_range(),
            SyntaxElement::Token(token) => token.text_range(),
        }
    }

    pub fn index(&self) -> usize {
        match self {
            SyntaxElement::Node(node) => node.index(),
            SyntaxElement::Token(token) => token.index(),
        }
    }

    pub fn kind(&self) -> SyntaxKind {
        match self {
            SyntaxElement::Node(node) => node.kind(),
            SyntaxElement::Token(token) => token.kind(),
        }
    }

    pub fn parent(&self) -> Option<SyntaxNode> {
        match self {
            SyntaxElement::Node(node) => node.parent(),
            SyntaxElement::Token(token) => token.parent(),
        }
    }

    pub fn ancestors(&self) -> impl Iterator<Item = SyntaxNode> + use<> {
        let first = match self {
            SyntaxElement::Node(node) => Some(node.clone()),
            SyntaxElement::Token(token) => token.parent(),
        };
        iter::successors(first, SyntaxNode::parent)
    }

    pub fn next_sibling_or_token(&self) -> Option<SyntaxElement> {
        match self {
            SyntaxElement::Node(node) => node.next_sibling_or_token(),
            SyntaxElement::Token(token) => token.next_sibling_or_token(),
        }
    }

    pub fn prev_sibling_or_token(&self) -> Option<SyntaxElement> {
        match self {
            SyntaxElement::Node(node) => node.prev_sibling_or_token(),
            SyntaxElement::Token(token) => token.prev_sibling_or_token(),
        }
    }

    fn first_token(&self) -> Option<SyntaxToken> {
        match self {
            SyntaxElement::Node(node) => node.first_token(),
            SyntaxElement::Token(token) => Some(token.clone()),
        }
    }

    fn last_token(&self) -> Option<SyntaxToken> {
        match self {
            SyntaxElement::Node(node) => node.last_token(),
            SyntaxElement::Token(token) => Some(token.clone()),
        }
    }

    fn token_at_offset(&self, offset: TextSize) -> TokenAtOffset<SyntaxToken> {
        assert!(self.text_range().start() <= offset && offset <= self.text_range().end());
        match self {
            SyntaxElement::Node(node) => node.token_at_offset(offset),
            SyntaxElement::Token(token) => TokenAtOffset::Single(token.clone()),
        }
    }
}

impl<N, T> NodeOrToken<N, T> {
    pub fn into_node(self) -> Option<N> {
        match self {
            NodeOrToken::Node(node) => Some(node),
            NodeOrToken::Token(_) => None,
        }
    }

    pub fn into_token(self) -> Option<T> {
        match self {
            NodeOrToken::Node(_) => None,
            NodeOrToken::Token(token) => Some(token),
        }
    }

    pub fn as_node(&self) -> Option<&N> {
        match self {
            NodeOrToken::Node(node) => Some(node),
            NodeOrToken::Token(_) => None,
        }
    }

    pub fn as_token(&self) -> Option<&T> {
        match self {
            NodeOrToken::Node(_) => None,
            NodeOrToken::Token(token) => Some(token),
        }
    }
}

impl<N: fmt::Display, T: fmt::Display> fmt::Display for NodeOrToken<N, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodeOrToken::Node(node) => fmt::Display::fmt(node, f),
            NodeOrToken::Token(token) => fmt::Display::fmt(token, f),
        }
    }
}

impl<T> WalkEvent<T> {
    pub fn map<F: FnOnce(T) -> U, U>(self, f: F) -> WalkEvent<U> {
        match self {
            WalkEvent::Enter(value) => WalkEvent::Enter(f(value)),
            WalkEvent::Leave(value) => WalkEvent::Leave(f(value)),
        }
    }
}

impl<T> TokenAtOffset<T> {
    pub fn map<F: Fn(T) -> U, U>(self, f: F) -> TokenAtOffset<U> {
        match self {
            TokenAtOffset::None => TokenAtOffset::None,
            TokenAtOffset::Single(value) => TokenAtOffset::Single(f(value)),
            TokenAtOffset::Between(left, right) => TokenAtOffset::Between(f(left), f(right)),
        }
    }

    pub fn right_biased(self) -> Option<T> {
        match self {
            TokenAtOffset::None => None,
            TokenAtOffset::Single(token) => Some(token),
            TokenAtOffset::Between(_, right) => Some(right),
        }
    }

    pub fn left_biased(self) -> Option<T> {
        match self {
            TokenAtOffset::None => None,
            TokenAtOffset::Single(token) => Some(token),
            TokenAtOffset::Between(left, _) => Some(left),
        }
    }
}

impl<T> Iterator for TokenAtOffset<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        match std::mem::replace(self, TokenAtOffset::None) {
            TokenAtOffset::None => None,
            TokenAtOffset::Single(token) => Some(token),
            TokenAtOffset::Between(left, right) => {
                *self = TokenAtOffset::Single(right);
                Some(left)
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            TokenAtOffset::None => (0, Some(0)),
            TokenAtOffset::Single(_) => (1, Some(1)),
            TokenAtOffset::Between(_, _) => (2, Some(2)),
        }
    }
}

impl<T> ExactSizeIterator for TokenAtOffset<T> {}

impl Iterator for SyntaxNodeChildren {
    type Item = SyntaxNode;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(child) = self.parent.tree.child(self.parent.id, self.next_index) {
            self.next_index += 1;
            if let SyntaxElement::Node(node) = child.to_element(self.parent.tree.clone()) {
                return Some(node);
            }
        }
        None
    }
}

impl Iterator for SyntaxElementChildren {
    type Item = SyntaxElement;

    fn next(&mut self) -> Option<Self::Item> {
        let child = self.parent.tree.child(self.parent.id, self.next_index)?;
        self.next_index += 1;
        Some(child.to_element(self.parent.tree.clone()))
    }
}

impl Preorder {
    pub fn skip_subtree(&mut self) {
        self.skip_subtree = true;
    }

    fn do_skip(&mut self) {
        self.next = self.next.take().map(|next| match next {
            WalkEvent::Enter(node) if node == self.start => WalkEvent::Leave(node),
            WalkEvent::Enter(first_child) => WalkEvent::Leave(
                first_child
                    .parent()
                    .expect("entered child should have a parent"),
            ),
            WalkEvent::Leave(parent) => WalkEvent::Leave(parent),
        });
    }
}

impl Iterator for Preorder {
    type Item = WalkEvent<SyntaxNode>;

    fn next(&mut self) -> Option<Self::Item> {
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

impl PreorderWithTokens {
    pub fn skip_subtree(&mut self) {
        self.skip_subtree = true;
    }

    fn do_skip(&mut self) {
        self.next = self.next.take().map(|next| match next {
            WalkEvent::Enter(element) if element == self.start => WalkEvent::Leave(element),
            WalkEvent::Enter(first_child) => WalkEvent::Leave(
                first_child
                    .parent()
                    .expect("entered child should have a parent")
                    .into(),
            ),
            WalkEvent::Leave(parent) => WalkEvent::Leave(parent),
        });
    }
}

impl Iterator for PreorderWithTokens {
    type Item = WalkEvent<SyntaxElement>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.skip_subtree {
            self.do_skip();
            self.skip_subtree = false;
        }

        let next = self.next.take();
        self.next = next.as_ref().and_then(|next| {
            Some(match next {
                WalkEvent::Enter(element) => match element {
                    SyntaxElement::Node(node) => match node.first_child_or_token() {
                        Some(child) => WalkEvent::Enter(child),
                        None => WalkEvent::Leave(node.clone().into()),
                    },
                    SyntaxElement::Token(token) => WalkEvent::Leave(token.clone().into()),
                },
                WalkEvent::Leave(element) if element == &self.start => return None,
                WalkEvent::Leave(element) => match element.next_sibling_or_token() {
                    Some(sibling) => WalkEvent::Enter(sibling),
                    None => WalkEvent::Leave(element.parent()?.into()),
                },
            })
        });
        next
    }
}

impl SyntaxText {
    pub fn len(&self) -> TextSize {
        self.range.len()
    }

    pub fn is_empty(&self) -> bool {
        self.range.is_empty()
    }

    pub fn contains_char(&self, c: char) -> bool {
        self.as_str().contains(c)
    }

    pub fn find_char(&self, c: char) -> Option<TextSize> {
        self.as_str()
            .find(c)
            .map(|offset| TextSize::new(offset.try_into().expect("source offset should fit u32")))
    }

    pub fn char_at(&self, offset: TextSize) -> Option<char> {
        if offset >= self.len() {
            return None;
        }
        let offset = u32::from(offset) as usize;
        self.as_str()[offset..].chars().next()
    }

    pub fn slice<R: private::SyntaxTextRange>(&self, range: R) -> SyntaxText {
        let start = range.start().unwrap_or_default();
        let end = range.end().unwrap_or(self.len());
        assert!(start <= end);
        let len = end - start;
        let start = self.range.start() + start;
        let end = start + len;
        let range = TextRange::new(start, end);
        assert!(
            self.range.contains_range(range),
            "invalid slice, range: {:?}, slice: {:?}",
            self.range,
            range,
        );
        SyntaxText {
            tree: self.tree.clone(),
            range,
        }
    }

    pub fn try_fold_chunks<T, F, E>(&self, init: T, mut f: F) -> Result<T, E>
    where
        F: FnMut(T, &str) -> Result<T, E>,
    {
        f(init, self.as_str())
    }

    pub fn try_for_each_chunk<F: FnMut(&str) -> Result<(), E>, E>(
        &self,
        mut f: F,
    ) -> Result<(), E> {
        f(self.as_str())
    }

    pub fn for_each_chunk<F: FnMut(&str)>(&self, mut f: F) {
        f(self.as_str());
    }

    fn as_str(&self) -> &str {
        &self.tree.source[self.range]
    }
}

impl SyntaxTreeBuilder {
    pub(crate) fn new(source: &str) -> Self {
        Self {
            source: source.to_owned(),
            ..Self::default()
        }
    }

    pub(crate) fn generated() -> Self {
        Self::default()
    }

    pub(crate) fn finish(self) -> Arc<SyntaxTree> {
        assert!(
            self.stack.is_empty(),
            "parser should close every started syntax node"
        );
        assert_eq!(self.root, Some(NodeId(0)), "parser should produce one root");

        Arc::new(SyntaxTree {
            source: self.source.into_boxed_str(),
            nodes: self.nodes.into_boxed_slice(),
            tokens: self.tokens.into_boxed_slice(),
            children: self.children.into_boxed_slice(),
            errors: self.errors.into_boxed_slice(),
        })
    }

    pub(crate) fn token(&mut self, kind: SyntaxKind, text: &str) {
        let parent = self
            .stack
            .last_mut()
            .expect("parser should emit tokens inside a node");
        let id = TokenId(
            self.tokens
                .len()
                .try_into()
                .expect("syntax tree should fit into u32 token ids"),
        );
        let text_range = TextRange::at(self.offset, TextSize::of(text));
        let index_in_parent = parent
            .children
            .len()
            .try_into()
            .expect("syntax tree should fit into u32 child indices");

        self.tokens.push(TokenData {
            kind,
            parent: parent.id,
            index_in_parent,
            text_range,
        });
        parent.children.push(ElementId::token(id));
        self.offset += TextSize::of(text);
    }

    pub(crate) fn generated_token(&mut self, kind: SyntaxKind, text: &str) {
        self.source.push_str(text);
        self.token(kind, text);
    }

    pub(crate) fn current_offset(&self) -> TextSize {
        self.offset
    }

    pub(crate) fn start_node(&mut self, kind: SyntaxKind) {
        let id = NodeId(
            self.nodes
                .len()
                .try_into()
                .expect("syntax tree should fit into u32 node ids"),
        );
        let parent = self.stack.last().map(|node| node.id);
        self.nodes.push(NodeData {
            kind,
            parent,
            index_in_parent: 0,
            first_child: 0,
            child_count: 0,
            text_range: TextRange::empty(self.offset),
        });
        self.stack.push(OpenNode {
            id,
            start: self.offset,
            children: Vec::new(),
        });
    }

    pub(crate) fn finish_node(&mut self) {
        let open = self
            .stack
            .pop()
            .expect("parser should only finish started nodes");
        let text_range = self.finished_node_range(&open);
        let index_in_parent = self.stack.last().map_or(0, |parent| {
            parent
                .children
                .len()
                .try_into()
                .expect("syntax tree should fit into u32 child indices")
        });
        let first_child = self
            .children
            .len()
            .try_into()
            .expect("syntax tree should fit into u32 child ids");
        let child_count = open
            .children
            .len()
            .try_into()
            .expect("syntax tree should fit into u32 child counts");

        let node = &mut self.nodes[open.id.0 as usize];
        node.index_in_parent = index_in_parent;
        node.first_child = first_child;
        node.child_count = child_count;
        node.text_range = text_range;
        self.children.extend(open.children.iter().copied());

        // A node's direct children must occupy one contiguous range in the compact table.
        // Staging them on the open node lets descendants finish first without interleaving
        // their children into the parent range.
        if let Some(parent) = self.stack.last_mut() {
            parent.children.push(ElementId::node(open.id));
        } else {
            self.root = Some(open.id);
        }
    }

    pub(crate) fn error(&mut self, error: String, text_pos: TextSize) {
        self.errors
            .push(SyntaxError::new_at_offset(error, text_pos));
    }

    pub(crate) fn error_with_range(&mut self, error: impl Into<String>, text_range: TextRange) {
        self.errors.push(SyntaxError::new(error, text_range));
    }

    fn finished_node_range(&self, open: &OpenNode) -> TextRange {
        if open.children.is_empty() {
            return TextRange::empty(open.start);
        }

        let first = open.children[0];
        let last = *open
            .children
            .last()
            .expect("non-empty open node should have a last child");
        TextRange::new(
            self.element_range(first).start(),
            self.element_range(last).end(),
        )
    }

    fn element_range(&self, element: ElementId) -> TextRange {
        if element.0 & 1 == 0 {
            self.nodes[(element.0 >> 1) as usize].text_range
        } else {
            self.tokens[(element.0 >> 1) as usize].text_range
        }
    }
}

impl From<SyntaxNode> for SyntaxElement {
    fn from(node: SyntaxNode) -> Self {
        SyntaxElement::Node(node)
    }
}

impl From<SyntaxToken> for SyntaxElement {
    fn from(token: SyntaxToken) -> Self {
        SyntaxElement::Token(token)
    }
}

impl PartialEq for SyntaxNode {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && Arc::ptr_eq(&self.tree, &other.tree)
    }
}

impl Eq for SyntaxNode {}

impl Hash for SyntaxNode {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.tree).hash(state);
        self.id.hash(state);
    }
}

impl PartialEq for SyntaxToken {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && Arc::ptr_eq(&self.tree, &other.tree)
    }
}

impl Eq for SyntaxToken {}

impl Hash for SyntaxToken {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.tree).hash(state);
        self.id.hash(state);
    }
}

impl fmt::Debug for SyntaxNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            let mut level = 0;
            for event in self.preorder_with_tokens() {
                match event {
                    WalkEvent::Enter(element) => {
                        for _ in 0..level {
                            write!(f, "  ")?;
                        }
                        match element {
                            SyntaxElement::Node(node) => writeln!(f, "{:?}", node)?,
                            SyntaxElement::Token(token) => writeln!(f, "{:?}", token)?,
                        }
                        level += 1;
                    }
                    WalkEvent::Leave(_) => level -= 1,
                }
            }
            debug_assert_eq!(level, 0);
            Ok(())
        } else {
            write!(f, "{:?}@{:?}", self.kind(), self.text_range())
        }
    }
}

impl fmt::Display for SyntaxNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.text(), f)
    }
}

impl fmt::Debug for SyntaxToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}@{:?}", self.kind(), self.text_range())?;
        if self.text().len() < 25 {
            return write!(f, " {:?}", self.text());
        }

        let text = self.text();
        for idx in 21..25 {
            if text.is_char_boundary(idx) {
                let text = format!("{} ...", &text[..idx]);
                return write!(f, " {:?}", text);
            }
        }
        unreachable!("the inspected range should contain a char boundary")
    }
}

impl fmt::Display for SyntaxToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.text(), f)
    }
}

impl fmt::Debug for SyntaxText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for SyntaxText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_str(), f)
    }
}

impl From<SyntaxText> for String {
    fn from(text: SyntaxText) -> Self {
        text.to_string()
    }
}

impl PartialEq<str> for SyntaxText {
    fn eq(&self, rhs: &str) -> bool {
        self.as_str() == rhs
    }
}

impl PartialEq<SyntaxText> for str {
    fn eq(&self, rhs: &SyntaxText) -> bool {
        rhs == self
    }
}

impl PartialEq<&'_ str> for SyntaxText {
    fn eq(&self, rhs: &&str) -> bool {
        self == *rhs
    }
}

impl PartialEq<SyntaxText> for &'_ str {
    fn eq(&self, rhs: &SyntaxText) -> bool {
        rhs == self
    }
}

impl PartialEq for SyntaxText {
    fn eq(&self, other: &SyntaxText) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for SyntaxText {}

mod private {
    use std::ops;

    use super::{TextRange, TextSize};

    pub trait SyntaxTextRange {
        fn start(&self) -> Option<TextSize>;
        fn end(&self) -> Option<TextSize>;
    }

    impl SyntaxTextRange for TextRange {
        fn start(&self) -> Option<TextSize> {
            Some(TextRange::start(*self))
        }

        fn end(&self) -> Option<TextSize> {
            Some(TextRange::end(*self))
        }
    }

    impl SyntaxTextRange for ops::Range<TextSize> {
        fn start(&self) -> Option<TextSize> {
            Some(self.start)
        }

        fn end(&self) -> Option<TextSize> {
            Some(self.end)
        }
    }

    impl SyntaxTextRange for ops::RangeFrom<TextSize> {
        fn start(&self) -> Option<TextSize> {
            Some(self.start)
        }

        fn end(&self) -> Option<TextSize> {
            None
        }
    }

    impl SyntaxTextRange for ops::RangeTo<TextSize> {
        fn start(&self) -> Option<TextSize> {
            None
        }

        fn end(&self) -> Option<TextSize> {
            Some(self.end)
        }
    }

    impl SyntaxTextRange for ops::RangeFull {
        fn start(&self) -> Option<TextSize> {
            None
        }

        fn end(&self) -> Option<TextSize> {
            None
        }
    }
}
