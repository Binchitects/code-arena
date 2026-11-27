//! A question asked over the window, as VS Code's dialog: what is asked, what follows from it, a box to tick, and
//! the buttons, the first of them the one Enter takes unless another is said. It is drawn over the whole of the
//! window it is asked in (a view may float in a window of its own), dimmed, and has the keys until it is answered:
//! Enter takes the button lit, Tab and the arrows light another, Space ticks the box, Escape cancels.
//!
//! ```ignore
//! dialog::ask("Are you sure you want to delete 'notes.txt'?")
//!     .detail("You can restore this file from the backups.")
//!     .button("Move to Backups")
//!     .back(tree_id)
//!     .open(ui, move |answer| if answer.yes() { delete() });
//! dialog::ask("Undo the last commit?").kind(Kind::Warning).button("Undo").destructive().open(ui, |a| …);
//! dialog::ask("Move 'a.rs' into 'src'?").button("Move").checkbox("Do not ask me again", false).open(ui, |a| …);
//! dialog::ask("The file was saved.").kind(Kind::Info).button("OK").no_cancel().open(ui, |_| {});
//! ```
//!
//! The buttons are the ones named, in their order, then Cancel (unless `no_cancel`). The answer says which button
//! was taken (none: cancelled) and whether the box is ticked. It is given once, after the dialog has closed and the
//! keys are back where `back` says. One dialog at a time: a second cancels the first, and so does a quick pick
//! opened while it is asked (a quick pick open when it is asked answers none). It may be asked from anywhere (a
//! click, a key, a quick pick's answer, a message taken in); a press outside its box does nothing.

// The parts to come call what the window's first parts do not yet.
#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;

use floem::action::remove_overlay;
use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::{CursorStyle, FlexWrap, TextOverflow};
use floem::views::{Decorators, container, empty, h_stack, h_stack_from_iter, label, v_stack};
use floem::{AnyView, IntoView, ViewId};
use serde_json::Value;

use crate::icons::icon;
use crate::state::{Send, Ui};
use crate::widgets::{cover, solid, window_size};

/// What a dialog is about: the icon before what it asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// No icon: a plain question.
    Question,
    Info,
    Warning,
    Error,
}

/// A button of a dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Button {
    label: String,
    /// It takes something away for good: drawn in the colour of an error.
    destructive: bool,
}

/// A question, as it is being made.
pub struct Dialog {
    message: String,
    detail: String,
    kind: Kind,
    buttons: Vec<Button>,
    /// The button Enter takes when the dialog opens.
    default: usize,
    /// The words of the button that cancels; none: there is no such button (Escape still cancels).
    cancel: Option<String>,
    checkbox: Option<(String, bool)>,
    back: Option<ViewId>,
}

/// What was answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Answer {
    /// The button taken, by its place among the ones named; none: cancelled.
    pub button: Option<usize>,
    /// Whether the box was ticked (false when the dialog has none).
    pub checked: bool,
}

impl Answer {
    /// The first button was taken.
    pub fn yes(&self) -> bool {
        self.button == Some(0)
    }
}

/// A question to ask: what it asks is its first line, in larger letters.
pub fn ask(message: &str) -> Dialog {
    Dialog {
        message: message.into(),
        detail: String::new(),
        kind: Kind::Question,
        buttons: Vec::new(),
        default: 0,
        cancel: Some("Cancel".into()),
        checkbox: None,
        back: None,
    }
}

impl Dialog {
    /// What follows from the answer, under the question.
    pub fn detail(mut self, detail: &str) -> Dialog {
        self.detail = detail.into();
        self
    }

    pub fn kind(mut self, kind: Kind) -> Dialog {
        self.kind = kind;
        self
    }

    /// One more button, after the ones named before it.
    pub fn button(mut self, label: &str) -> Dialog {
        self.buttons.push(Button { label: label.into(), destructive: false });
        self
    }

    /// The button just named takes something away for good.
    pub fn destructive(mut self) -> Dialog {
        if let Some(b) = self.buttons.last_mut() {
            b.destructive = true;
        }
        self
    }

    /// The button just named is the one Enter takes (the first is, when none is said).
    pub fn default(mut self) -> Dialog {
        self.default = self.buttons.len().saturating_sub(1);
        self
    }

    /// Other words on the button that cancels.
    pub fn cancel(mut self, label: &str) -> Dialog {
        self.cancel = Some(label.into());
        self
    }

    /// No button that cancels (a dialog that only says something). Escape still closes it, as cancelled.
    pub fn no_cancel(mut self) -> Dialog {
        self.cancel = None;
        self
    }

    /// A box to tick under the question ("Do not ask me again"), ticked or not at first.
    pub fn checkbox(mut self, label: &str, on: bool) -> Dialog {
        self.checkbox = Some((label.into(), on));
        self
    }

    /// The view it is asked from: the dialog is drawn in that view's window, and the view has the keys again once
    /// it is answered.
    pub fn back(mut self, to: ViewId) -> Dialog {
        self.back = Some(to);
        self
    }

    /// The buttons as drawn: the ones named, then the one that cancels. A dialog with none named has "OK".
    fn drawn(&self) -> Vec<Button> {
        let mut all = self.buttons.clone();
        if all.is_empty() {
            all.push(Button { label: "OK".into(), destructive: false });
        }
        if let Some(cancel) = &self.cancel {
            all.push(Button { label: cancel.clone(), destructive: false });
        }
        all
    }

    /// Asks it. `done` is given the answer once.
    pub fn open(self, ui: Ui, done: impl FnOnce(Answer) + 'static) {
        let state = ui.state::<Open>();
        // One at a time: the one open is cancelled first. A menu open closes, and a quick pick answers none.
        answer(ui, None);
        crate::widgets::popup::close(ui);
        crate::widgets::quick::cancel(ui);
        let buttons = self.drawn();
        let b = Buttons { named: buttons.len() - usize::from(self.cancel.is_some()), total: buttons.len() };
        let has_box = self.checkbox.is_some();
        state.lit.set(Lit::Button(self.default.min(b.named.saturating_sub(1))));
        state.checked.set(self.checkbox.as_ref().is_some_and(|(_, on)| *on));
        state.back.set(self.back);
        state.done.set(Some(Rc::new(RefCell::new(Some(Box::new(done))))));
        let size = window_size(ui, self.back);
        let key = move |key: &Key, m: Modifiers| {
            let Some(pressed) = press_of(key, m) else { return };
            let mut lit = state.lit.get_untracked();
            let did = press(b, has_box, &mut lit, pressed);
            if state.lit.get_untracked() != lit {
                state.lit.set(lit);
            }
            match did {
                Did::Nothing => {}
                Did::Tick => state.checked.update(|on| *on = !*on),
                Did::Answer(button) => answer(ui, button),
            }
        };
        let id = cover(
            ui,
            size,
            true,
            key,
            || {},
            move |keep| {
                container(solid(body(ui, state, self, buttons, b, size.width), keep))
                    .style(|s| s.absolute().inset(0.0).size_full().items_center().justify_center())
                    .into_any()
            },
        );
        state.layer.set(Some(id));
    }
}

/// The open dialog's buttons, counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Buttons {
    /// The buttons that answer with their place.
    pub named: usize,
    /// All of them: one more than `named` when there is a button that cancels.
    pub total: usize,
}

/// What is lit in a dialog: the box to tick, or one of its buttons (by its place among all of them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lit {
    Check,
    Button(usize),
}

/// A key pressed in a dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    /// Tab, Right or Down: the next of the box and the buttons, around the end.
    Next,
    /// Shift+Tab, Left or Up: the one before.
    Before,
    /// Enter: the button lit (the first, while the box is lit).
    Take,
    /// Space: the box ticked, or the button lit.
    Space,
    /// Escape.
    Leave,
}

/// What a key did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Did {
    Nothing,
    /// The box is ticked, or no longer.
    Tick,
    /// Answered with a button named, or cancelled.
    Answer(Option<usize>),
}

fn press_of(key: &Key, m: Modifiers) -> Option<Press> {
    Some(match key {
        Key::Named(NamedKey::Tab) if m.contains(Modifiers::SHIFT) => Press::Before,
        Key::Named(NamedKey::Tab | NamedKey::ArrowRight | NamedKey::ArrowDown) => Press::Next,
        Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowUp) => Press::Before,
        Key::Named(NamedKey::Enter) => Press::Take,
        Key::Named(NamedKey::Space) => Press::Space,
        Key::Named(NamedKey::Escape) => Press::Leave,
        _ => return None,
    })
}

/// The answer a button gives: its place when it is one of the named, cancelled when it is the one after them.
fn answer_of(b: Buttons, button: usize) -> Option<usize> {
    (button < b.named).then_some(button)
}

/// A key pressed: what is lit moves among the box and the buttons, the box is ticked, or the dialog is answered.
pub fn press(b: Buttons, has_box: bool, lit: &mut Lit, key: Press) -> Did {
    // The stops in the order Tab goes through them: the box, then each button.
    let first = usize::from(has_box);
    let stops = first + b.total;
    let at = match *lit {
        Lit::Check => 0,
        Lit::Button(i) => first + i.min(b.total.saturating_sub(1)),
    };
    let of = |stop: usize| if has_box && stop == 0 { Lit::Check } else { Lit::Button(stop - first) };
    match key {
        Press::Next if stops > 0 => *lit = of((at + 1) % stops),
        Press::Before if stops > 0 => *lit = of((at + stops - 1) % stops),
        Press::Next | Press::Before => {}
        Press::Take => {
            return match *lit {
                // Enter while the box is lit: the first button, as VS Code's.
                Lit::Check => Did::Answer(answer_of(b, 0)),
                Lit::Button(i) => Did::Answer(answer_of(b, i)),
            };
        }
        Press::Space => {
            return match *lit {
                Lit::Check => Did::Tick,
                Lit::Button(i) => Did::Answer(answer_of(b, i)),
            };
        }
        Press::Leave => return Did::Answer(None),
    }
    Did::Nothing
}

/// What `done` is kept in until the dialog is answered: taken out once.
type Done = Rc<RefCell<Option<Box<dyn FnOnce(Answer)>>>>;

/// The dialog that is open: its layer over the window, what is lit, the box, and who is told the answer.
#[derive(Clone, Copy)]
struct Open {
    layer: RwSignal<Option<ViewId>>,
    lit: RwSignal<Lit>,
    checked: RwSignal<bool>,
    back: RwSignal<Option<ViewId>>,
    done: RwSignal<Option<Done>>,
}

impl crate::ext::State for Open {
    fn new(ui: Ui) -> Open {
        Open {
            layer: ui.scope.create_rw_signal(None),
            lit: ui.scope.create_rw_signal(Lit::Button(0)),
            checked: ui.scope.create_rw_signal(false),
            back: ui.scope.create_rw_signal(None),
            done: ui.scope.create_rw_signal(None),
        }
    }
}

/// Whether a dialog is open.
pub fn is_open(ui: Ui) -> bool {
    ui.state::<Open>().done.with_untracked(|d| d.is_some())
}

/// The open dialog answered (none: cancelled): it closes, the keys go back, then its asker is told.
fn answer(ui: Ui, button: Option<usize>) {
    let state = ui.state::<Open>();
    let Some(done) = state.done.get_untracked() else { return };
    state.done.set(None);
    if let Some(id) = state.layer.get_untracked() {
        state.layer.set(None);
        remove_overlay(id);
    }
    if let Some(back) = state.back.get_untracked() {
        back.request_focus();
    }
    let told = done.borrow_mut().take();
    if let Some(told) = told {
        told(Answer { button, checked: state.checked.get_untracked() });
    }
}

/// The open dialog cancelled, if one is open (a part that goes away while it asks).
pub fn cancel(ui: Ui) {
    answer(ui, None);
}

/// The dialog's box: the icon and the question, what follows from it, the box to tick, the buttons at the right.
fn body(ui: Ui, state: Open, d: Dialog, buttons: Vec<Button>, b: Buttons, window: f64) -> impl IntoView {
    let t = ui.colors;
    let Dialog { message, detail, kind, checkbox, .. } = d;
    let mark = |name: &'static str| icon(name, 28.0).style(|s| s.margin_right(12.0));
    let mark: AnyView = match kind {
        Kind::Question => empty().into_any(),
        Kind::Info => mark("info").style(move |s| s.color(t.get().notify_info)).into_any(),
        Kind::Warning => mark("warning").style(move |s| s.color(t.get().notify_warning)).into_any(),
        Kind::Error => mark("error").style(move |s| s.color(t.get().notify_error)).into_any(),
    };
    let has_detail = !detail.is_empty();
    let words = v_stack((
        label(move || message.clone()).style(|s| s.font_size(18.0).line_height(1.25).text_overflow(TextOverflow::Wrap)),
        label(move || detail.clone()).style(move |s| {
            s.margin_top(10.0).line_height(1.4).text_overflow(TextOverflow::Wrap).apply_if(!has_detail, |s| s.hide())
        }),
    ))
    .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0));
    let tick: AnyView = match checkbox {
        None => empty().into_any(),
        Some((text, _)) => h_stack((
            container(icon("check", 14.0).style(move |s| s.apply_if(!state.checked.get(), |s| s.hide()))).style(
                move |s| {
                    let c = t.get();
                    let lit = state.lit.get() == Lit::Check;
                    s.width(18.0)
                        .height(18.0)
                        .items_center()
                        .justify_center()
                        .border(1.0)
                        .border_radius(3.0)
                        .border_color(if lit { c.focus } else { c.checkbox_border })
                        .background(c.checkbox_bg)
                        .color(c.checkbox_fg)
                },
            ),
            label(move || text.clone()).style(|s| s.margin_left(8.0)),
        ))
        .on_click_stop(move |_| {
            state.checked.update(|on| *on = !*on);
            if state.lit.get_untracked() != Lit::Check {
                state.lit.set(Lit::Check);
            }
        })
        .style(|s| s.margin_top(14.0).items_center().cursor(CursorStyle::Pointer))
        .into_any(),
    };
    let row = h_stack_from_iter(buttons.into_iter().enumerate().map(move |(i, button)| {
        let Button { label: text, destructive } = button;
        label(move || text.clone()).on_click_stop(move |_| answer(ui, answer_of(b, i))).style(move |s| {
            let c = t.get();
            let lit = state.lit.get() == Lit::Button(i);
            // The first button is VS Code's first button; the others, and the one that cancels, its second.
            let (bg, fg, hover) = if destructive {
                (c.danger_bg, c.danger_fg, c.danger_hover)
            } else if i == 0 {
                (c.button_bg, c.button_fg, c.button_hover)
            } else {
                (c.button2_bg, c.button2_fg, c.button2_hover)
            };
            s.padding_horiz(14.0)
                .padding_vert(4.0)
                .border_radius(4.0)
                .font_size(13.0)
                .line_height(1.4)
                .background(bg)
                .color(fg)
                .cursor(CursorStyle::Pointer)
                .outline(if lit { 1.0 } else { 0.0 })
                .outline_color(c.focus)
                .hover(|s| s.background(hover))
        })
    }))
    .style(|s| s.margin_top(20.0).gap(8.0).justify_end().flex_wrap(FlexWrap::Wrap));
    v_stack((h_stack((mark, words)).style(|s| s.items_start()), tick, row)).style(move |s| {
        let c = t.get();
        s.width(450.0_f64.min(window - 40.0).max(120.0))
            .padding(20.0)
            .border(1.0)
            .border_color(c.widget_border)
            .border_radius(6.0)
            .background(c.widget)
            .color(c.fg)
            .box_shadow_blur(16.0)
            .box_shadow_color(c.shadow)
            .selectable(false)
    })
}

/// What it adds to the window's registries, once at start: nothing (a dialog is asked by the part it is of).
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are this part's, taken in: none are.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    const TWO: Buttons = Buttons { named: 1, total: 2 };

    #[test]
    fn the_buttons_are_the_ones_named_then_the_one_that_cancels() {
        let labels = |d: Dialog| d.drawn().into_iter().map(|b| b.label).collect::<Vec<_>>();
        assert_eq!(labels(ask("Delete?").button("Move to Backups")), ["Move to Backups", "Cancel"]);
        assert_eq!(labels(ask("Save?").button("Save").button("Don't Save")), ["Save", "Don't Save", "Cancel"]);
        assert_eq!(labels(ask("Saved.").button("OK").no_cancel()), ["OK"]);
        assert_eq!(labels(ask("Saved.")), ["OK", "Cancel"], "none named: OK");
        assert_eq!(labels(ask("Leave?").button("Leave").cancel("Stay")), ["Leave", "Stay"]);
        let d = ask("Undo?").button("Keep").button("Undo").destructive().default();
        assert_eq!((d.default, d.drawn()[1].destructive, d.drawn()[0].destructive), (1, true, false));
    }

    #[test]
    fn enter_takes_the_button_lit_and_escape_cancels() {
        let mut lit = Lit::Button(0);
        assert_eq!(press(TWO, false, &mut lit, Press::Take), Did::Answer(Some(0)));
        assert_eq!(press(TWO, false, &mut lit, Press::Leave), Did::Answer(None));
        assert_eq!(press(TWO, false, &mut lit, Press::Next), Did::Nothing);
        assert_eq!(lit, Lit::Button(1));
        assert_eq!(press(TWO, false, &mut lit, Press::Take), Did::Answer(None), "the button that cancels");
        assert_eq!(press(TWO, false, &mut lit, Press::Space), Did::Answer(None));
        assert_eq!(press(TWO, false, &mut lit, Press::Next), Did::Nothing);
        assert_eq!(lit, Lit::Button(0), "around the end");
        assert_eq!(press(TWO, false, &mut lit, Press::Before), Did::Nothing);
        assert_eq!(lit, Lit::Button(1), "and around the beginning");
        // Three named, none that cancels: each answers with its place.
        let three = Buttons { named: 3, total: 3 };
        let mut lit = Lit::Button(2);
        assert_eq!(press(three, false, &mut lit, Press::Take), Did::Answer(Some(2)));
        assert_eq!(press(three, false, &mut lit, Press::Leave), Did::Answer(None));
    }

    #[test]
    fn the_box_is_a_stop_before_the_buttons_and_space_ticks_it() {
        let mut lit = Lit::Button(0);
        assert_eq!(press(TWO, true, &mut lit, Press::Before), Did::Nothing);
        assert_eq!(lit, Lit::Check);
        assert_eq!(press(TWO, true, &mut lit, Press::Space), Did::Tick);
        assert_eq!(press(TWO, true, &mut lit, Press::Take), Did::Answer(Some(0)), "Enter is the first button's");
        assert_eq!(press(TWO, true, &mut lit, Press::Before), Did::Nothing);
        assert_eq!(lit, Lit::Button(1), "around");
        assert_eq!(press(TWO, true, &mut lit, Press::Next), Did::Nothing);
        assert_eq!(lit, Lit::Check);
        assert_eq!(press(TWO, true, &mut lit, Press::Next), Did::Nothing);
        assert_eq!(lit, Lit::Button(0));
    }

    #[test]
    fn the_keys_are_vs_codes() {
        let none = Modifiers::empty();
        assert_eq!(press_of(&Key::Named(NamedKey::Tab), none), Some(Press::Next));
        assert_eq!(press_of(&Key::Named(NamedKey::Tab), Modifiers::SHIFT), Some(Press::Before));
        assert_eq!(press_of(&Key::Named(NamedKey::ArrowLeft), none), Some(Press::Before));
        assert_eq!(press_of(&Key::Named(NamedKey::ArrowRight), none), Some(Press::Next));
        assert_eq!(press_of(&Key::Named(NamedKey::Enter), none), Some(Press::Take));
        assert_eq!(press_of(&Key::Named(NamedKey::Space), none), Some(Press::Space));
        assert_eq!(press_of(&Key::Named(NamedKey::Escape), none), Some(Press::Leave));
        assert_eq!(press_of(&Key::Character("a".into()), none), None);
    }

    #[test]
    fn an_answer_is_given_once_and_a_second_dialog_cancels_the_first() {
        let ui = Ui::new();
        let state = ui.state::<Open>();
        let said = Rc::new(RefCell::new(Vec::<Answer>::new()));
        let keep = |said: &Rc<RefCell<Vec<Answer>>>| {
            let said = said.clone();
            move |a: Answer| said.borrow_mut().push(a)
        };
        // As `open` leaves it, without the layer (there is no window here).
        let asked = |done: Box<dyn FnOnce(Answer)>, checked: bool| {
            answer(ui, None);
            state.checked.set(checked);
            state.done.set(Some(Rc::new(RefCell::new(Some(done)))));
        };
        assert!(!is_open(ui));
        asked(Box::new(keep(&said)), true);
        assert!(is_open(ui));
        answer(ui, Some(0));
        assert!(!is_open(ui));
        answer(ui, Some(1));
        assert_eq!(*said.borrow(), [Answer { button: Some(0), checked: true }], "told once");
        assert!(said.borrow()[0].yes());
        // A second asked while the first is open: the first is cancelled.
        let second = Rc::new(Cell::new(None));
        asked(Box::new(keep(&said)), false);
        let to = second.clone();
        asked(Box::new(move |a| to.set(Some(a))), false);
        assert_eq!(said.borrow().len(), 2);
        assert_eq!(said.borrow()[1], Answer { button: None, checked: false });
        assert!(second.get().is_none() && is_open(ui));
        cancel(ui);
        assert_eq!(second.get(), Some(Answer { button: None, checked: false }));
    }
}
