//! Right-to-left and mixed text. A text's direction is that of its first strong letter (Hebrew or Arabic: right to
//! left; Latin and the rest: left to right; a text of none goes left to right), as the browser's `dir="auto"` finds
//! it and as the text's shaper lays each of its lines out. The chat's parts use it to stand a right-to-left text on
//! the right: the box and the bubbles lay each line out by its own words, an answer's blocks by their first strong
//! letters (a list by its first item), code always left to right, and a title set among other words is isolated so
//! it does not reorder them. What every part has (`register`, `take`) is said in `ext.rs`.

use std::rc::Rc;
use std::sync::LazyLock;

use floem::IntoView;
use floem::peniko::Color;
use floem::style::{AlignItems, Style};
use floem::text::{Attrs, AttrsList, FamilyOwned, LineHeightValue, TextLayout};
use floem::unit::PxPctAuto;
use floem::views::{Decorators, label, rich_text};
use serde_json::Value;
use unicode_bidi::{Direction, get_base_direction, get_base_direction_full};

use crate::markdown::Block;
use crate::state::{Send, Ui};
use crate::widgets::UI_FONT;

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

static UI: LazyLock<Vec<FamilyOwned>> = LazyLock::new(|| FamilyOwned::parse_list(&UI_FONT).collect());

/// How a text of the chat is drawn: its size, its line height (times the size), its colour.
#[derive(Clone, Copy)]
pub struct Look {
    pub size: f32,
    pub line_height: f32,
    pub color: Color,
}

/// A text of several lines, each laid out by its own words (the browser's `dir="auto"` with
/// `unicode-bidi: plaintext`): a right-to-left line stands on the right of the text's width, a left-to-right one on
/// its left. A text with no right-to-left line is a label as before (it can be selected); one with such a line is
/// laid out to its widest line's width, or to its place's when it wraps. Its style wraps it (`TextOverflow::Wrap`).
pub fn lines(words: impl Fn() -> String + 'static, look: impl Fn() -> Look + 'static) -> floem::AnyView {
    if !floem::reactive::untrack(|| any_rtl_line(&words())) {
        return label(words)
            .style(move |s| {
                let l = look();
                s.font_size(l.size).line_height(l.line_height).color(l.color)
            })
            .into_any();
    }
    rich_text(move || laid(&words(), look())).into_any()
}

/// A text laid out to its widest line's width, so that each line stands on the side of its own direction.
fn laid(words: &str, l: Look) -> TextLayout {
    let attrs =
        Attrs::new().family(&UI).font_size(l.size).line_height(LineHeightValue::Normal(l.line_height)).color(l.color);
    let mut layout = TextLayout::new();
    layout.set_text(words, AttrsList::new(attrs));
    let widest = layout.size().width.ceil() as f32 + 1.0;
    layout.set_size(widest, f32::MAX);
    layout
}

/// The left-to-right mark: an invisible strong letter that sets the direction of the line it starts.
pub const LRM: char = '\u{200E}';

/// Whether a text goes right to left: its first strong letter (outside isolated runs, over all its lines until one
/// has such a letter) is Hebrew or Arabic.
pub fn rtl(text: &str) -> bool {
    get_base_direction_full(text) == Direction::Rtl
}

/// Whether a line goes right to left, by its own first strong letter (a line of none goes left to right).
pub fn rtl_line(line: &str) -> bool {
    get_base_direction(line) == Direction::Rtl
}

/// Whether any of a text's lines goes right to left (the shaper then lays that line out from the right).
pub fn any_rtl_line(text: &str) -> bool {
    text.split('\n').any(rtl_line)
}

/// A title set among other words, isolated (first strong isolate … pop directional isolate): its own direction is
/// found from its words and it does not reorder the words around it. A title of no right-to-left letter is as it was.
pub fn isolate(title: &str) -> String {
    if title
        .chars()
        .any(|c| matches!(unicode_bidi::bidi_class(c), unicode_bidi::BidiClass::R | unicode_bidi::BidiClass::AL))
    {
        format!("\u{2068}{title}\u{2069}")
    } else {
        title.to_string()
    }
}

/// A line of code as it is laid out: left to right always. A line that would start right to left (a comment in
/// Persian) is led by the left-to-right mark, and the runs' ranges are then moved on by its length; any other line
/// is as it was, with no shift.
pub fn ltr_line(line: &str) -> (String, usize) {
    if rtl_line(line) {
        let mut out = String::with_capacity(line.len() + LRM.len_utf8());
        out.push(LRM);
        out.push_str(line);
        (out, LRM.len_utf8())
    } else {
        (line.to_string(), 0)
    }
}

/// The direction of the list a block's item is in: that of the list's first item, the first one before it at its
/// depth with only deeper items (or further paragraphs of items) between, and of its kind (a numbered list after a
/// bulleted one is a list of its own). Its bullets stand on that side; each item's
/// words take their own direction.
pub fn list_rtl(blocks: &[Rc<Block>], n: usize) -> bool {
    // An item's depth, its kind (None: a further paragraph of the item before it; else whether it is numbered), its
    // words.
    fn item(b: &Block) -> Option<(u8, Option<bool>, &str)> {
        match b {
            Block::Item { depth, marker, text } => {
                let kind = (!marker.is_empty()).then(|| marker.starts_with(|c: char| c.is_ascii_digit()));
                Some((*depth, kind, text.text.as_str()))
            }
            _ => None,
        }
    }
    let Some((mine, mut kind, words)) = blocks.get(n).and_then(|b| item(b)) else { return false };
    let mut first = words;
    for b in blocks[..n].iter().rev() {
        match item(b) {
            Some((depth, Some(k), words)) if depth == mine && kind.is_none_or(|kind| kind == k) => {
                kind = Some(k);
                first = words;
            }
            Some((depth, Some(_), _)) if depth == mine => break,
            Some((depth, _, _)) if depth >= mine => {}
            _ => break,
        }
    }
    rtl(first)
}

/// A text's style in a column, stood on the right when it goes right to left: its natural width up to the column's
/// (it wraps there). A left-to-right one is as its style was.
pub fn stand(s: Style, rtl: bool) -> Style {
    if rtl { s.width(PxPctAuto::Auto).max_width_full().align_self(Some(AlignItems::FlexEnd)) } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::Text;

    #[test]
    fn a_text_goes_the_way_of_its_first_strong_letter() {
        assert!(rtl("این کد را توضیح بده"));
        assert!(rtl("שלום world"));
        assert!(!rtl("Hello سلام"));
        assert!(!rtl(""));
        assert!(!rtl("123 ..."));
        // Digits and marks are not strong: the first letter is.
        assert!(rtl("1. سلام"));
        // A line of no strong letter leaves it to the next.
        assert!(rtl("...\nسلام"));
        // An isolated run is not looked in.
        assert!(!rtl("\u{2068}سلام\u{2069} done"));
    }

    #[test]
    fn each_line_goes_its_own_way() {
        let asked = "این کد را توضیح بده\nwhy does it fail?";
        let lines: Vec<bool> = asked.split('\n').map(rtl_line).collect();
        assert_eq!(lines, [true, false]);
        assert!(any_rtl_line(asked));
        assert!(!any_rtl_line("why\ndoes it fail?"));
        assert!(!rtl_line("..."));
    }

    #[test]
    fn a_title_is_isolated_only_when_it_has_right_to_left_letters() {
        assert_eq!(isolate("Fix the build"), "Fix the build");
        assert_eq!(isolate("کد"), "\u{2068}کد\u{2069}");
    }

    #[test]
    fn code_stays_left_to_right() {
        assert_eq!(ltr_line("let x = 1; // one"), ("let x = 1; // one".to_string(), 0));
        let (line, shift) = ltr_line("// توضیح");
        assert_eq!(shift, 3);
        assert!(line.starts_with(LRM));
        assert!(!rtl_line(&line));
    }

    #[test]
    fn a_list_goes_the_way_of_its_first_item() {
        let item = |depth: u8, marker: &str, words: &str| {
            Rc::new(Block::Item { depth, marker: marker.into(), text: Text { text: words.into(), ..Text::default() } })
        };
        let para = |words: &str| Rc::new(Block::Paragraph(Text { text: words.into(), ..Text::default() }));
        let blocks = vec![
            para("این تابع"),
            item(0, "•", "First, the parse call"),
            item(0, "•", "سپس خطا"),
            item(1, "•", "یک"),
            item(1, "•", "two"),
            item(0, "", "بیشتر"),
            item(0, "•", "last"),
            item(0, "1.", "یک"),
            item(0, "2.", "two"),
            para("after"),
            item(0, "1.", "سلام"),
            item(0, "2.", "hello"),
        ];
        // The first list begins in English: all its items' bullets stand on the left.
        assert!(!list_rtl(&blocks, 1));
        assert!(!list_rtl(&blocks, 2));
        assert!(!list_rtl(&blocks, 6));
        assert!(!list_rtl(&blocks, 5));
        // The nested list begins in Persian.
        assert!(list_rtl(&blocks, 3));
        assert!(list_rtl(&blocks, 4));
        // A numbered list right after a bulleted one is a list of its own.
        assert!(list_rtl(&blocks, 7));
        assert!(list_rtl(&blocks, 8));
        // A list after a paragraph is a list of its own.
        assert!(list_rtl(&blocks, 10));
        assert!(list_rtl(&blocks, 11));
        // Not an item.
        assert!(!list_rtl(&blocks, 0));
    }
}
