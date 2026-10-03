//! Borrowed rich-text spans and temporary IME weight projection.

use super::geometry::boundary;
use crate::model::{BoldRange, bold};
use cosmic_text::{Attrs, Buffer, Shaping, Weight};

/// Preserve the unstyled fast path; styled spans borrow the row text instead of
/// constructing copied substrings or a full-document rich-text projection.
pub(super) fn set_body_text(
    buffer: &mut Buffer,
    text: &str,
    bold: &[BoldRange],
    attrs: &Attrs<'_>,
) {
    if bold.is_empty() {
        buffer.set_text(text, attrs, Shaping::Advanced, None);
        return;
    }
    let mut index = 0;
    let mut offset = 0;
    let spans = std::iter::from_fn(|| {
        while offset < text.len() {
            let Some(range) = bold.get(index) else {
                let tail = &text[offset..];
                offset = text.len();
                return Some((tail, attrs.clone()));
            };
            // Document ranges are validated UTF-8 boundaries. Keep display
            // slicing safe even if an inconsistent range reaches this layer.
            let start = boundary(text, range.start);
            let end = boundary(text, range.end);
            if end <= offset || end <= start {
                index += 1;
                continue;
            }
            if start > offset {
                let plain = &text[offset..start];
                offset = start;
                return Some((plain, attrs.clone()));
            }
            let styled = &text[offset..end];
            offset = end;
            index += 1;
            return Some((styled, attrs.clone().weight(Weight::BOLD)));
        }
        None
    });
    buffer.set_rich_text(spans, attrs, Shaping::Advanced, None);
}

/// Insert a presentation-only range at `at`, splitting an existing bold span
/// when composition is regular and shifting the original suffix by UTF-8 bytes.
pub(super) fn preedit_bold(
    source: &[BoldRange],
    text: &str,
    at: usize,
    length: usize,
    typing_bold: bool,
) -> Vec<BoldRange> {
    if source.is_empty() && !typing_bold {
        return Vec::new();
    }
    let mut result = Vec::with_capacity(source.len().saturating_add(2));
    for range in source.iter().take_while(|range| range.start < at) {
        push_merged(&mut result, range.start, range.end.min(at));
    }
    if typing_bold {
        push_merged(&mut result, at, at.saturating_add(length));
    }
    for range in source.iter().filter(|range| range.end > at) {
        push_merged(
            &mut result,
            range.start.max(at).saturating_add(length),
            range.end.saturating_add(length),
        );
    }
    bold::finish_insertion(text, &mut result, at, at + length, typing_bold);
    result
}

fn push_merged(ranges: &mut Vec<BoldRange>, start: usize, end: usize) {
    if start >= end {
        return;
    }
    if let Some(last) = ranges.last_mut()
        && last.end >= start
    {
        last.end = last.end.max(end);
    } else {
        ranges.push(BoldRange { start, end });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic_text::Metrics;

    #[test]
    fn preedit_projection_splits_regular_and_merges_bold_using_utf8_bytes() {
        let source = [BoldRange { start: 0, end: 4 }];
        let length = "中文".len();
        assert_eq!(
            preedit_bold(&source, "ab中文cd", 2, length, false),
            [
                BoldRange { start: 0, end: 2 },
                BoldRange { start: 8, end: 10 }
            ]
        );
        assert_eq!(
            preedit_bold(&source, "ab中文cd", 2, length, true),
            [BoldRange { start: 0, end: 10 }]
        );
        assert_eq!(
            preedit_bold(&source, "abcd中文", 4, length, true),
            [BoldRange { start: 0, end: 10 }]
        );
        assert_eq!(source, [BoldRange { start: 0, end: 4 }]);

        // Preedit and a committed combining character must style the same
        // entire grapheme, without changing the source ranges.
        let source = [BoldRange { start: 0, end: 1 }];
        assert!(preedit_bold(&source, "e\u{301}", 1, 2, false).is_empty());
        assert_eq!(
            preedit_bold(&[], "e\u{301}", 1, 2, true),
            [BoldRange { start: 0, end: 3 }]
        );
        assert_eq!(source, [BoldRange { start: 0, end: 1 }]);
    }

    #[test]
    fn body_weight_changes_without_changing_text_or_loading_fonts() {
        let mut buffer = Buffer::new_empty(Metrics::new(16.0, 24.0));
        let attrs = Attrs::new();
        let text = "a中文z";
        set_body_text(&mut buffer, text, &[], &attrs);
        assert_eq!(
            buffer.lines[0].attrs_list().get_span(1).weight,
            Weight::NORMAL
        );

        set_body_text(&mut buffer, text, &[BoldRange { start: 1, end: 7 }], &attrs);
        assert_eq!(buffer.lines[0].text(), text);
        let spans = buffer.lines[0].attrs_list();
        assert_eq!(spans.get_span(0).weight, Weight::NORMAL);
        assert_eq!(spans.get_span(1).weight, Weight::BOLD);
        assert_eq!(spans.get_span(4).weight, Weight::BOLD);
        assert_eq!(spans.get_span(7).weight, Weight::NORMAL);

        set_body_text(&mut buffer, text, &[], &attrs);
        assert_eq!(buffer.lines[0].text(), text);
        assert_eq!(
            buffer.lines[0].attrs_list().get_span(1).weight,
            Weight::NORMAL
        );
    }
}
