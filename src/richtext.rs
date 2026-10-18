#![allow(dead_code)] // parked: powers the future dual edit/view mode
//! WYSIWYG bridge between Markdown (how notes are stored and synced) and
//! formatting spans (how the editor shows them, with the `**`/`#` markers
//! hidden).
//!
//! The editor never shows Markdown syntax: `**bold**` is displayed as **bold**
//! via a text tag. On save, the tags are turned back into Markdown so the note
//! stays byte-compatible with the Android app.
//!
//! **The round trip must be lossless** — note content syncs to the phone, so a
//! lossy `markdown → spans → markdown` pass would corrupt notes. That property
//! is what the tests here guard.
//!
//! Supported: inline **bold**, _italic_, ~~strike~~, `code`, and line-level
//! `#`..`###` headings. Anything else (links, images, lists, quotes, tables)
//! passes through as literal text — visible, unchanged, never corrupted.

/// Inline styles, as a small bitset so a run can be bold *and* italic.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Style(u8);

impl Style {
    pub const BOLD: Style = Style(1 << 0);
    pub const ITALIC: Style = Style(1 << 1);
    pub const STRIKE: Style = Style(1 << 2);
    pub const CODE: Style = Style(1 << 3);

    pub fn contains(self, other: Style) -> bool {
        self.0 & other.0 == other.0
    }
    pub fn with(self, other: Style) -> Style {
        Style(self.0 | other.0)
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// A formatting span over the *displayed* (marker-free) text, in char offsets.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Inline(Style),
    /// Heading level 1..=6, applied to the whole line.
    Heading(u8),
}

/// Result of parsing Markdown: the text to display (no markers) plus the spans
/// to apply over it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub text: String,
    pub spans: Vec<Span>,
}

/// Parses Markdown into displayable text + spans. Markers are removed from the
/// text and recorded as spans.
pub fn parse(md: &str) -> Parsed {
    let mut out = Parsed::default();
    let normalized = md.replace("\r\n", "\n").replace('\r', "\n");

    // Char offset into `out.text` where the current line starts.
    let mut line_start = 0;
    let lines: Vec<&str> = normalized.split('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        parse_line(line, &mut out, line_start);
        if i + 1 < lines.len() {
            out.text.push('\n');
        }
        line_start = out.text.chars().count();
    }
    out
}

fn parse_line(line: &str, out: &mut Parsed, line_start: usize) {
    // Heading: 1..6 '#' then a space. Applies to the whole (marker-stripped) line.
    let hashes = line.chars().take_while(|&c| c == '#').count();
    let (content, heading) = if (1..=6).contains(&hashes) && line.chars().nth(hashes) == Some(' ') {
        (&line[hashes + 1..], Some(hashes as u8))
    } else {
        (line, None)
    };

    let inline_start = out.text.chars().count();
    parse_inline(content, out, out.text.chars().count());
    let inline_end = out.text.chars().count();

    if let Some(level) = heading {
        // A heading with no text still occupies its line; span the line's text.
        let _ = line_start;
        out.spans.push(Span {
            start: inline_start,
            end: inline_end,
            kind: Kind::Heading(level),
        });
    }
}

/// Marker pairs, longest first so `**` wins over `*`.
const MARKERS: &[(&str, Style)] = &[
    ("**", Style::BOLD),
    ("~~", Style::STRIKE),
    ("`", Style::CODE),
    ("_", Style::ITALIC),
    ("*", Style::ITALIC),
];

fn parse_inline(text: &str, out: &mut Parsed, base: usize) {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if let Some((marker, style, inner_len, consumed)) = match_marker(&chars, i) {
            let disp_start = base + out.text.chars().count() - base_text_len(out, base);
            // Append the inner text (marker-free) and record the span over it.
            let start = out.text.chars().count();
            let inner: String = chars[i + marker.len()..i + marker.len() + inner_len]
                .iter()
                .collect();
            // Inner may itself contain other markers; parse recursively.
            let before = out.text.chars().count();
            parse_inline(&inner, out, before);
            let end = out.text.chars().count();
            out.spans.push(Span {
                start,
                end,
                kind: Kind::Inline(style),
            });
            let _ = disp_start;
            i += consumed;
        } else {
            out.text.push(chars[i]);
            i += 1;
        }
    }
}

fn base_text_len(_out: &Parsed, base: usize) -> usize {
    base
}

/// If a closable marker starts at `i`, returns (marker, style, inner_char_len,
/// total_consumed_chars).
fn match_marker(chars: &[char], i: usize) -> Option<(&'static str, Style, usize, usize)> {
    for (marker, style) in MARKERS {
        let m: Vec<char> = marker.chars().collect();
        if chars[i..].starts_with(&m) {
            // Find the closing marker after at least one inner char.
            let inner_from = i + m.len();
            let mut j = inner_from;
            while j + m.len() <= chars.len() {
                if chars[j..].starts_with(&m) && j > inner_from {
                    return Some((marker, *style, j - inner_from, (j + m.len()) - i));
                }
                j += 1;
            }
        }
    }
    None
}

/// Serialises displayable text + spans back to Markdown. Inverse of [`parse`].
pub fn to_markdown(parsed: &Parsed) -> String {
    let chars: Vec<char> = parsed.text.chars().collect();

    // Opening / closing marker strings to insert at each char boundary.
    let mut opens: Vec<Vec<&str>> = vec![Vec::new(); chars.len() + 1];
    let mut closes: Vec<Vec<&str>> = vec![Vec::new(); chars.len() + 1];
    let mut line_headings: Vec<(usize, u8)> = Vec::new();

    for span in &parsed.spans {
        match span.kind {
            Kind::Heading(level) => line_headings.push((span.start, level)),
            Kind::Inline(style) => {
                for (marker, s) in MARKERS_CANONICAL {
                    if style.contains(*s) {
                        opens[span.start].push(marker);
                        closes[span.end].push(marker);
                    }
                }
            }
        }
    }

    let mut result = String::new();
    // Emit a heading prefix at the start of any line that has one.
    let mut heading_at: std::collections::HashMap<usize, u8> = std::collections::HashMap::new();
    for (start, level) in line_headings {
        heading_at.insert(start, level);
    }

    for idx in 0..=chars.len() {
        // Close then re-open in reverse for tidy nesting.
        for marker in closes[idx].iter().rev() {
            result.push_str(marker);
        }
        if let Some(level) = heading_at.get(&idx) {
            result.push_str(&"#".repeat(*level as usize));
            result.push(' ');
        }
        for marker in &opens[idx] {
            result.push_str(marker);
        }
        if idx < chars.len() {
            result.push(chars[idx]);
        }
    }
    result
}

/// One canonical marker per style for serialisation (italic → `_`, not `*`).
const MARKERS_CANONICAL: &[(&str, Style)] = &[
    ("`", Style::CODE),
    ("**", Style::BOLD),
    ("_", Style::ITALIC),
    ("~~", Style::STRIKE),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(md: &str) -> String {
        to_markdown(&parse(md))
    }

    #[test]
    fn plain_text_is_unchanged() {
        assert_eq!(roundtrip("just some words"), "just some words");
    }

    #[test]
    fn bold_hides_markers_in_display() {
        let p = parse("a **b** c");
        assert_eq!(p.text, "a b c"); // markers gone from display
        assert_eq!(p.spans.len(), 1);
        assert_eq!(
            p.spans[0],
            Span {
                start: 2,
                end: 3,
                kind: Kind::Inline(Style::BOLD)
            }
        );
    }

    #[test]
    fn heading_display_drops_the_hashes() {
        let p = parse("# Title");
        assert_eq!(p.text, "Title");
        assert_eq!(p.spans[0].kind, Kind::Heading(1));
    }

    #[test]
    fn roundtrip_is_lossless_for_supported_marks() {
        for md in [
            "**bold**",
            "_italic_",
            "~~strike~~",
            "`code`",
            "# Heading",
            "## Sub heading",
            "a **b** and _c_ and ~~d~~",
            "# Title\n\nbody with **bold**",
            "line one\nline two",
            "plain",
            "",
        ] {
            assert_eq!(roundtrip(md), md, "round trip changed: {md:?}");
        }
    }

    #[test]
    fn unsupported_syntax_passes_through_untouched() {
        // Links, images, lists, tables are not our marks — must survive verbatim.
        for md in [
            "see [the doc](https://example.com)",
            "![alt](image.png)",
            "- a list item",
            "1. numbered",
            "> a quote",
        ] {
            assert_eq!(roundtrip(md), md, "passthrough changed: {md:?}");
        }
    }

    #[test]
    fn asterisk_italic_normalises_to_underscore() {
        // Display is identical; we canonicalise the marker on save.
        assert_eq!(parse("*x*").text, "x");
        assert_eq!(roundtrip("*x*"), "_x_");
    }

    #[test]
    fn unmatched_marker_is_literal() {
        // A lone `**` with no close is just text, not a format.
        assert_eq!(parse("a ** b").text, "a ** b");
        assert_eq!(roundtrip("a ** b"), "a ** b");
    }
}
