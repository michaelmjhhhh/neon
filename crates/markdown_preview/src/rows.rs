use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::ops::Range;

use markdown::{BlockKind, MarkdownDocument};

const LIST_CHUNK_THRESHOLD: usize = 64;
const LIST_CHUNK_ITEMS: usize = 16;

struct Row {
    block_ix: usize,
    items: Option<Range<usize>>,
    source_range: Range<usize>,
    fingerprint: u64,
    id: u64,
}

/// The preview's scroll rows. Ordinary documents use the block array directly;
/// only documents with oversized lists allocate the additional row index.
#[derive(Default)]
pub(crate) struct PreviewRows {
    expanded: Option<Vec<Row>>,
}

impl PreviewRows {
    pub(crate) fn new(document: &MarkdownDocument, keep_chunking: bool) -> Self {
        if !keep_chunking && !document.blocks().iter().any(|block| {
            matches!(block.kind(), BlockKind::List(list) if list.items().len() > LIST_CHUNK_THRESHOLD)
        }) {
            return Self::default();
        }

        let mut rows = Vec::new();
        let mut occurrences = HashMap::<u64, u64>::new();
        for (block_ix, block) in document.blocks().iter().enumerate() {
            if let BlockKind::List(list) = block.kind() {
                let item_count = list.items().len();
                for start in (0..item_count).step_by(LIST_CHUNK_ITEMS) {
                    let end = (start + LIST_CHUNK_ITEMS).min(item_count);
                    let source_start = if start == 0 {
                        block.source_range().start
                    } else {
                        list.items()[start].source_range().start
                    };
                    let source_end = if end == item_count {
                        block.source_range().end
                    } else {
                        list.items()[end].source_range().start
                    };
                    // Identity follows rendered content and ordered numbering,
                    // not the row's current position in the document.
                    let mut hasher = DefaultHasher::new();
                    "list-chunk".hash(&mut hasher);
                    list.start()
                        .map(|number| number + start as u64)
                        .hash(&mut hasher);
                    list.items()[start..end].hash(&mut hasher);
                    (end == item_count).hash(&mut hasher);
                    let fingerprint = hasher.finish();
                    let occurrence = occurrences.entry(fingerprint).or_default();
                    let mut hasher = DefaultHasher::new();
                    (fingerprint, *occurrence).hash(&mut hasher);
                    *occurrence += 1;
                    rows.push(Row {
                        block_ix,
                        items: Some(start..end),
                        source_range: source_start..source_end,
                        fingerprint,
                        id: hasher.finish(),
                    });
                }
            } else {
                rows.push(Row {
                    block_ix,
                    items: None,
                    source_range: block.source_range(),
                    fingerprint: block.fingerprint(),
                    id: block.id().as_u64(),
                });
            }
        }
        Self {
            expanded: Some(rows),
        }
    }

    pub(crate) fn is_expanded(&self) -> bool {
        self.expanded.is_some()
    }

    pub(crate) fn len(&self, document: &MarkdownDocument) -> usize {
        self.expanded
            .as_ref()
            .map_or(document.blocks().len(), Vec::len)
    }

    pub(crate) fn block_ix(&self, row_ix: usize) -> usize {
        self.expanded
            .as_ref()
            .map_or(row_ix, |rows| rows[row_ix].block_ix)
    }

    pub(crate) fn list_items(&self, row_ix: usize) -> Option<Range<usize>> {
        self.expanded
            .as_ref()
            .and_then(|rows| rows[row_ix].items.clone())
    }

    pub(crate) fn id(&self, row_ix: usize, document: &MarkdownDocument) -> u64 {
        self.expanded.as_ref().map_or_else(
            || document.blocks()[row_ix].id().as_u64(),
            |rows| rows[row_ix].id,
        )
    }

    pub(crate) fn source_range(&self, row_ix: usize, document: &MarkdownDocument) -> Range<usize> {
        self.expanded.as_ref().map_or_else(
            || document.blocks()[row_ix].source_range(),
            |rows| rows[row_ix].source_range.clone(),
        )
    }

    pub(crate) fn row_for_offset(
        &self,
        offset: usize,
        document: &MarkdownDocument,
    ) -> Option<usize> {
        if let Some(rows) = &self.expanded {
            if rows.is_empty() {
                return None;
            }
            Some(
                rows.partition_point(|row| row.source_range.start <= offset)
                    .saturating_sub(1),
            )
        } else {
            document.block_ix_for_offset(offset)
        }
    }

    pub(crate) fn row_for_id(&self, id: u64, document: &MarkdownDocument) -> Option<usize> {
        (0..self.len(document)).find(|&ix| self.id(ix, document) == id)
    }

    fn fingerprint(&self, row_ix: usize, document: &MarkdownDocument) -> u64 {
        self.expanded.as_ref().map_or_else(
            || document.blocks()[row_ix].fingerprint(),
            |rows| rows[row_ix].fingerprint,
        )
    }

    /// The changed contiguous region, preserving measurements outside it even
    /// when a document gains or loses its first chunked list.
    pub(crate) fn changed_range(
        &self,
        document: &MarkdownDocument,
        next: &Self,
        next_document: &MarkdownDocument,
    ) -> Option<(Range<usize>, Range<usize>)> {
        let old_count = self.len(document);
        let new_count = next.len(next_document);
        let shared_count = old_count.min(new_count);
        let prefix = (0..shared_count)
            .take_while(|&ix| self.fingerprint(ix, document) == next.fingerprint(ix, next_document))
            .count();
        if prefix == old_count && prefix == new_count {
            return None;
        }
        let suffix = (0..shared_count - prefix)
            .take_while(|&ix| {
                self.fingerprint(old_count - ix - 1, document)
                    == next.fingerprint(new_count - ix - 1, next_document)
            })
            .count();
        Some((prefix..old_count - suffix, prefix..new_count - suffix))
    }
}
