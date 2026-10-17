//! Shorten compact labels at the structural boundaries recorded during rendering.
//!
//! The renderer marks pieces that can become `…`, such as the arguments inside `Map<Key, Value>`.
//! Once the whole label is available, we can choose which inner pieces to hide while keeping the
//! surrounding names and punctuation. Concrete tooltip text keeps the complete rendered type.

use std::{cmp::Reverse, ops::Range};

/// Text plus places where a whole argument list or nested type can become `…`.
/// These ranges describe already-rendered text, so shortening needs no more type queries.
/// They are nested or disjoint, just like the type structure that produced them. Keep their
/// offsets tied to the original text until all replacements have been chosen.
pub(super) struct CompactOutput {
    text: String,
    max_length: Option<usize>,
    groups: Vec<ElisionGroup>,
    depth: usize,
}

impl CompactOutput {
    pub(super) fn new(max_length: Option<usize>) -> Self {
        Self {
            text: String::new(),
            max_length,
            groups: Vec::new(),
            depth: 0,
        }
    }

    pub(super) fn branch(&self) -> Self {
        Self::new(self.max_length)
    }

    pub(super) fn push_str(&mut self, text: &str) {
        self.text.push_str(text);
    }

    pub(super) fn push(&mut self, ch: char) {
        self.text.push(ch);
    }

    pub(super) fn checkpoint(&self) -> (usize, usize) {
        (self.text.len(), self.groups.len())
    }

    /// A failed type render invalidates its recorded ranges along with its partial text.
    pub(super) fn rollback(&mut self, (text_len, group_count): (usize, usize)) {
        self.text.truncate(text_len);
        self.groups.truncate(group_count);
    }

    pub(super) fn begin_group(&mut self) -> Option<usize> {
        self.max_length?;
        self.depth += 1;
        Some(self.text.len())
    }

    pub(super) fn end_group(&mut self, start: Option<usize>) {
        if let Some(start) = start {
            self.depth -= 1;
            self.groups.push(ElisionGroup {
                range: start..self.text.len(),
                depth: self.depth,
                savings: 0,
            });
        }
    }

    /// A successful replacement brings its own groups, with offsets and nesting starting at zero.
    /// Shift them to account for text and enclosing groups already in this label. For example,
    /// an iterator replacement can be appended inside `Option<...>` instead of at the label's start.
    pub(super) fn append(&mut self, mut branch: Self) {
        let offset = self.text.len();
        for group in &mut branch.groups {
            group.range.start += offset;
            group.range.end += offset;
            group.depth += self.depth;
        }
        self.text.push_str(&branch.text);
        self.groups.extend(branch.groups);
    }

    /// Collapse inner groups first, preserving outer names and closing punctuation. The budget
    /// is soft: a long identifier stays whole even when no amount of elision can make it fit.
    pub(super) fn finish(mut self) -> String {
        let Some(max_length) = self.max_length else {
            return self.text;
        };
        // Without groups there is nothing to shorten. A byte count within the budget also
        // guarantees that the character count fits, so these labels need no Unicode scan.
        if self.groups.is_empty() || self.text.len() <= max_length {
            return self.text;
        }
        // Count Unicode characters for display length (`É` counts as one), while ranges keep the
        // byte offsets recorded by the renderer for slicing the original UTF-8 string.
        let mut length = self.text.chars().count();
        if length <= max_length {
            return self.text;
        }

        // Each replacement leaves one `…` character. Empty or single-character groups cannot
        // save space, so their savings stay at zero.
        for group in &mut self.groups {
            group.savings = self.text[group.range.clone()]
                .chars()
                .count()
                .saturating_sub(1);
        }
        // At equal depth, prefer the larger group so fewer ellipses are needed.
        self.groups.sort_unstable_by_key(|group| {
            (
                Reverse(group.depth),
                Reverse(group.savings),
                group.range.start,
            )
        });
        let mut replacements = Vec::<ElisionGroup>::new();
        for group in self.groups {
            if length <= max_length {
                break;
            }
            if group.savings == 0 {
                continue;
            }
            // A parent may still need shortening after its children were collapsed:
            // `Map<Key, Vec<…>>` can become `Map<…>`. Drop the child edits and undo their
            // savings before counting the parent's replacement, so we count each character once.
            replacements.retain(|child| {
                let contained =
                    group.range.start <= child.range.start && child.range.end <= group.range.end;
                if contained {
                    length += child.savings;
                }
                !contained
            });
            length -= group.savings;
            replacements.push(group);
        }

        if replacements.is_empty() {
            return self.text;
        }
        // Apply the selected ranges from left to right. Keeping the original string until now
        // avoids shifting every later offset whenever an inner group is hidden.
        replacements.sort_unstable_by_key(|group| group.range.start);
        // Keep only enough capacity for the shortened label, counting the UTF-8 ellipses in bytes.
        let removed_bytes = replacements
            .iter()
            .map(|group| group.range.len())
            .sum::<usize>();
        let shortened_len = self.text.len() - removed_bytes + replacements.len() * '…'.len_utf8();
        let mut shortened = String::with_capacity(shortened_len);
        let mut start = 0;
        for group in replacements {
            shortened.push_str(&self.text[start..group.range.start]);
            shortened.push('…');
            start = group.range.end;
        }
        shortened.push_str(&self.text[start..]);
        shortened
    }
}

/// One region the renderer allows us to replace with a single `…`.
struct ElisionGroup {
    // Byte range in the original compact text, excluding the enclosing punctuation.
    range: Range<usize>,
    // Counts enclosing collapsible groups; deeper groups are considered first.
    depth: usize,
    // Characters removed by the replacement, calculated once the full label is available.
    savings: usize,
}
