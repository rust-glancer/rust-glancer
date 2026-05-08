use std::mem;

use ra_syntax::AstNode as _;

use crate::{MemoryRecorder, MemorySize};

const GREEN_CHILD_BYTES: usize = mem::size_of::<usize>() * 2;

crate::impl_memory_size_leaf!(
    ra_syntax::Edition,
    ra_syntax::SyntaxKind,
    ra_syntax::TextRange,
    ra_syntax::TextSize,
);

impl MemorySize for ra_syntax::ast::SourceFile {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        recorder.scope("syntax", |recorder| {
            self.syntax().record_memory_children(recorder);
        });
    }
}

impl<T> MemorySize for ra_syntax::Parse<T> {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        // `Parse` owns the immutable green tree; `syntax_node()` only creates a temporary cursor
        // so we can reuse the same approximate tree accounting as `SourceFile`.
        recorder.scope("syntax", |recorder| {
            self.syntax_node().record_memory_children(recorder);
        });
    }
}

impl MemorySize for ra_syntax::SyntaxNode {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        let stats = SyntaxTreeStats::from_node(self);
        stats.record(recorder);
    }
}

impl MemorySize for ra_syntax::SyntaxToken {
    fn record_memory_children(&self, recorder: &mut MemoryRecorder) {
        // The token text is owned by the green tree; the public API exposes length but not the
        // actual allocation layout, so this remains tagged as approximate.
        recorder.record_approximate::<ra_syntax::SyntaxToken>(self.text().len());
    }
}

/// Aggregates syntax-tree accounting to keep reports readable for large files.
struct SyntaxTreeStats {
    nodes: usize,
    child_edges: usize,
    tokens: usize,
    token_bytes: usize,
}

impl SyntaxTreeStats {
    fn from_node(node: &ra_syntax::SyntaxNode) -> Self {
        let mut stats = Self {
            nodes: 0,
            child_edges: 0,
            tokens: 0,
            token_bytes: 0,
        };

        for event in node.preorder_with_tokens() {
            let ra_syntax::WalkEvent::Enter(element) = event else {
                continue;
            };

            match element {
                ra_syntax::NodeOrToken::Node(node) => {
                    stats.nodes += 1;
                    stats.child_edges = stats
                        .child_edges
                        .saturating_add(node.children_with_tokens().count());
                }
                ra_syntax::NodeOrToken::Token(token) => {
                    stats.tokens += 1;
                    stats.token_bytes = stats.token_bytes.saturating_add(token.text().len());
                }
            }
        }

        stats
    }

    fn record(&self, recorder: &mut MemoryRecorder) {
        recorder.scope("green_tree", |recorder| {
            // rowan keeps the exact green-node/token layout private. We keep the original
            // wrapper-sized node/token estimates and add the important piece they miss: green
            // child entries stored inline with each green node allocation.
            recorder.scope("nodes", |recorder| {
                recorder.record_approximate::<ra_syntax::SyntaxNode>(
                    self.nodes
                        .saturating_mul(mem::size_of::<ra_syntax::SyntaxNode>()),
                );
                recorder.record_type_name(
                    crate::MemoryRecordKind::Approximate,
                    "rowan::GreenChild",
                    self.child_edges.saturating_mul(GREEN_CHILD_BYTES),
                );
            });
            recorder.scope("tokens", |recorder| {
                recorder.record_approximate::<ra_syntax::SyntaxToken>(
                    self.tokens
                        .saturating_mul(mem::size_of::<ra_syntax::SyntaxToken>()),
                );
            });
            recorder.scope("token_text", |recorder| {
                recorder.record_approximate::<str>(self.token_bytes);
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::{MemoryRecorder, MemorySize};

    #[test]
    fn records_source_file_syntax_tree_as_approximate_memory() {
        let file = ra_syntax::ast::SourceFile::parse(
            r#"
            struct User {
                name: String,
            }
            "#,
            ra_syntax::Edition::CURRENT,
        )
        .ok()
        .expect("test source should parse as a source file");

        let mut recorder = MemoryRecorder::new("source_file");
        file.record_memory_size(&mut recorder);
        let totals = recorder.totals_by_path();

        assert!(totals.contains_key("source_file.syntax.green_tree.nodes"));
        assert!(totals.contains_key("source_file.syntax.green_tree.tokens"));
        assert!(totals.contains_key("source_file.syntax.green_tree.token_text"));
    }

    #[test]
    fn records_text_ranges_as_shallow_values() {
        let range =
            ra_syntax::TextRange::new(ra_syntax::TextSize::new(1), ra_syntax::TextSize::new(4));

        assert_eq!(
            range.memory_size(),
            std::mem::size_of::<ra_syntax::TextRange>()
        );
    }
}
