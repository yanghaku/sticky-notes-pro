//! UTF-8 byte ranges aligned to whole graphemes; adjacent spans are merged.

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use unicode_segmentation::GraphemeCursor;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoldRange {
    pub start: usize,
    pub end: usize,
}

pub(crate) fn validate(text: &str, ranges: &[BoldRange]) -> Result<()> {
    let mut previous_end = 0;
    for range in ranges {
        ensure!(
            range.start < range.end && range.end <= text.len(),
            "invalid bold range"
        );
        ensure!(
            text.is_char_boundary(range.start) && text.is_char_boundary(range.end),
            "bold range splits UTF-8 character"
        );
        ensure!(
            range.start >= previous_end,
            "bold ranges overlap or are unsorted"
        );
        previous_end = range.end;
    }
    Ok(())
}

pub(crate) fn push(ranges: &mut Vec<BoldRange>, range: BoldRange) {
    if range.start >= range.end {
        return;
    }
    if let Some(last) = ranges.last_mut().filter(|last| last.end >= range.start) {
        last.end = last.end.max(range.end);
    } else {
        ranges.push(range);
    }
}

pub(crate) fn clipped(ranges: &[BoldRange], from: usize, to: usize) -> Vec<BoldRange> {
    let mut result = Vec::new();
    for range in ranges {
        let start = range.start.max(from);
        let end = range.end.min(to);
        if start < end {
            push(
                &mut result,
                BoldRange {
                    start: start - from,
                    end: end - from,
                },
            );
        }
    }
    result
}

pub(crate) fn append(target: &mut Vec<BoldRange>, ranges: &[BoldRange], offset: usize) {
    for range in ranges {
        push(
            target,
            BoldRange {
                start: range.start + offset,
                end: range.end + offset,
            },
        );
    }
}

/// Preserve existing bold when editing joins characters into a single grapheme.
/// Check only range endpoints, without scanning/copying the whole row's text.
pub(crate) fn normalize(text: &str, ranges: &mut Vec<BoldRange>) {
    let mut written = 0;
    for index in 0..ranges.len() {
        let range = ranges[index];
        let range = BoldRange {
            start: grapheme_edge(text, range.start, false),
            end: grapheme_edge(text, range.end, true),
        };
        if written > 0 && ranges[written - 1].end >= range.start {
            ranges[written - 1].end = ranges[written - 1].end.max(range.end);
        } else {
            ranges[written] = range;
            written += 1;
        }
    }
    ranges.truncate(written);
}

/// The chosen insertion weight wins for a grapheme formed across either edge
/// of the inserted text, including turning a formerly bold base back to normal.
pub(crate) fn finish_insertion(
    text: &str,
    ranges: &mut Vec<BoldRange>,
    from: usize,
    to: usize,
    enabled: bool,
) {
    normalize(text, ranges);
    if from < to {
        let start = grapheme_edge(text, from, false);
        let end = grapheme_edge(text, to, true);
        if start != from || end != to {
            set(ranges, start, end, enabled);
        }
    }
}

fn grapheme_edge(text: &str, offset: usize, forward: bool) -> usize {
    let mut cursor = GraphemeCursor::new(offset, text.len(), true);
    // All callers supply valid UTF-8 offsets and the complete text, so the
    // cursor cannot request a missing chunk or additional preceding context.
    if cursor.is_boundary(text, 0).expect("complete grapheme text") {
        return offset;
    }
    if forward {
        cursor
            .next_boundary(text, 0)
            .expect("complete grapheme text")
            .unwrap_or(text.len())
    } else {
        cursor
            .prev_boundary(text, 0)
            .expect("complete grapheme text")
            .unwrap_or(0)
    }
}

/// Replace a text interval and apply the chosen style to inserted bytes. Work
/// is limited to the edited row's ranges, never the rest of the document.
pub(crate) fn edit(
    ranges: &mut Vec<BoldRange>,
    from: usize,
    to: usize,
    inserted: usize,
    bold: bool,
) {
    if ranges.is_empty() {
        if bold && inserted != 0 {
            ranges.push(BoldRange {
                start: from,
                end: from + inserted,
            });
        }
        return;
    }
    if from == to && !bold && ranges.last().is_some_and(|last| last.end <= from) {
        return;
    }
    // Appending to a bold run is the common continuous-typing path.
    if from == to
        && bold
        && let Some(last) = ranges.last_mut().filter(|last| last.end == from)
    {
        last.end += inserted;
        return;
    }
    let old = std::mem::take(ranges);
    for range in &old {
        if range.start < from {
            push(
                ranges,
                BoldRange {
                    start: range.start,
                    end: range.end.min(from),
                },
            );
        }
    }
    if bold {
        push(
            ranges,
            BoldRange {
                start: from,
                end: from + inserted,
            },
        );
    }
    for range in old {
        if range.end > to {
            push(
                ranges,
                BoldRange {
                    start: range.start.max(to) - to + from + inserted,
                    end: range.end - to + from + inserted,
                },
            );
        }
    }
}

pub(crate) fn set(ranges: &mut Vec<BoldRange>, from: usize, to: usize, enabled: bool) {
    edit(ranges, from, to, to - from, enabled);
}

pub(crate) fn covers(ranges: &[BoldRange], from: usize, to: usize) -> bool {
    if from == to {
        return true;
    }
    let mut covered = from;
    for range in ranges {
        if range.start > covered {
            return false;
        }
        if range.end > covered {
            covered = range.end;
        }
        if covered >= to {
            return true;
        }
    }
    false
}
