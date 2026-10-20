use std::ops::Range;
use std::sync::Arc;

/// A set of inline text styles. Styles combine, so a run may be both strong
/// and emphasized.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct InlineStyle(u8);

impl InlineStyle {
    pub const STRONG: Self = Self(1 << 0);
    pub const EMPHASIS: Self = Self(1 << 1);
    pub const STRIKETHROUGH: Self = Self(1 << 2);
    pub const CODE: Self = Self(1 << 3);
    pub const LINK: Self = Self(1 << 4);

    pub fn is_plain(self) -> bool {
        self.0 == 0
    }

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub(crate) fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub(crate) fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }
}

/// A styled byte range of an [`InlineText`]. Runs are sorted, never overlap,
/// and only cover text that has a style.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InlineRun {
    range: Range<usize>,
    style: InlineStyle,
}

impl InlineRun {
    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    pub fn style(&self) -> InlineStyle {
        self.style
    }
}

/// A link target over a byte range of an [`InlineText`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InlineLink {
    range: Range<usize>,
    url: Arc<str>,
}

impl InlineLink {
    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    pub fn url(&self) -> &Arc<str> {
        &self.url
    }
}

/// Inline content flattened to one string with styled runs and links.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct InlineText {
    text: Arc<str>,
    runs: Vec<InlineRun>,
    links: Vec<InlineLink>,
}

impl InlineText {
    pub fn text(&self) -> &Arc<str> {
        &self.text
    }

    pub fn runs(&self) -> &[InlineRun] {
        &self.runs
    }

    pub fn links(&self) -> &[InlineLink] {
        &self.links
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// Accumulates inline events into an [`InlineText`].
#[derive(Default)]
pub(crate) struct InlineBuilder {
    text: String,
    runs: Vec<InlineRun>,
    links: Vec<InlineLink>,
    style: InlineStyle,
    /// Start offset and target of each link currently open.
    open_links: Vec<(usize, Arc<str>)>,
    /// Images seen, with the byte range their alt text occupies.
    images: Vec<(Range<usize>, Arc<str>)>,
    open_images: Vec<(usize, Arc<str>)>,
}

impl InlineBuilder {
    pub(crate) fn push_text(&mut self, text: &str) {
        self.push_styled(text, self.style);
    }

    pub(crate) fn push_code(&mut self, code: &str) {
        let mut style = self.style;
        style.insert(InlineStyle::CODE);
        self.push_styled(code, style);
    }

    pub(crate) fn push_hard_break(&mut self) {
        self.push_text("\n");
    }

    pub(crate) fn push_style(&mut self, style: InlineStyle) {
        self.style.insert(style);
    }

    pub(crate) fn pop_style(&mut self, style: InlineStyle) {
        self.style.remove(style);
    }

    pub(crate) fn start_link(&mut self, url: &str) {
        self.open_links.push((self.text.len(), url.into()));
        self.style.insert(InlineStyle::LINK);
    }

    pub(crate) fn end_link(&mut self) {
        if let Some((start, url)) = self.open_links.pop() {
            let range = start..self.text.len();
            if !range.is_empty() {
                self.links.push(InlineLink { range, url });
            }
        }
        if self.open_links.is_empty() {
            self.style.remove(InlineStyle::LINK);
        }
    }

    pub(crate) fn start_image(&mut self, url: &str) {
        self.open_images.push((self.text.len(), url.into()));
        self.style.insert(InlineStyle::EMPHASIS);
    }

    pub(crate) fn end_image(&mut self) {
        if let Some((start, url)) = self.open_images.pop() {
            self.images.push((start..self.text.len(), url));
        }
        if self.open_images.is_empty() {
            self.style.remove(InlineStyle::EMPHASIS);
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.text.is_empty() && self.images.is_empty()
    }

    /// The single image this content consists of, apart from whitespace.
    pub(crate) fn sole_image(&self) -> Option<(Arc<str>, Arc<str>)> {
        let [(range, url)] = self.images.as_slice() else {
            return None;
        };
        let outside = self.text[..range.start]
            .chars()
            .chain(self.text[range.end..].chars());
        if outside.clone().any(|c| !c.is_whitespace()) {
            return None;
        }
        Some((url.clone(), self.text[range.clone()].into()))
    }

    pub(crate) fn finish(self) -> InlineText {
        InlineText {
            text: self.text.into(),
            runs: self.runs,
            links: self.links,
        }
    }

    fn push_styled(&mut self, text: &str, style: InlineStyle) {
        if text.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(text);
        if style.is_plain() {
            return;
        }
        match self.runs.last_mut() {
            Some(last) if last.style == style && last.range.end == start => {
                last.range.end = self.text.len();
            }
            _ => self.runs.push(InlineRun {
                range: start..self.text.len(),
                style,
            }),
        }
    }
}
