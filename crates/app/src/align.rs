//! Correspondence between the source text and its translation, for synchronized scrolling.
//!
//! Long texts are translated chunk by chunk, so the start of every chunk is a known pair of
//! positions. Within a chunk, paragraphs are paired when the source and the translation have the
//! same number of them, which is the usual case; otherwise the chunk is one segment. Positions
//! inside a segment map proportionally.

use std::ops::Range;

/// A translated chunk: its byte range in the source and where its translation starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkSpan {
    pub source: Range<usize>,
    pub output_start: usize,
}

/// One of the two panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Source,
    Output,
}

impl Side {
    pub fn other(self) -> Self {
        match self {
            Self::Source => Self::Output,
            Self::Output => Self::Source,
        }
    }
}

/// Corresponding byte ranges of the source and the translation, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Alignment {
    segments: Vec<(Range<usize>, Range<usize>)>,
}

impl Alignment {
    /// Aligns `output`, the translation of `source` so far, using the chunks it was made of.
    pub fn new(source: &str, output: &str, chunks: &[ChunkSpan]) -> Self {
        let mut segments = Vec::new();
        for (i, chunk) in chunks.iter().enumerate() {
            let output_end = chunks
                .get(i + 1)
                .map_or(output.len(), |next| next.output_start)
                .min(output.len());
            let source_range =
                chunk.source.start.min(source.len())..chunk.source.end.min(source.len());
            let output_range = chunk.output_start.min(output_end)..output_end;
            let source_paragraphs = paragraphs(source, source_range.clone());
            let output_paragraphs = paragraphs(output, output_range.clone());
            if source_paragraphs.len() > 1 && source_paragraphs.len() == output_paragraphs.len() {
                segments.extend(source_paragraphs.into_iter().zip(output_paragraphs));
            } else if !output_range.is_empty() {
                segments.push((source_range, trim(output, output_range)));
            }
        }
        Self { segments }
    }

    fn range(segment: &(Range<usize>, Range<usize>), side: Side) -> &Range<usize> {
        match side {
            Side::Source => &segment.0,
            Side::Output => &segment.1,
        }
    }

    /// The segment containing `offset` on `side`, or the last one before it.
    fn segment_index(&self, side: Side, offset: usize) -> Option<usize> {
        let after = self
            .segments
            .partition_point(|s| Self::range(s, side).start <= offset);
        after.checked_sub(1)
    }

    /// The position on the other side corresponding to `offset` on `side`, as a byte offset
    /// into `other` (the other side's text), on a character boundary.
    pub fn map(&self, side: Side, offset: usize, other: &str) -> Option<usize> {
        let index = self.segment_index(side, offset).or_else(|| {
            // Before the first segment: its start.
            (!self.segments.is_empty()).then_some(0)
        })?;
        let segment = &self.segments[index];
        let from = Self::range(segment, side);
        let to = Self::range(segment, side.other());
        let fraction = if from.is_empty() {
            0.0
        } else {
            (offset.saturating_sub(from.start) as f64 / from.len() as f64).clamp(0.0, 1.0)
        };
        let target = to.start + (to.len() as f64 * fraction) as usize;
        Some(floor_char_boundary(other, target.min(other.len())))
    }

    /// The range on the other side that corresponds to the segment containing `offset`.
    pub fn counterpart(&self, side: Side, offset: usize) -> Option<Range<usize>> {
        let index = self.segment_index(side, offset)?;
        Some(Self::range(&self.segments[index], side.other()).clone())
    }
}

/// Paragraph ranges in `text[range]`, without surrounding whitespace: separated by blank lines,
/// or by line breaks in text that has no blank lines.
fn paragraphs(text: &str, range: Range<usize>) -> Vec<Range<usize>> {
    let slice = &text[range.clone()];
    let split = |separator: &dyn Fn(&str) -> Vec<Range<usize>>| {
        separator(slice)
            .into_iter()
            .map(|r| trim(text, range.start + r.start..range.start + r.end))
            .filter(|r| !r.is_empty())
            .collect::<Vec<_>>()
    };
    let by_blank_lines = split(&|s| split_at_blank_lines(s));
    if by_blank_lines.len() > 1 {
        return by_blank_lines;
    }
    split(&|s| {
        let mut out = Vec::new();
        let mut start = 0;
        for (i, _) in s.match_indices('\n') {
            out.push(start..i);
            start = i + 1;
        }
        out.push(start..s.len());
        out
    })
}

/// Ranges of `s` separated by blank (whitespace-only) lines.
fn split_at_blank_lines(s: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut paragraph: Option<Range<usize>> = None;
    let mut line_start = 0;
    for line in s.split_inclusive('\n') {
        let line_end = line_start + line.len();
        if line.trim().is_empty() {
            out.extend(paragraph.take());
        } else {
            paragraph.get_or_insert(line_start..line_end).end = line_end;
        }
        line_start = line_end;
    }
    out.extend(paragraph);
    out
}

/// `range` without leading and trailing whitespace.
fn trim(text: &str, range: Range<usize>) -> Range<usize> {
    let slice = &text[range.clone()];
    let start = range.start + (slice.len() - slice.trim_start().len());
    let end = range.start + slice.trim_end().len();
    start..end.max(start)
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_paragraphs_within_chunks() {
        let source = "One.\n\nTwo two.\n\nThree.";
        let output = "一。\n\n二二。\n\n三。";
        let chunks = [ChunkSpan {
            source: 0..source.len(),
            output_start: 0,
        }];
        let a = Alignment::new(source, output, &chunks);
        assert_eq!(a.segments.len(), 3);
        // "Two" maps to "二".
        let two = source.find("Two").unwrap();
        assert_eq!(a.map(Side::Source, two, output), output.find("二"));
        assert_eq!(
            a.counterpart(Side::Output, output.find("三").unwrap()),
            Some(source.find("Three").unwrap()..source.len())
        );
    }

    #[test]
    fn falls_back_to_chunks_and_proportions() {
        let source = "Line one.\nLine two.\n\nNext chunk.";
        // The translation merged the two lines of the first chunk.
        let output = "一行目。二行目。\n\n次。";
        let chunks = [
            ChunkSpan {
                source: 0..19,
                output_start: 0,
            },
            ChunkSpan {
                source: 21..source.len(),
                output_start: output.find("次").unwrap(),
            },
        ];
        let a = Alignment::new(source, output, &chunks);
        assert_eq!(a.segments.len(), 2);
        // The middle of the first chunk maps to the middle of its translation, on a character
        // boundary.
        let mapped = a.map(Side::Source, 10, output).unwrap();
        assert!(output.is_char_boundary(mapped));
        assert!(mapped > 0 && mapped < output.find("次").unwrap());
        assert_eq!(
            a.map(Side::Output, output.find("次").unwrap(), source),
            Some(21)
        );
    }

    #[test]
    fn single_newline_paragraphs_pair_up() {
        let source = "一行目。\n二行目。\n三行目。";
        let output = "Line one.\nLine two.\nLine three.";
        let chunks = [ChunkSpan {
            source: 0..source.len(),
            output_start: 0,
        }];
        let a = Alignment::new(source, output, &chunks);
        assert_eq!(a.segments.len(), 3);
        let third = source.find("三").unwrap();
        assert_eq!(
            a.map(Side::Source, third, output),
            output.find("Line three")
        );
    }

    #[test]
    fn splits_at_blank_lines() {
        assert_eq!(split_at_blank_lines("a\n\nb"), [0..2, 3..4]);
        assert_eq!(split_at_blank_lines("a\n \n\nb\nc"), [0..2, 5..8]);
        assert_eq!(split_at_blank_lines("abc"), vec![(0..3)]);
    }

    #[test]
    fn empty_alignment_maps_nothing() {
        let a = Alignment::default();
        assert_eq!(a.map(Side::Source, 3, "abc"), None);
        assert_eq!(a.counterpart(Side::Source, 3), None);
    }
}
