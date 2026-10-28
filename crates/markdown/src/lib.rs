//! Markdown parsing for the preview, independent of any UI framework.
//!
//! A document is a flat list of top-level [`Block`]s. Each block carries a
//! content fingerprint, so two parses of an edited document can be compared
//! with [`BlockSplice::between`] and only the blocks that actually changed are
//! handed to the renderer again. Text is stored as `Arc<str>` so the renderer
//! can share it without copying.

mod document;
mod events;
mod inline;
mod splice;
mod word_count;

pub use document::{
    Block, BlockId, BlockKind, Callout, CalloutTone, CodeBlock, ColumnAlignment, DocumentStats,
    Heading, Image, ListItem, MarkdownDocument, MarkdownList, PARSER_OPTIONS, Property, Table,
    callout_header,
};
pub use events::{MarkdownEvent, events};
pub use inline::{InlineLink, InlineRun, InlineStyle, InlineText};
pub use splice::BlockSplice;
pub use word_count::{WordCounter, count_words};
