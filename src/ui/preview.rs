//! Live Preview: an editable rich view of the note body with the Markdown
//! markers hidden (Typora-style), backed by `crate::richtext`.
//!
//! Architecture: the SourceView's buffer stays the single source of truth —
//! autosave, undo and the mirror all keep working against it. The rich
//! `TextView` is a *projection*: entering preview parses the markdown into
//! marker-free text + tags; edits in the rich view are serialised back to
//! markdown and written into the source buffer (which autosaves). A `syncing`
//! flag breaks the loop. Because the markers never exist in the rich buffer,
//! there is no cursor-at-hidden-marker problem at all.

use crate::richtext::{self, Kind, Parsed, Span, Style};
use adw::prelude::*;
use gtk::glib;
use std::cell::Cell;
use std::rc::Rc;

/// Tag names, one per supported style; headings are `h1`..`h6`.
const BOLD: &str = "rt-bold";
const ITALIC: &str = "rt-italic";
const STRIKE: &str = "rt-strike";
const CODE: &str = "rt-code";

fn heading_tag(level: u8) -> String {
    format!("rt-h{level}")
}

/// Creates the formatting tags once per buffer.
pub fn install_tags(buffer: &gtk::TextBuffer) {
    let t = buffer.tag_table();
    let mk = |name: &str| -> gtk::TextTag {
        let tag = gtk::TextTag::new(Some(name));
        t.add(&tag);
        tag
    };
    mk(BOLD).set_weight(700);
    mk(ITALIC).set_style(gtk::pango::Style::Italic);
    mk(STRIKE).set_strikethrough(true);
    let code = mk(CODE);
    code.set_family(Some("monospace"));
    code.set_background_full_height(true);
    for (level, scale) in [(1u8, 1.7), (2, 1.4), (3, 1.2), (4, 1.1), (5, 1.05), (6, 1.0)] {
        let tag = mk(&heading_tag(level));
        tag.set_scale(scale);
        tag.set_weight(700);
    }
}

/// Replaces the buffer content with the parsed markdown, tags applied.
pub fn show_markdown(buffer: &gtk::TextBuffer, md: &str) {
    let parsed = richtext::parse(md);
    buffer.set_text(&parsed.text);
    let iter_at = |off: usize| buffer.iter_at_offset(off as i32);
    for span in &parsed.spans {
        let (start, end) = (iter_at(span.start), iter_at(span.end));
        match span.kind {
            Kind::Inline(style) => {
                for (name, s) in [
                    (BOLD, Style::BOLD),
                    (ITALIC, Style::ITALIC),
                    (STRIKE, Style::STRIKE),
                    (CODE, Style::CODE),
                ] {
                    if style.contains(s) {
                        buffer.apply_tag_by_name(name, &start, &end);
                    }
                }
            }
            Kind::Heading(level) => {
                buffer.apply_tag_by_name(&heading_tag(level), &start, &end);
            }
        }
    }
}

/// Serialises the rich buffer (text + tags) back to markdown.
pub fn markdown_from(buffer: &gtk::TextBuffer) -> String {
    let text = buffer
        .text(&buffer.start_iter(), &buffer.end_iter(), true)
        .to_string();
    let len = text.chars().count();

    // Per-char style bitset + heading level, read straight off the tags.
    let mut styles: Vec<u8> = Vec::with_capacity(len);
    let mut headings: Vec<u8> = Vec::with_capacity(len);
    for off in 0..len {
        let iter = buffer.iter_at_offset(off as i32);
        let mut s = Style::default();
        let mut h = 0u8;
        for tag in iter.tags() {
            match tag.name().as_deref() {
                Some(BOLD) => s = s.with(Style::BOLD),
                Some(ITALIC) => s = s.with(Style::ITALIC),
                Some(STRIKE) => s = s.with(Style::STRIKE),
                Some(CODE) => s = s.with(Style::CODE),
                Some(name) => {
                    if let Some(l) = name.strip_prefix("rt-h") {
                        h = l.parse().unwrap_or(0);
                    }
                }
                None => {}
            }
        }
        styles.push(style_bits(s));
        headings.push(h);
    }
    richtext::to_markdown(&rebuild_spans(&text, &styles, &headings))
}

fn style_bits(s: Style) -> u8 {
    let mut b = 0;
    if s.contains(Style::BOLD) {
        b |= 1;
    }
    if s.contains(Style::ITALIC) {
        b |= 2;
    }
    if s.contains(Style::STRIKE) {
        b |= 4;
    }
    if s.contains(Style::CODE) {
        b |= 8;
    }
    b
}

fn bits_style(b: u8) -> Style {
    let mut s = Style::default();
    if b & 1 != 0 {
        s = s.with(Style::BOLD);
    }
    if b & 2 != 0 {
        s = s.with(Style::ITALIC);
    }
    if b & 4 != 0 {
        s = s.with(Style::STRIKE);
    }
    if b & 8 != 0 {
        s = s.with(Style::CODE);
    }
    s
}

/// Pure core, testable without GTK: turns per-char style bits + heading levels
/// into `Parsed` spans. Inline runs never cross a newline (markdown emphasis
/// is line-scoped), and headings span each line's content.
pub fn rebuild_spans(text: &str, styles: &[u8], headings: &[u8]) -> Parsed {
    let chars: Vec<char> = text.chars().collect();
    let mut spans: Vec<Span> = Vec::new();

    // Inline runs of identical non-zero style, split at newlines.
    let mut run_start: Option<(usize, u8)> = None;
    for i in 0..=chars.len() {
        let here = if i < chars.len() && chars[i] != '\n' {
            styles.get(i).copied().unwrap_or(0)
        } else {
            0
        };
        match run_start {
            Some((start, bits)) if bits != here => {
                spans.push(Span {
                    start,
                    end: i,
                    kind: Kind::Inline(bits_style(bits)),
                });
                run_start = (here != 0).then_some((i, here));
            }
            None if here != 0 => run_start = Some((i, here)),
            _ => {}
        }
    }

    // One heading span per line that carries a heading level (majority: the
    // level found on its first char).
    let mut line_start = 0;
    for i in 0..=chars.len() {
        if i == chars.len() || chars[i] == '\n' {
            if line_start < i {
                let level = headings.get(line_start).copied().unwrap_or(0);
                if level > 0 {
                    spans.push(Span {
                        start: line_start,
                        end: i,
                        kind: Kind::Heading(level),
                    });
                }
            }
            line_start = i + 1;
        }
    }

    Parsed {
        text: text.to_string(),
        spans,
    }
}

/// Wires the preview toggle, the rich view, and the two-way sync.
pub fn install(
    state: &super::window::Shared,
    widgets: &Rc<super::window::Widgets>,
    rich_view: &gtk::TextView,
    toggle: &gtk::ToggleButton,
) {
    let rich_buffer = rich_view.buffer();
    install_tags(&rich_buffer);
    let syncing = Rc::new(Cell::new(false));

    // Toggle on: project the markdown; toggle off: back to the source view.
    {
        let (widgets, rich_buffer, syncing) = (widgets.clone(), rich_buffer.clone(), syncing.clone());
        let state = state.clone();
        toggle.connect_toggled(move |t| {
            if t.is_active() {
                // Only text notes have a markdown body.
                let is_text = state
                    .borrow()
                    .selected_note()
                    .is_some_and(|n| n.checklist_items.is_none());
                if !is_text {
                    t.set_active(false);
                    return;
                }
                let md = widgets
                    .buffer
                    .text(&widgets.buffer.start_iter(), &widgets.buffer.end_iter(), true)
                    .to_string();
                syncing.set(true);
                show_markdown(&rich_buffer, &md);
                syncing.set(false);
                widgets.body_stack.set_visible_child_name("preview");
            } else if widgets.body_stack.visible_child_name().as_deref() == Some("preview") {
                widgets.body_stack.set_visible_child_name("text");
            }
        });
    }

    // Rich edits → markdown → source buffer (autosave picks it up).
    let sync_back = {
        let (widgets, rich_buffer, syncing) = (widgets.clone(), rich_buffer.clone(), syncing.clone());
        Rc::new(move || {
            if syncing.get() {
                return;
            }
            let md = markdown_from(&rich_buffer);
            syncing.set(true);
            widgets.buffer.set_text(&md);
            syncing.set(false);
        })
    };
    {
        let sync_back = sync_back.clone();
        rich_buffer.connect_changed(move |_| sync_back());
    }

    // Someone else changed the source while preview is open (note switch,
    // watcher reload): leave preview — the projection is stale and the
    // selected note may not even be the same one.
    {
        let (toggle, syncing) = (toggle.clone(), syncing.clone());
        widgets.buffer.connect_changed(move |_| {
            if !syncing.get() && toggle.is_active() {
                toggle.set_active(false);
            }
        });
    }

    // Formatting keys in the rich view toggle tags instead of typing markers.
    {
        let (rich_buffer, sync_back) = (rich_buffer.clone(), sync_back.clone());
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            if !modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK) {
                return glib::Propagation::Proceed;
            }
            let name = match key.to_unicode().map(|c| c.to_ascii_lowercase()) {
                Some('b') => BOLD,
                Some('i') => ITALIC,
                Some('s') => STRIKE,
                Some('1') => return heading_shortcut(&rich_buffer, 1, &sync_back),
                Some('2') => return heading_shortcut(&rich_buffer, 2, &sync_back),
                Some('3') => return heading_shortcut(&rich_buffer, 3, &sync_back),
                _ => return glib::Propagation::Proceed,
            };
            if let Some((start, end)) = rich_buffer.selection_bounds() {
                let tag = rich_buffer.tag_table().lookup(name).unwrap();
                if start.has_tag(&tag) {
                    rich_buffer.remove_tag(&tag, &start, &end);
                } else {
                    rich_buffer.apply_tag(&tag, &start, &end);
                }
                sync_back();
            }
            glib::Propagation::Stop
        });
        rich_view.add_controller(keys);
    }
}

fn heading_shortcut(
    buffer: &gtk::TextBuffer,
    level: u8,
    sync_back: &Rc<impl Fn() + 'static>,
) -> glib::Propagation {
    let cursor = buffer.iter_at_mark(&buffer.get_insert());
    let mut start = cursor;
    start.set_line_offset(0);
    let mut end = cursor;
    if !end.ends_line() {
        end.forward_to_line_end();
    }
    // Clear any other heading level first, then toggle the requested one.
    let table = buffer.tag_table();
    let tag = table.lookup(&heading_tag(level)).unwrap();
    let had = start.has_tag(&tag);
    for l in 1..=6u8 {
        if let Some(t) = table.lookup(&heading_tag(l)) {
            buffer.remove_tag(&t, &start, &end);
        }
    }
    if !had {
        buffer.apply_tag(&tag, &start, &end);
    }
    sync_back();
    glib::Propagation::Stop
}

#[cfg(test)]
mod tests {
    use super::*;

    /// chars of `text` styled with `bits` between `from..to`, plain elsewhere.
    fn bits(text: &str, from: usize, to: usize, b: u8) -> Vec<u8> {
        (0..text.chars().count())
            .map(|i| if (from..to).contains(&i) { b } else { 0 })
            .collect()
    }

    #[test]
    fn a_bold_run_serialises_back_to_markdown() {
        let text = "a b c";
        let styles = bits(text, 2, 3, 1);
        let headings = vec![0; 5];
        let md = richtext::to_markdown(&rebuild_spans(text, &styles, &headings));
        assert_eq!(md, "a **b** c");
    }

    #[test]
    fn projection_round_trips_through_the_rebuilt_spans() {
        // parse → per-char samples → rebuild → serialise must reproduce the
        // original for simple (non-nested) markdown.
        for md in ["**bold** and _italic_", "# Title\nplain", "~~gone~~", "plain"] {
            let parsed = richtext::parse(md);
            let n = parsed.text.chars().count();
            let mut styles = vec![0u8; n];
            let mut headings = vec![0u8; n];
            for span in &parsed.spans {
                for i in span.start..span.end.min(n) {
                    match span.kind {
                        Kind::Inline(s) => styles[i] |= style_bits(s),
                        Kind::Heading(h) => headings[i] = h,
                    }
                }
            }
            let back = richtext::to_markdown(&rebuild_spans(&parsed.text, &styles, &headings));
            assert_eq!(back, md, "projection was lossy for {md:?}");
        }
    }

    #[test]
    fn mixed_run_emits_both_markers() {
        let text = "ab";
        let styles = vec![3, 0]; // 'a' bold+italic
        let md = richtext::to_markdown(&rebuild_spans(text, &styles, &[0, 0]));
        // Both markers open and close around the run; exact order is defined
        // by MARKERS_CANONICAL. Re-parsing shows the same styling.
        let re = richtext::parse(&md);
        assert_eq!(re.text, "ab");
        assert_eq!(re.spans.len(), 2);
    }

    #[test]
    fn runs_do_not_cross_newlines() {
        let text = "a\nb";
        let styles = vec![1, 1, 1]; // naive tags spanning the newline
        let md = richtext::to_markdown(&rebuild_spans(text, &styles, &[0, 0, 0]));
        assert_eq!(md, "**a**\n**b**", "emphasis must stay line-scoped");
    }
}
