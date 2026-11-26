//! WhatsApp message formatting: `*bold*`, `_italic_`, `~strikethrough~`,
//! ```` ```monospace``` ````, `` `inline code` `` and `> ` quote lines.
//! Markers are removed from the shown text, as WhatsApp does.
use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Style {
    Bold,
    Italic,
    Strike,
    Mono,
    Code,
    Quote,
}

pub(super) struct Formatted {
    pub text: String,
    /// Styled byte ranges of `text`.
    pub styles: Vec<(Range<usize>, Style)>,
    /// For every byte offset of the source (and its end), the offset in `text`.
    map: Vec<usize>,
}

impl Formatted {
    pub fn map(&self, range: &Range<usize>) -> Range<usize> {
        let at = |i: usize| self.map[i.min(self.map.len() - 1)];
        at(range.start)..at(range.end)
    }
}

/// Format `source`, leaving `protected` byte ranges (links, mentions) untouched.
pub(super) fn format(source: &str, protected: &[Range<usize>]) -> Formatted {
    let (drop, styles) = analyze(source, protected);
    let chars: Vec<(usize, char)> = source.char_indices().collect();
    let mut text = String::with_capacity(source.len());
    let mut map = vec![0; source.len() + 1];
    for (i, c) in &chars {
        for slot in &mut map[*i..*i + c.len_utf8()] {
            *slot = text.len();
        }
        if !drop[*i] {
            text.push(*c);
        }
    }
    map[source.len()] = text.len();
    let styles = styles
        .into_iter()
        .map(|(range, style)| (map[range.start]..map[range.end], style))
        .filter(|(range, _)| range.start < range.end)
        .collect();
    Formatted { text, styles, map }
}

/// The styles and markers of `source` in its own byte positions, for an editor
/// that keeps the markers visible: (marker ranges, styled content ranges).
pub(super) fn in_place(
    source: &str,
    protected: &[Range<usize>],
) -> (Vec<Range<usize>>, Vec<(Range<usize>, Style)>) {
    let (drop, styles) = analyze(source, protected);
    let mut markers: Vec<Range<usize>> = Vec::new();
    for (i, dropped) in drop.iter().enumerate() {
        if *dropped {
            match markers.last_mut() {
                Some(last) if last.end == i => last.end = i + 1,
                _ => markers.push(i..i + 1),
            }
        }
    }
    (markers, styles)
}

/// Which bytes are formatting markers, and the styled ranges between them.
fn analyze(source: &str, protected: &[Range<usize>]) -> (Vec<bool>, Vec<(Range<usize>, Style)>) {
    let chars: Vec<(usize, char)> = source.char_indices().collect();
    let mut drop = vec![false; source.len()];
    // Characters no marker may pair across or inside.
    let mut blocked: Vec<bool> = chars
        .iter()
        .map(|(i, _)| protected.iter().any(|r| r.contains(i)))
        .collect();
    let mut styles: Vec<(Range<usize>, Style)> = Vec::new();

    // ```monospace``` may span lines; nothing inside it is formatted.
    let mut search = 0;
    while let Some(open) = source[search..].find("```").map(|i| i + search) {
        let Some(close) = source[open + 3..].find("```").map(|i| i + open + 3) else {
            break;
        };
        let inner = &source[open + 3..close];
        let free = |range: Range<usize>| {
            !chars
                .iter()
                .zip(&blocked)
                .any(|((i, _), b)| *b && range.contains(i))
        };
        if inner.trim().is_empty() || !free(open..close + 3) {
            search = open + 1;
            continue;
        }
        for i in (open..open + 3).chain(close..close + 3) {
            drop[i] = true;
        }
        styles.push((open + 3..close, Style::Mono));
        for ((i, _), b) in chars.iter().zip(blocked.iter_mut()) {
            if (open..close + 3).contains(i) {
                *b = true;
            }
        }
        search = close + 3;
    }

    // "> " at the start of a line quotes it.
    let mut line_start = 0;
    for line in source.split('\n') {
        let start = line_start;
        line_start += line.len() + 1;
        let index = chars.partition_point(|(i, _)| *i < start);
        if line.starts_with("> ") && line.len() > 2 && !blocked.get(index).copied().unwrap_or(true)
        {
            drop[start] = true;
            drop[start + 1] = true;
            styles.push((start + 2..start + line.len(), Style::Quote));
        }
    }

    inline(&chars, &blocked, 0, chars.len(), &mut drop, &mut styles);
    (drop, styles)
}

/// The shown text alone, for one-line previews.
pub(super) fn plain(source: &str) -> String {
    let links: Vec<_> = crate::links::detect(source)
        .into_iter()
        .map(|(range, _)| range)
        .collect();
    format(source, &links).text
}

fn marker(c: char) -> Option<Style> {
    Some(match c {
        '*' => Style::Bold,
        '_' => Style::Italic,
        '~' => Style::Strike,
        '`' => Style::Code,
        _ => return None,
    })
}

/// Pairs single-character markers within `lo..hi`. A marker opens after a
/// non-word character and before non-space text; it closes after non-space
/// text and before a non-word character, on the same line.
fn inline(
    chars: &[(usize, char)],
    blocked: &[bool],
    lo: usize,
    hi: usize,
    drop: &mut [bool],
    styles: &mut Vec<(Range<usize>, Style)>,
) {
    let word = |i: usize| chars.get(i).is_some_and(|(_, c)| c.is_alphanumeric());
    let space = |i: usize| chars.get(i).is_none_or(|(_, c)| c.is_whitespace());
    let mut i = lo;
    while i < hi {
        let (at, c) = chars[i];
        let Some(style) = marker(c).filter(|_| !blocked[i]) else {
            i += 1;
            continue;
        };
        let opens = (i == 0 || !word(i - 1)) && i + 1 < hi && !space(i + 1) && chars[i + 1].1 != c;
        let mut close = None;
        if opens {
            for j in i + 2..hi {
                let (_, d) = chars[j];
                if d == '\n' || blocked[j] {
                    break;
                }
                if d == c && !space(j - 1) && chars[j - 1].1 != c && !word(j + 1) {
                    close = Some(j);
                    break;
                }
            }
        }
        let Some(j) = close else {
            i += 1;
            continue;
        };
        drop[at] = true;
        drop[chars[j].0] = true;
        styles.push((chars[i + 1].0..chars[j].0, style));
        if style != Style::Code {
            inline(chars, blocked, i + 1, j, drop, styles);
        }
        i = j + 1;
    }
}
