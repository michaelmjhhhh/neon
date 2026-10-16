use std::ops::Range;

use crate::Block;

/// The smallest contiguous replacement that turns one block list into another.
///
/// Blocks are compared by content fingerprint, so a keystroke inside one
/// paragraph yields a splice of that paragraph alone even though every block
/// after it moved in the source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockSplice {
    old_range: Range<usize>,
    new_range: Range<usize>,
}

impl BlockSplice {
    /// The splice from `old` to `new`, or `None` when they render identically.
    pub fn between(old: &[Block], new: &[Block]) -> Option<Self> {
        let prefix = old
            .iter()
            .zip(new)
            .take_while(|(old, new)| old.fingerprint() == new.fingerprint())
            .count();
        if prefix == old.len() && prefix == new.len() {
            return None;
        }

        let max_suffix = old.len().min(new.len()) - prefix;
        let suffix = old
            .iter()
            .rev()
            .zip(new.iter().rev())
            .take(max_suffix)
            .take_while(|(old, new)| old.fingerprint() == new.fingerprint())
            .count();

        Some(Self {
            old_range: prefix..old.len() - suffix,
            new_range: prefix..new.len() - suffix,
        })
    }

    /// Indices of the replaced blocks in the old list.
    pub fn old_range(&self) -> Range<usize> {
        self.old_range.clone()
    }

    /// Indices of the replacement blocks in the new list.
    pub fn new_range(&self) -> Range<usize> {
        self.new_range.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MarkdownDocument;

    fn splice(old: &str, new: &str) -> Option<(Range<usize>, Range<usize>)> {
        let old = MarkdownDocument::parse(old);
        let new = MarkdownDocument::parse(new);
        BlockSplice::between(old.blocks(), new.blocks()).map(|s| (s.old_range(), s.new_range()))
    }

    #[test]
    fn identical_documents_need_no_splice() {
        assert_eq!(splice("# A\n\nb\n\nc", "# A\n\nb\n\nc"), None);
    }

    #[test]
    fn editing_one_paragraph_replaces_only_it() {
        assert_eq!(
            splice("# A\n\nbody\n\nc\n\nd", "# A\n\nbody!\n\nc\n\nd"),
            Some((1..2, 1..2))
        );
    }

    #[test]
    fn inserting_a_block_keeps_neighbours() {
        assert_eq!(splice("a\n\nc", "a\n\nb\n\nc"), Some((1..1, 1..2)));
    }

    #[test]
    fn deleting_a_block_keeps_neighbours() {
        assert_eq!(splice("a\n\nb\n\nc", "a\n\nc"), Some((1..2, 1..1)));
    }

    #[test]
    fn repeated_blocks_do_not_overlap_prefix_and_suffix() {
        assert_eq!(splice("x\n\nx", "x\n\nx\n\nx"), Some((2..2, 2..3)));
    }
}
