use std::sync::Arc;

use crate::green::{GreenArena, GreenNode, GreenToken, SyntaxKind};

use super::element::GreenElement;

/// Compatibility placeholder for rowan's structural interner.
///
/// The arena-backed fork intentionally builds a single owned tree instead of
/// sharing arbitrary subtrees across parses. That keeps teardown cheap and
/// makes every persistent green allocation come from the tree's bump arena.
#[derive(Default, Debug)]
pub struct NodeCache;

impl NodeCache {
    pub(crate) fn node(
        &mut self,
        arena: &Arc<GreenArena>,
        kind: SyntaxKind,
        children: &mut Vec<(u64, GreenElement)>,
        first_child: usize,
    ) -> (u64, GreenNode) {
        let node = GreenNode::new_in(arena, kind, children.drain(first_child..).map(|(_, it)| it));
        (0, node)
    }

    pub(crate) fn token(
        &mut self,
        arena: &Arc<GreenArena>,
        kind: SyntaxKind,
        text: &str,
    ) -> (u64, GreenToken) {
        let token = GreenToken::new_in(arena, kind, text);
        (0, token)
    }
}
