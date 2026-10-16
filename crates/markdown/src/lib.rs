//! Markdown parsing for the preview, independent of any UI framework.
//!
//! A document is a flat list of top-level [`Block`]s. Each block carries a
//! content fingerprint, so two parses of an edited document can be compared
//! with [`BlockSplice::between`] and only the blocks that actually changed are
//! handed to the renderer again. Text is stored as `Arc<str>` so the renderer
//! can share it without copying.

mod document;
mod inline;
mod splice;

pub use document::{
    Block, BlockId, BlockKind, CodeBlock, ColumnAlignment, DocumentStats, Heading, Image, ListItem,
    MarkdownDocument, MarkdownList, Table,
};
pub use inline::{InlineLink, InlineRun, InlineStyle, InlineText};
pub use splice::BlockSplice;
