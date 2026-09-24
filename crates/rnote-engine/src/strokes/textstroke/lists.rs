//! Bullet and numbered lists in text strokes.
//!
//! A paragraph is a list item when it starts with tabs for its nesting level, followed by a bullet
//! (`•`, `◦`, `▪`) or a number (`1.`, `1)`) and a space. Lists are stored as plain text, so they
//! look the same wherever the text is shown.
//!
//! Wrapped lines of list items get a hanging indent: at every wrap a line separator
//! ([SOFT_BREAK]) is inserted, followed by spaces that line the wrapped line up with the start of
//! the item's text. These are recreated whenever the text changes.

// Imports
use super::{RangedTextAttribute, TextAlignment, TextStroke};
use piet::TextLayout;
use std::ops::Range;
use unicode_segmentation::GraphemeCursor;

/// Marks an automatic line break inside a list item. It is followed by the indentation of the
/// wrapped line.
pub const SOFT_BREAK: char = '\u{2028}';
/// Used together with regular spaces to fine tune the indentation of wrapped lines.
const HAIR_SPACE: char = '\u{200A}';
/// The bullets for the nesting levels, repeating.
const BULLETS: [char; 3] = ['•', '◦', '▪'];
const MAX_LEVEL: usize = 8;
/// Hanging indents that are wider than this fraction of the text width are left out.
const MAX_INDENT_FRACTION: f64 = 0.6;
/// Upper bound for the hanging indents inserted in one go.
const MAX_REFLOW_ITERATIONS: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Marker {
    Bullet(char),
    Number { n: u32, delim: char },
}

impl Marker {
    fn text(self) -> String {
        match self {
            Marker::Bullet(bullet) => bullet.to_string(),
            Marker::Number { n, delim } => format!("{n}{delim}"),
        }
    }

    /// The marker of the following item in the same list.
    fn next(self) -> Self {
        match self {
            Marker::Bullet(bullet) => Marker::Bullet(bullet),
            Marker::Number { n, delim } => Marker::Number {
                n: n.saturating_add(1),
                delim,
            },
        }
    }

    /// The marker when the item is moved to another level.
    fn for_level(self, level: usize) -> Self {
        match self {
            Marker::Bullet(_) => Marker::Bullet(bullet_for_level(level)),
            // Gets the right number when the list is renumbered.
            Marker::Number { delim, .. } => Marker::Number { n: 1, delim },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ListItem {
    /// Start of the paragraph.
    start: usize,
    /// End of the paragraph, not including the newline.
    end: usize,
    level: usize,
    marker: Marker,
    marker_start: usize,
    marker_end: usize,
    /// Where the text of the item starts, after the marker and the space.
    content_start: usize,
}

fn bullet_for_level(level: usize) -> char {
    BULLETS[level % BULLETS.len()]
}

fn is_indent_char(c: char) -> bool {
    c == ' ' || c == HAIR_SPACE
}

/// The ranges of the paragraphs, not including the newlines between them.
fn paragraph_ranges(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for (i, c) in text.char_indices() {
        if c == '\n' {
            ranges.push(start..i);
            start = i + 1;
        }
    }
    ranges.push(start..text.len());
    ranges
}

fn parse_list_item(text: &str, paragraph: Range<usize>) -> Option<ListItem> {
    let paragraph_text = &text[paragraph.clone()];
    let level = paragraph_text.bytes().take_while(|b| *b == b'\t').count();
    let rest = &paragraph_text[level..];
    let first = rest.chars().next()?;

    let (marker, marker_len) = if BULLETS.contains(&first) {
        (Marker::Bullet(first), first.len_utf8())
    } else if first.is_ascii_digit() {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let delim = rest[digits..].chars().next()?;
        if digits > 6 || (delim != '.' && delim != ')') {
            return None;
        }
        let n = rest[..digits].parse().ok()?;
        (Marker::Number { n, delim }, digits + 1)
    } else {
        return None;
    };
    if !rest[marker_len..].starts_with(' ') {
        return None;
    }

    let marker_start = paragraph.start + level;
    Some(ListItem {
        start: paragraph.start,
        end: paragraph.end,
        level,
        marker,
        marker_start,
        marker_end: marker_start + marker_len,
        content_start: marker_start + marker_len + 1,
    })
}

fn list_items(text: &str) -> Vec<ListItem> {
    paragraph_ranges(text)
        .into_iter()
        .filter_map(|paragraph| parse_list_item(text, paragraph))
        .collect()
}

/// The list item whose paragraph contains `pos`.
fn list_item_at(text: &str, pos: usize) -> Option<ListItem> {
    let start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
    let end = text[pos..].find('\n').map_or(text.len(), |i| pos + i);
    parse_list_item(text, start..end)
}

/// The range of the automatic line break with its indentation, if `pos` lies behind the break.
fn soft_break_before(text: &str, pos: usize) -> Option<Range<usize>> {
    let before = text[..pos].trim_end_matches(is_indent_char);
    let start = before.strip_suffix(SOFT_BREAK)?.len();
    let after = &text[pos..];
    let end = pos + after.len() - after.trim_start_matches(is_indent_char).len();
    Some(start..end)
}

/// The ranges of all automatic line breaks with their indentation.
fn soft_break_ranges(text: &str) -> Vec<Range<usize>> {
    text.match_indices(SOFT_BREAK)
        .map(|(start, _)| {
            let after = &text[start + SOFT_BREAK.len_utf8()..];
            let indent_len = after.len() - after.trim_start_matches(is_indent_char).len();
            start..start + SOFT_BREAK.len_utf8() + indent_len
        })
        .collect()
}

/// The indentation that is closest to `width`, made of spaces and hair spaces.
fn indent_for_width(width: f64, space_width: f64, hair_space_width: f64) -> String {
    if space_width <= 0.0 {
        return String::new();
    }
    let spaces = (width / space_width).floor();
    let hair_spaces = if hair_space_width > 0.0 {
        ((width - spaces * space_width) / hair_space_width).round()
    } else {
        0.0
    };
    " ".repeat(spaces as usize) + &HAIR_SPACE.to_string().repeat(hair_spaces as usize)
}

impl TextStroke {
    /// Bring the lists up to date after the text was edited.
    ///
    /// Turns a typed `- ` or `* ` at the start of a paragraph into a bullet, renumbers the
    /// numbered lists and recreates the hanging indents of wrapped list items. `typed` is the text
    /// that was just typed at the cursor, if any.
    ///
    /// Returns whether the text changed.
    pub fn update_lists(
        &mut self,
        cursor: &mut GraphemeCursor,
        selection_cursor: Option<&mut GraphemeCursor>,
        typed: Option<&str>,
    ) -> bool {
        let typed_space = typed == Some(" ");
        if !typed_space && !self.text.contains(SOFT_BREAK) && list_items(&self.text).is_empty() {
            return false;
        }

        let text_before = self.text.clone();
        let mut positions = vec![cursor.cur_cursor()];
        if let Some(selection_cursor) = &selection_cursor {
            positions.push(selection_cursor.cur_cursor());
        }

        if typed_space {
            self.autoformat_bullet(positions[0], &mut positions);
        }
        self.relayout_lists(&mut positions);

        *cursor = GraphemeCursor::new(positions[0], self.text.len(), true);
        if let Some(selection_cursor) = selection_cursor {
            *selection_cursor = GraphemeCursor::new(positions[1], self.text.len(), true);
        }
        self.text != text_before
    }

    /// Handle a newline in a list item: continues the list with a new item, or ends the list
    /// when the item is empty.
    ///
    /// Returns `false` without changing anything when the cursor is not in a list item.
    pub fn list_newline(&mut self, cursor: &mut GraphemeCursor) -> bool {
        let pos = self.normalized_cursor_pos(cursor.cur_cursor(), true);
        let Some(item) = list_item_at(&self.text, pos) else {
            return false;
        };
        let pos = pos.max(item.content_start);
        let mut positions = vec![pos];

        if self.text[item.content_start..item.end].trim().is_empty() {
            if item.level > 0 {
                self.set_list_item_level(item, item.level - 1, &mut positions);
            } else {
                self.replace_range_mapped(item.start..item.content_start, "", &mut positions);
            }
        } else {
            let prefix = format!(
                "\n{}{} ",
                "\t".repeat(item.level),
                item.marker.next().text()
            );
            self.replace_range_mapped(pos..pos, &prefix, &mut positions);
        }
        self.relayout_lists(&mut positions);

        *cursor = GraphemeCursor::new(positions[0], self.text.len(), true);
        true
    }

    /// Indent or outdent the list items touched by the cursor or the selection.
    ///
    /// Returns `false` without changing anything when none of them is a list item.
    pub fn list_change_level(
        &mut self,
        cursor: &mut GraphemeCursor,
        selection_cursor: Option<&mut GraphemeCursor>,
        outdent: bool,
    ) -> bool {
        let cursor_pos = cursor.cur_cursor();
        let selection_pos = selection_cursor
            .as_ref()
            .map_or(cursor_pos, |c| c.cur_cursor());
        let range = crate::utils::positive_range(cursor_pos, selection_pos);
        let items = list_items(&self.text)
            .into_iter()
            .filter(|item| item.start <= range.end && range.start <= item.end)
            .collect::<Vec<ListItem>>();
        if items.is_empty() {
            return false;
        }

        let mut positions = vec![cursor_pos, selection_pos];
        // From the back, so the positions of the items in front stay valid.
        for item in items.into_iter().rev() {
            let level = if outdent {
                item.level.saturating_sub(1)
            } else {
                (item.level + 1).min(MAX_LEVEL)
            };
            if level != item.level {
                self.set_list_item_level(item, level, &mut positions);
            }
        }
        self.relayout_lists(&mut positions);

        *cursor = GraphemeCursor::new(positions[0], self.text.len(), true);
        if let Some(selection_cursor) = selection_cursor {
            *selection_cursor = GraphemeCursor::new(positions[1], self.text.len(), true);
        }
        true
    }

    /// Handle backspace at the start of a list item's text, which removes the marker, and at the
    /// start of a wrapped line, which removes what is in front of the automatic line break.
    ///
    /// Returns `false` without changing anything otherwise.
    pub fn list_backspace(&mut self, cursor: &mut GraphemeCursor) -> bool {
        let pos = cursor.cur_cursor();
        let mut positions = vec![pos];

        if let Some(soft_break) = soft_break_before(&self.text, pos)
            && soft_break.end == pos
        {
            let Ok(Some(prev)) = GraphemeCursor::new(soft_break.start, self.text.len(), true)
                .prev_boundary(&self.text, 0)
            else {
                return false;
            };
            self.replace_range_mapped(prev..soft_break.start, "", &mut positions);
        } else if let Some(item) = list_item_at(&self.text, pos)
            && pos == item.content_start
        {
            self.replace_range_mapped(item.marker_start..item.content_start, "", &mut positions);
        } else {
            return false;
        }
        self.relayout_lists(&mut positions);

        *cursor = GraphemeCursor::new(positions[0], self.text.len(), true);
        true
    }

    /// Handle delete in front of an automatic line break, which removes what follows the break,
    /// and at the end of a paragraph followed by a list item, which removes the item's marker
    /// together with the newline.
    ///
    /// Returns `false` without changing anything otherwise.
    pub fn list_delete(&mut self, cursor: &mut GraphemeCursor) -> bool {
        let pos = cursor.cur_cursor();
        let mut positions = vec![pos];
        let after = &self.text[pos..];

        if after.starts_with(SOFT_BREAK) {
            let Some(soft_break) = soft_break_before(&self.text, pos + SOFT_BREAK.len_utf8())
            else {
                return false;
            };
            let Ok(Some(next)) = GraphemeCursor::new(soft_break.end, self.text.len(), true)
                .next_boundary(&self.text, 0)
            else {
                return false;
            };
            self.replace_range_mapped(soft_break.end..next, "", &mut positions);
        } else if after.starts_with('\n')
            && let Some(next_item) = list_item_at(&self.text, pos + 1)
        {
            self.replace_range_mapped(pos..next_item.content_start, "", &mut positions);
        } else {
            return false;
        }
        self.relayout_lists(&mut positions);

        *cursor = GraphemeCursor::new(positions[0], self.text.len(), true);
        true
    }

    /// Move the cursor out of places where it should not be: the markers of list items and the
    /// indentation of wrapped lines. `forward` is the direction the cursor was moving in.
    pub fn normalize_cursor(&self, cursor: &mut GraphemeCursor, forward: bool) {
        let pos = cursor.cur_cursor();
        let normalized = self.normalized_cursor_pos(pos, forward);
        if normalized != pos {
            *cursor = GraphemeCursor::new(normalized, self.text.len(), true);
        }
    }

    pub(super) fn normalized_cursor_pos(&self, pos: usize, forward: bool) -> usize {
        let pos = pos.min(self.text.len());
        if let Some(soft_break) = soft_break_before(&self.text, pos)
            && pos < soft_break.end
        {
            return if forward {
                soft_break.end
            } else {
                soft_break.start
            };
        }
        if let Some(item) = list_item_at(&self.text, pos)
            && pos < item.content_start
        {
            // Jump over the marker to the end of the previous paragraph when moving backwards.
            return if !forward && item.start > 0 {
                item.start - 1
            } else {
                item.content_start
            };
        }
        pos
    }

    /// The text in the given range, without the automatic line breaks.
    pub fn text_without_soft_breaks(&self, range: Range<usize>) -> String {
        let mut text = String::with_capacity(range.len());
        let mut chars = self.text[range].chars().peekable();
        while let Some(c) = chars.next() {
            if c == SOFT_BREAK {
                while chars.next_if(|c| is_indent_char(*c)).is_some() {}
            } else {
                text.push(c);
            }
        }
        text
    }

    /// Renumber the lists and recreate the hanging indents.
    fn relayout_lists(&mut self, positions: &mut [usize]) {
        for soft_break in soft_break_ranges(&self.text).into_iter().rev() {
            self.replace_range_mapped(soft_break, "", positions);
        }
        self.merge_adjacent_attrs();
        self.renumber_lists(positions);
        self.reflow_list_items(positions);
    }

    /// Turn a typed `- ` or `* ` in front of `pos` at the start of a paragraph into a bullet.
    fn autoformat_bullet(&mut self, pos: usize, positions: &mut [usize]) {
        let paragraph_start = self.text[..pos].rfind('\n').map_or(0, |i| i + 1);
        let typed = &self.text[paragraph_start..pos];
        let level = typed.bytes().take_while(|b| *b == b'\t').count();
        if matches!(&typed[level..], "- " | "* ") {
            let marker_start = paragraph_start + level;
            self.replace_range_mapped(
                marker_start..marker_start + 1,
                &bullet_for_level(level).to_string(),
                positions,
            );
        }
    }

    fn set_list_item_level(&mut self, item: ListItem, level: usize, positions: &mut [usize]) {
        let prefix = format!(
            "{}{}",
            "\t".repeat(level),
            item.marker.for_level(level).text()
        );
        self.replace_range_mapped(item.start..item.marker_end, &prefix, positions);
    }

    /// Number the items of each numbered list consecutively. The first item of a list keeps its
    /// number, so lists can start at any number.
    fn renumber_lists(&mut self, positions: &mut [usize]) {
        let mut replacements = Vec::new();
        // The number of the previous item for each level of the current list.
        let mut counters: Vec<Option<u32>> = Vec::new();

        for paragraph in paragraph_ranges(&self.text) {
            let Some(item) = parse_list_item(&self.text, paragraph) else {
                counters.clear();
                continue;
            };
            counters.truncate(item.level + 1);
            counters.resize(item.level + 1, None);

            match item.marker {
                Marker::Number { n, delim } => {
                    let wanted = counters[item.level].map_or(n, |prev| prev.saturating_add(1));
                    if wanted != n {
                        replacements.push((
                            item.marker_start..item.marker_end,
                            Marker::Number { n: wanted, delim }.text(),
                        ));
                    }
                    counters[item.level] = Some(wanted);
                }
                Marker::Bullet(_) => counters[item.level] = None,
            }
        }

        for (range, replacement) in replacements.into_iter().rev() {
            self.replace_range_mapped(range, &replacement, positions);
        }
    }

    /// Insert automatic line breaks with an indentation where list items wrap, so that the
    /// wrapped lines line up with the start of the item's text.
    ///
    /// Expects that there are no automatic line breaks in the text.
    fn reflow_list_items(&mut self, positions: &mut [usize]) {
        let Some(max_width) = self.text_style.max_width() else {
            return;
        };
        if !matches!(self.text_style.alignment, TextAlignment::Start) {
            return;
        }
        let Some((space_width, hair_space_width)) = self.indent_char_widths() else {
            return;
        };
        // Wraps that are left as they are, because the indentation does not fit.
        let mut skipped = Vec::new();

        for _ in 0..MAX_REFLOW_ITERATIONS {
            let items = list_items(&self.text);
            if items.is_empty() {
                return;
            }
            let Ok(text_layout) = self
                .text_style
                .build_text_layout(&mut piet_cairo::CairoText::new(), self.text.clone())
            else {
                return;
            };

            // The first line of a list item that wrapped without an automatic line break.
            let Some((line, wrap_pos, item)) = (1..text_layout.line_count()).find_map(|line| {
                let wrap_pos = text_layout.line_metric(line)?.start_offset;
                let before = &self.text[..wrap_pos];
                if before.ends_with('\n')
                    || before.ends_with(SOFT_BREAK)
                    || self.text[wrap_pos..].starts_with(SOFT_BREAK)
                {
                    return None;
                }
                if skipped.contains(&wrap_pos) {
                    return None;
                }
                let item = items
                    .iter()
                    .find(|item| item.start < wrap_pos && wrap_pos <= item.end)?;
                Some((line, wrap_pos, *item))
            }) else {
                return;
            };

            // When the previous line holds nothing but the indentation, the following word does
            // not fit next to it. Then this wrap is left without the indentation.
            let prev_line_start = text_layout
                .line_metric(line - 1)
                .map_or(0, |lm| lm.start_offset);
            if let Some(soft_break) = soft_break_before(&self.text, prev_line_start)
                && soft_break.end == wrap_pos
            {
                skipped.push(soft_break.start);
                self.replace_range_mapped(soft_break, "", positions);
                continue;
            }

            let indent_width = text_layout
                .hit_test_text_position(item.content_start)
                .point
                .x;
            if indent_width <= 0.0 || indent_width > max_width * MAX_INDENT_FRACTION {
                skipped.push(wrap_pos);
                continue;
            }

            let soft_break = format!(
                "{SOFT_BREAK}{}",
                indent_for_width(indent_width, space_width, hair_space_width)
            );
            self.replace_range_mapped(wrap_pos..wrap_pos, &soft_break, positions);
            // The indentation is measured without the ranged attributes, so it must not get any.
            self.remove_attrs_for_range(wrap_pos..wrap_pos + soft_break.len());
        }
    }

    /// The widths of a space and a hair space in the base style of the text.
    fn indent_char_widths(&self) -> Option<(f64, f64)> {
        const SAMPLE_LEN: usize = 8;

        let mut text_style = self.text_style.clone();
        text_style.ranged_text_attributes.clear();
        text_style.set_max_width(None);
        let sample = " ".repeat(SAMPLE_LEN) + &HAIR_SPACE.to_string().repeat(SAMPLE_LEN);
        let text_layout = text_style
            .build_text_layout(&mut piet_cairo::CairoText::new(), sample)
            .ok()?;

        let spaces_width = text_layout.hit_test_text_position(SAMPLE_LEN).point.x;
        let all_width = text_layout
            .hit_test_text_position(SAMPLE_LEN * (1 + HAIR_SPACE.len_utf8()))
            .point
            .x;
        Some((
            spaces_width / SAMPLE_LEN as f64,
            (all_width - spaces_width) / SAMPLE_LEN as f64,
        ))
    }

    /// Replace the text in `range`, keeping `positions` and the ranged text attributes in place.
    ///
    /// Positions inside the replaced range end up behind the replacement. Text inserted at the
    /// boundary of an attribute is not covered by it.
    fn replace_range_mapped(
        &mut self,
        range: Range<usize>,
        replacement: &str,
        positions: &mut [usize],
    ) {
        let map_start = |pos: usize| {
            if pos < range.start {
                pos
            } else if pos >= range.end {
                pos - range.len() + replacement.len()
            } else {
                range.start + replacement.len()
            }
        };
        let map_end = |pos: usize| {
            if pos <= range.start {
                pos
            } else {
                map_start(pos)
            }
        };

        self.text.replace_range(range.clone(), replacement);
        for pos in positions.iter_mut() {
            *pos = map_start(*pos);
        }
        self.text_style.ranged_text_attributes.retain_mut(|attr| {
            attr.range = map_start(attr.range.start)..map_end(attr.range.end);
            attr.range.start < attr.range.end
        });
    }

    /// Merge touching ranged text attributes that are the same. The automatic line breaks split
    /// the attributes, without this they would get split into ever more parts.
    fn merge_adjacent_attrs(&mut self) {
        let mut merged: Vec<RangedTextAttribute> =
            Vec::with_capacity(self.text_style.ranged_text_attributes.len());

        for attr in self.text_style.ranged_text_attributes.drain(..) {
            // Only merge with the latest attribute of the same kind, so attributes that override
            // each other keep their order.
            let latest_same_kind = merged.iter_mut().rev().find(|m| {
                std::mem::discriminant(&m.attribute) == std::mem::discriminant(&attr.attribute)
            });
            match latest_same_kind {
                Some(latest)
                    if latest.attribute == attr.attribute
                        && latest.range.start <= attr.range.end
                        && attr.range.start <= latest.range.end =>
                {
                    latest.range = latest.range.start.min(attr.range.start)
                        ..latest.range.end.max(attr.range.end);
                }
                _ => merged.push(attr),
            }
        }

        self.text_style.ranged_text_attributes = merged;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strokes::textstroke::{TextAttribute, TextStyle};
    use p2d::math::Vector2;

    fn textstroke(text: &str) -> TextStroke {
        let mut text_style = TextStyle::default();
        text_style.set_max_width(None);
        TextStroke::new(text.to_string(), Vector2::ZERO, text_style)
    }

    fn cursor_at(textstroke: &TextStroke, pos: usize) -> GraphemeCursor {
        GraphemeCursor::new(pos, textstroke.text.len(), true)
    }

    #[test]
    fn parses_list_items() {
        let text = "• a\n\t◦ b\n12. c\n3) d\nplain\n•no space\n1.5 e";
        let items = list_items(text);

        assert_eq!(items.len(), 4);
        assert_eq!(items[0].marker, Marker::Bullet('•'));
        assert_eq!(items[1].level, 1);
        assert_eq!(&text[items[1].content_start..items[1].end], "b");
        assert_eq!(items[2].marker, Marker::Number { n: 12, delim: '.' });
        assert_eq!(items[3].marker, Marker::Number { n: 3, delim: ')' });
    }

    #[test]
    fn typed_dash_becomes_a_bullet() {
        let mut ts = textstroke("intro\n\t- ");
        let mut cursor = cursor_at(&ts, ts.text.len());

        assert!(ts.update_lists(&mut cursor, None, Some(" ")));
        assert_eq!(ts.text, "intro\n\t◦ ");
        assert_eq!(cursor.cur_cursor(), ts.text.len());

        // Not at the start of a paragraph.
        let mut ts = textstroke("a - ");
        let mut cursor = cursor_at(&ts, ts.text.len());
        assert!(!ts.update_lists(&mut cursor, None, Some(" ")));
        assert_eq!(ts.text, "a - ");
    }

    #[test]
    fn newline_continues_and_ends_the_list() {
        let mut ts = textstroke("1. first");
        let mut cursor = cursor_at(&ts, ts.text.len());

        assert!(ts.list_newline(&mut cursor));
        assert_eq!(ts.text, "1. first\n2. ");
        assert_eq!(cursor.cur_cursor(), ts.text.len());

        // On the empty item the list ends.
        assert!(ts.list_newline(&mut cursor));
        assert_eq!(ts.text, "1. first\n");
        assert_eq!(cursor.cur_cursor(), ts.text.len());

        // Outside of lists nothing happens.
        assert!(!ts.list_newline(&mut cursor));
    }

    #[test]
    fn newline_in_an_empty_nested_item_outdents() {
        let mut ts = textstroke("• a\n\t◦ ");
        let mut cursor = cursor_at(&ts, ts.text.len());

        assert!(ts.list_newline(&mut cursor));
        assert_eq!(ts.text, "• a\n• ");
        assert_eq!(cursor.cur_cursor(), ts.text.len());
    }

    #[test]
    fn newline_splits_an_item() {
        let mut ts = textstroke("• ab");
        let mut cursor = cursor_at(&ts, "• a".len());

        assert!(ts.list_newline(&mut cursor));
        assert_eq!(ts.text, "• a\n• b");
        assert_eq!(cursor.cur_cursor(), "• a\n• ".len());
    }

    #[test]
    fn tab_changes_the_level_and_renumbers() {
        let mut ts = textstroke("1. a\n2. b\n3. c");
        let mut cursor = cursor_at(&ts, "1. a\n2. b".len());

        assert!(ts.list_change_level(&mut cursor, None, false));
        assert_eq!(ts.text, "1. a\n\t1. b\n2. c");
        assert_eq!(cursor.cur_cursor(), "1. a\n\t1. b".len());

        assert!(ts.list_change_level(&mut cursor, None, true));
        assert_eq!(ts.text, "1. a\n2. b\n3. c");
        assert_eq!(cursor.cur_cursor(), "1. a\n2. b".len());
    }

    #[test]
    fn tab_changes_all_selected_items() {
        let mut ts = textstroke("• a\n• b\nplain");
        let mut cursor = cursor_at(&ts, 0);
        let mut selection_cursor = cursor_at(&ts, ts.text.len());

        assert!(ts.list_change_level(&mut cursor, Some(&mut selection_cursor), false));
        assert_eq!(ts.text, "\t◦ a\n\t◦ b\nplain");
        assert_eq!(selection_cursor.cur_cursor(), ts.text.len());

        let mut ts = textstroke("plain");
        let mut cursor = cursor_at(&ts, 2);
        assert!(!ts.list_change_level(&mut cursor, None, false));
    }

    #[test]
    fn backspace_removes_the_marker() {
        let mut ts = textstroke("\t◦ a");
        let mut cursor = cursor_at(&ts, "\t◦ ".len());

        assert!(ts.list_backspace(&mut cursor));
        assert_eq!(ts.text, "\ta");
        assert_eq!(cursor.cur_cursor(), 1);
    }

    #[test]
    fn delete_merges_the_next_item_without_its_marker() {
        let mut ts = textstroke("• a\n• b");
        let mut cursor = cursor_at(&ts, "• a".len());

        assert!(ts.list_delete(&mut cursor));
        assert_eq!(ts.text, "• ab");
        assert_eq!(cursor.cur_cursor(), "• a".len());
    }

    #[test]
    fn soft_breaks_are_invisible_to_the_cursor_and_the_clipboard() {
        let text = format!("• one two{SOFT_BREAK}  {HAIR_SPACE}three");
        let ts = textstroke(&text);
        let break_start = "• one two".len();
        let content_start = text.len() - "three".len();

        assert_eq!(
            ts.normalized_cursor_pos(break_start + 4, true),
            content_start
        );
        assert_eq!(
            ts.normalized_cursor_pos(break_start + 4, false),
            break_start
        );
        assert_eq!(ts.normalized_cursor_pos(content_start, true), content_start);
        // Inside the marker
        assert_eq!(ts.normalized_cursor_pos(0, true), "• ".len());
        assert_eq!(ts.text_without_soft_breaks(0..text.len()), "• one twothree");
    }

    #[test]
    fn attributes_keep_their_place_and_get_merged() {
        let mut ts = textstroke("• a");
        ts.text_style.ranged_text_attributes = vec![
            RangedTextAttribute {
                range: 0.."• a".len(),
                attribute: TextAttribute::Underline(true),
            },
            RangedTextAttribute {
                range: "• a".len().."• a".len(),
                attribute: TextAttribute::Underline(true),
            },
        ];
        let mut positions = [ts.text.len()];

        ts.replace_range_mapped(0..0, "x", &mut positions);
        assert_eq!(positions, [ts.text.len()]);
        assert_eq!(ts.text_style.ranged_text_attributes.len(), 1);
        assert_eq!(
            ts.text_style.ranged_text_attributes[0].range,
            1..ts.text.len()
        );

        ts.text_style
            .ranged_text_attributes
            .push(RangedTextAttribute {
                range: ts.text.len()..ts.text.len() + 2,
                attribute: TextAttribute::Underline(true),
            });
        ts.merge_adjacent_attrs();
        assert_eq!(ts.text_style.ranged_text_attributes.len(), 1);
        assert_eq!(
            ts.text_style.ranged_text_attributes[0].range,
            1..ts.text.len() + 2
        );
    }

    #[test]
    fn wrapped_items_get_a_hanging_indent() {
        let mut ts = textstroke("");
        ts.text_style.set_max_width(Some(300.0));
        ts.text = format!(
            "• {}\nplain {}",
            "word ".repeat(20).trim_end(),
            "word ".repeat(20).trim_end()
        );
        let mut cursor = cursor_at(&ts, ts.text.len());

        assert!(ts.update_lists(&mut cursor, None, None));
        assert!(ts.text.contains(SOFT_BREAK));
        // Only the list item gets automatic line breaks.
        assert!(!ts.text.split('\n').nth(1).unwrap().contains(SOFT_BREAK));
        assert_eq!(cursor.cur_cursor(), ts.text.len());

        // The wrapped lines of the item start at the same x as its text.
        let text_layout = ts
            .text_style
            .build_text_layout(&mut piet_cairo::CairoText::new(), ts.text.clone())
            .unwrap();
        let content_x = text_layout.hit_test_text_position("• ".len()).point.x;
        for (start, _) in ts.text.match_indices(SOFT_BREAK) {
            let line_content = soft_break_before(&ts.text, start + SOFT_BREAK.len_utf8())
                .unwrap()
                .end;
            let x = text_layout.hit_test_text_position(line_content).point.x;
            assert!((x - content_x).abs() < 2.0, "{x} vs {content_x}");
        }

        // Updating again does not change anything, and the clipboard text is the original one.
        let text = ts.text.clone();
        assert!(!ts.update_lists(&mut cursor, None, None));
        assert_eq!(ts.text, text);
        assert!(
            ts.text_without_soft_breaks(0..ts.text.len())
                .starts_with(&format!("• {}", "word ".repeat(20).trim_end()))
        );
    }
}
