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
    /// `==marked==` text, as in Obsidian and Typora.
    pub const HIGHLIGHT: Self = Self(1 << 5);

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

    /// The part of this text in `range`, with the runs and links over it.
    pub(crate) fn slice(&self, range: Range<usize>) -> InlineText {
        let clip = |other: Range<usize>| {
            let start = other.start.max(range.start);
            let end = other.end.min(range.end);
            (start < end).then(|| start - range.start..end - range.start)
        };
        InlineText {
            text: self.text[range.clone()].into(),
            runs: self
                .runs
                .iter()
                .filter_map(|run| {
                    Some(InlineRun {
                        range: clip(run.range())?,
                        style: run.style,
                    })
                })
                .collect(),
            links: self
                .links
                .iter()
                .filter_map(|link| {
                    Some(InlineLink {
                        range: clip(link.range())?,
                        url: link.url.clone(),
                    })
                })
                .collect(),
        }
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
        let markers = highlight_markers(&self.text, &self.runs);
        if markers.is_empty() {
            return InlineText {
                text: self.text.into(),
                runs: self.runs,
                links: self.links,
            };
        }
        apply_highlights(&self.text, &self.runs, &self.links, &markers)
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

/// Where `==` opens and closes a highlight, as pairs of marker offsets.
///
/// Like strikethrough, an opening marker is followed by text and a closing
/// one follows text, so `a == b` stays as written. A highlight ends with its
/// line, and markers inside code are text.
fn highlight_markers(text: &str, runs: &[InlineRun]) -> Vec<(usize, usize)> {
    let in_code = |ix: usize| {
        runs.iter()
            .any(|run| run.style.contains(InlineStyle::CODE) && run.range.contains(&ix))
    };
    let bytes = text.as_bytes();
    let mut pairs = Vec::new();
    let mut open = None;
    let mut ix = 0;
    while ix < bytes.len() {
        if bytes[ix] == b'\n' {
            open = None;
            ix += 1;
            continue;
        }
        let is_marker = bytes[ix..].starts_with(b"==")
            && bytes.get(ix + 2) != Some(&b'=')
            && (ix == 0 || bytes[ix - 1] != b'=')
            && !in_code(ix);
        if !is_marker {
            ix += 1;
            continue;
        }
        let before = text[..ix].chars().next_back();
        let after = text[ix + 2..].chars().next();
        match open {
            Some(start) if ix > start + 2 && before.is_some_and(|c| !c.is_whitespace()) => {
                pairs.push((start, ix));
                open = None;
            }
            _ if after.is_some_and(|c| !c.is_whitespace()) => open = Some(ix),
            _ => {}
        }
        ix += 2;
    }
    pairs
}

/// `text` without the highlight markers, its runs and links moved to match,
/// and the text between each pair of markers highlighted.
fn apply_highlights(
    text: &str,
    runs: &[InlineRun],
    links: &[InlineLink],
    pairs: &[(usize, usize)],
) -> InlineText {
    let markers: Vec<usize> = pairs
        .iter()
        .flat_map(|&(open, close)| [open, close])
        .collect();
    // Each marker before an offset moves it back by the marker's length.
    let map = |offset: usize| offset - 2 * markers.partition_point(|&marker| marker < offset);

    let mut stripped = String::with_capacity(text.len());
    let mut copied = 0;
    for &marker in &markers {
        stripped.push_str(&text[copied..marker]);
        copied = marker + 2;
    }
    stripped.push_str(&text[copied..]);

    let mut styles = vec![InlineStyle::default(); stripped.len()];
    for run in runs {
        for style in &mut styles[map(run.range.start)..map(run.range.end)] {
            style.insert(run.style);
        }
    }
    for &(open, close) in pairs {
        for style in &mut styles[map(open + 2)..map(close)] {
            style.insert(InlineStyle::HIGHLIGHT);
        }
    }
    let mut runs: Vec<InlineRun> = Vec::new();
    for (ix, &style) in styles.iter().enumerate() {
        if style.is_plain() {
            continue;
        }
        match runs.last_mut() {
            Some(last) if last.style == style && last.range.end == ix => last.range.end = ix + 1,
            _ => runs.push(InlineRun {
                range: ix..ix + 1,
                style,
            }),
        }
    }
    let links = links
        .iter()
        .map(|link| InlineLink {
            range: map(link.range.start)..map(link.range.end),
            url: link.url.clone(),
        })
        .filter(|link| !link.range.is_empty())
        .collect();
    InlineText {
        text: stripped.into(),
        runs,
        links,
    }
}
