//! A dialog with fields, as VS Code's and the old Code Arena's: a box over the dimmed window with its title, what it
//! is for, the fields (inputs, a box to tick, a choice of one), and its buttons at the right. A part makes one
//! `Form` in its state, draws its layer from its `register` with `host`, and opens it with `show`:
//!
//! ```ignore
//! layers::add(Place::Over, "goal.form", 30, |ui, send| goal_form(ui, send));
//! st.form.show(); // the first field has the keys
//! ```
//!
//! The keys: each input has them while it has the focus (no command of the window's runs), Tab and Shift+Tab go
//! from one input to the next, Enter takes the form's first button, and Escape closes it, as a press on the dimmed
//! window does. A press in the box that is not in an input leaves the box the keys, so Escape still closes it.

use std::time::Duration;

use floem::action::exec_after;
use floem::event::{Event, EventListener, EventPropagation};
use floem::keyboard::{Key, NamedKey};
use floem::peniko::Brush;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::{CursorStyle, Position, TextOverflow};
use floem::views::{Decorators, container, empty, h_stack, label, stack, text_input, v_stack, v_stack_from_iter};
use floem::{AnyView, IntoView, View as _, ViewId};

use crate::icons::icon;
use crate::keymap::{Focus, TakesKeys};
use crate::state::Ui;
use crate::widgets::mix;

/// A form's own signals; Copy, so its parts' closures hold it.
#[derive(Clone, Copy)]
pub struct Form {
    /// Whether it is open.
    pub open: RwSignal<bool>,
    /// Its inputs, in the order Tab goes through them.
    fields: RwSignal<Vec<ViewId>>,
    /// Its box, which has the keys when no input has them.
    boxed: RwSignal<Option<ViewId>>,
    /// How many of its views have the focus (0 once Escape took it from an input).
    inside: RwSignal<i32>,
    /// What had the keys before it opened.
    back: RwSignal<Option<ViewId>>,
    /// How many times it was shown: a lost focus closes only the showing it was lost in, never one opened again
    /// before the check (a sign-in asked again as soon as the last was answered).
    shown: RwSignal<u64>,
}

impl Form {
    pub fn new(ui: Ui) -> Form {
        Form {
            open: ui.scope.create_rw_signal(false),
            fields: ui.scope.create_rw_signal(Vec::new()),
            boxed: ui.scope.create_rw_signal(None),
            inside: ui.scope.create_rw_signal(0),
            back: ui.scope.create_rw_signal(None),
            shown: ui.scope.create_rw_signal(0),
        }
    }

    /// Opens it, its first input with the keys once it is drawn; `back` has them again when it closes.
    pub fn show(self, back: Option<ViewId>) {
        self.back.set(back);
        self.shown.update(|n| *n += 1);
        if !self.open.get_untracked() {
            self.open.set(true);
        }
        exec_after(Duration::from_millis(16), move |_| {
            if let Some(first) = self.fields.with_untracked(|f| f.first().copied()) {
                first.request_focus();
            } else if let Some(b) = self.boxed.get_untracked() {
                b.request_focus();
            }
        });
    }

    /// Closes it; the keys go back.
    pub fn close(self) {
        if self.open.get_untracked() {
            self.open.set(false);
        }
        if let Some(back) = self.back.get_untracked() {
            back.request_focus();
        }
    }

    fn gained(self) {
        self.inside.update(|n| *n += 1);
    }

    /// A view of the form lost the keys: when none of its views has them a moment later (Escape in an input), it
    /// closes.
    fn lost(self) {
        self.inside.update(|n| *n = (*n - 1).max(0));
        let at = self.shown.get_untracked();
        exec_after(Duration::ZERO, move |_| {
            if self.open.get_untracked() && self.inside.get_untracked() == 0 && self.shown.get_untracked() == at {
                self.close();
            }
        });
    }

    /// The box has the keys (a press in it, not in an input).
    fn hold(self) {
        if let Some(b) = self.boxed.get_untracked() {
            b.request_focus();
        }
    }

    /// A one-line input of the form: `enter` is what Enter does in it.
    pub fn input(self, ui: Ui, text: RwSignal<String>, hint: &'static str, enter: impl Fn() + 'static) -> AnyView {
        let t = ui.colors;
        let focused = ui.scope.create_rw_signal(false);
        let view = text_input(text).placeholder(hint);
        let id = view.id();
        self.fields.update(|f| f.push(id));
        view.on_event(EventListener::KeyDown, move |e| {
            let Event::KeyDown(k) = e else { return EventPropagation::Continue };
            match &k.key.logical_key {
                Key::Named(NamedKey::Enter) => enter(),
                Key::Named(NamedKey::Tab) => {
                    // The inputs shown now (one may be hidden by a choice), the next or the one before.
                    let all: Vec<ViewId> = self
                        .fields
                        .get_untracked()
                        .into_iter()
                        .filter(|f| *f == id || !f.is_hidden_recursive())
                        .collect();
                    if let Some(i) = all.iter().position(|f| *f == id) {
                        let n = all.len();
                        let to = if k.modifiers.shift() { (i + n - 1) % n } else { (i + 1) % n };
                        all[to].request_focus();
                    }
                }
                _ => return EventPropagation::Continue,
            }
            EventPropagation::Stop
        })
        .on_event_cont(EventListener::FocusGained, move |_| {
            focused.set(true);
            self.gained();
        })
        .on_event_cont(EventListener::FocusLost, move |_| {
            focused.set(false);
            self.lost();
        })
        .on_event_stop(EventListener::PointerDown, |_| {})
        .takes_keys(Focus::Input)
        .style(move |s| {
            let c = t.get();
            let s = s
                .width_full()
                .min_width(0.0)
                .height(26.0)
                .padding_horiz(6.0)
                .border(1.0)
                .border_radius(2.0)
                .border_color(if focused.get() { c.focus } else { c.input_border })
                .background(c.input_bg)
                .color(c.input_fg)
                .cursor_color(Brush::Solid(c.cursor));
            crate::widgets::steady_input(s, c.input_bg, Some(c.focus))
        })
        .into_any()
    }

    /// A one-line input whose text is a secret (a password): drawn as dots, never in a text box, which would draw
    /// it and copy it. Its keys are taken here; `enter` is what Enter does in it.
    pub fn secret(self, ui: Ui, text: RwSignal<String>, enter: impl Fn() + 'static) -> AnyView {
        let t = ui.colors;
        let focused = ui.scope.create_rw_signal(false);
        let dots = h_stack((
            label(move || "\u{2022}".repeat(text.with(|x| x.chars().count()).min(80))),
            empty().style(move |s| {
                s.width(1.0).height(16.0).background(t.get().cursor).apply_if(!focused.get(), |s| s.hide())
            }),
        ))
        .style(|s| s.items_center().height_full());
        let view = container(dots).keyboard_navigable();
        let id = view.id();
        self.fields.update(|f| f.push(id));
        view.on_event(EventListener::KeyDown, move |e| {
            let Event::KeyDown(k) = e else { return EventPropagation::Continue };
            let (key, m) = (&k.key.logical_key, k.modifiers);
            match key {
                Key::Named(NamedKey::Enter) => enter(),
                Key::Named(NamedKey::Escape) => self.close(),
                Key::Named(NamedKey::Tab) => self.tab(id, m.shift()),
                _ => {
                    let typed = text.with_untracked(|was| {
                        crate::widgets::quick::secret_key(was, key, m, || floem::Clipboard::get_contents().ok())
                            .filter(|now| now != was)
                    });
                    if let Some(now) = typed {
                        text.set(now);
                    }
                }
            }
            EventPropagation::Stop
        })
        .on_event_cont(EventListener::FocusGained, move |_| {
            focused.set(true);
            self.gained();
        })
        .on_event_cont(EventListener::FocusLost, move |_| {
            focused.set(false);
            self.lost();
        })
        .on_event_stop(EventListener::PointerDown, move |_| id.request_focus())
        .takes_keys(Focus::Input)
        .style(move |s| {
            let c = t.get();
            s.width_full()
                .min_width(0.0)
                .height(26.0)
                .padding_horiz(6.0)
                .border(1.0)
                .border_radius(2.0)
                .border_color(if focused.get() { c.focus } else { c.input_border })
                .background(c.input_bg)
                .color(c.input_fg)
                .cursor(CursorStyle::Text)
        })
        .into_any()
    }

    /// Tab (`back`: Shift+Tab) in the input `id`: the next input shown, or the one before, around the ends.
    fn tab(self, id: ViewId, back: bool) {
        let all: Vec<ViewId> =
            self.fields.get_untracked().into_iter().filter(|f| *f == id || !f.is_hidden_recursive()).collect();
        if let Some(i) = all.iter().position(|f| *f == id) {
            let n = all.len();
            all[if back { (i + n - 1) % n } else { (i + 1) % n }].request_focus();
        }
    }

    /// A field's name over it.
    pub fn named(self, ui: Ui, name: &'static str, field: AnyView) -> AnyView {
        let t = ui.colors;
        v_stack((label(move || name).style(move |s| s.font_size(12.0).color(t.get().fg).selectable(false)), field))
            .style(|s| s.gap(4.0).width_full().min_width(0.0))
            .into_any()
    }

    /// A box to tick, with what it means under its words.
    pub fn checkbox(self, ui: Ui, on: RwSignal<bool>, words: &'static str, hint: &'static str) -> AnyView {
        let t = ui.colors;
        let mark = container(icon("check", 14.0).style(move |s| s.apply_if(!on.get(), |s| s.hide()))).style(move |s| {
            let c = t.get();
            s.width(18.0)
                .height(18.0)
                .flex_shrink(0.0_f32)
                .items_center()
                .justify_center()
                .border(1.0)
                .border_radius(3.0)
                .border_color(c.checkbox_border)
                .background(c.checkbox_bg)
                .color(c.checkbox_fg)
        });
        h_stack((mark, said(ui, words, hint)))
            .on_click_stop(move |_| {
                on.update(|o| *o = !*o);
                self.hold();
            })
            .style(|s| s.gap(8.0).items_start().cursor(CursorStyle::Pointer))
            .into_any()
    }

    /// One of a choice: a round mark, filled while it is the one chosen.
    pub fn radio<T: Copy + PartialEq + 'static>(
        self,
        ui: Ui,
        chosen: RwSignal<T>,
        value: T,
        words: &'static str,
        hint: &'static str,
    ) -> AnyView {
        let t = ui.colors;
        let dot = empty().style(move |s| {
            s.width(8.0)
                .height(8.0)
                .border_radius(4.0)
                .background(t.get().checkbox_fg)
                .apply_if(chosen.get() != value, |s| s.hide())
        });
        let ring = container(dot).style(move |s| {
            let c = t.get();
            s.width(16.0)
                .height(16.0)
                .flex_shrink(0.0_f32)
                .margin_top(1.0)
                .items_center()
                .justify_center()
                .border(1.0)
                .border_radius(8.0)
                .border_color(c.checkbox_border)
                .background(c.checkbox_bg)
        });
        h_stack((ring, said(ui, words, hint)))
            .on_click_stop(move |_| {
                if chosen.get_untracked() != value {
                    chosen.set(value);
                }
                self.hold();
            })
            .style(|s| s.gap(8.0).items_start().cursor(CursorStyle::Pointer))
            .into_any()
    }

    /// The form over the window: its title, what it is for, its fields, and its buttons. Hidden while it is closed.
    pub fn host(
        self,
        ui: Ui,
        title: impl Fn() -> String + 'static,
        detail: impl Fn() -> String + 'static,
        fields: Vec<AnyView>,
        buttons: AnyView,
    ) -> AnyView {
        let t = ui.colors;
        let head = v_stack((
            label(title).style(|s| s.font_size(15.0).font_bold().line_height(1.3)),
            label(detail).style(move |s| {
                s.margin_top(6.0).font_size(12.0).line_height(1.4).color(t.get().desc).text_overflow(TextOverflow::Wrap)
            }),
        ))
        .style(|s| s.width_full().min_width(0.0));
        let body = v_stack_from_iter(fields).style(|s| s.gap(14.0).margin_top(16.0).width_full().min_width(0.0));
        let foot = container(buttons).style(|s| s.margin_top(20.0).width_full().justify_end());
        let card = v_stack((head, body, foot))
            .keyboard_navigable()
            .on_event_stop(EventListener::PointerDown, move |_| self.hold())
            .on_event_cont(EventListener::FocusGained, move |_| self.gained())
            .on_event_cont(EventListener::FocusLost, move |_| self.lost())
            .on_event(EventListener::KeyDown, move |e| {
                if let Event::KeyDown(k) = e
                    && k.key.logical_key == Key::Named(NamedKey::Escape)
                {
                    self.close();
                    return EventPropagation::Stop;
                }
                EventPropagation::Continue
            })
            .style(move |s| {
                let c = t.get();
                s.width(480.0_f64.min(ui.window.get().0 - 40.0).max(160.0))
                    .padding(20.0)
                    .border(1.0)
                    .border_color(c.widget_border)
                    .border_radius(6.0)
                    .background(c.widget)
                    .color(c.fg)
                    .font_size(13.0)
                    .box_shadow_blur(16.0)
                    .box_shadow_color(c.shadow)
                    .focus_visible(|s| s.outline(0.0))
            });
        self.boxed.set(Some(card.id()));
        stack((
            empty().on_event_stop(EventListener::PointerDown, move |_| self.close()).style(|s| {
                s.position(Position::Absolute)
                    .inset(0.0)
                    .size_full()
                    .background(floem::peniko::Color::rgba8(0, 0, 0, 0x4D))
            }),
            card,
        ))
        .style(move |s| {
            s.position(Position::Absolute)
                .inset(0.0)
                .size_full()
                .items_center()
                .justify_center()
                .z_index(150)
                .apply_if(!self.open.get(), |s| s.hide())
        })
        .into_any()
    }
}

/// A choice's words, and under them what it means (none when the hint is empty).
fn said(ui: Ui, words: &'static str, hint: &'static str) -> impl IntoView {
    let t = ui.colors;
    v_stack((
        label(move || words).style(|s| s.selectable(false)),
        label(move || hint).style(move |s| {
            s.font_size(12.0)
                .line_height(1.4)
                .color(t.get().desc)
                .text_overflow(TextOverflow::Wrap)
                .selectable(false)
                .apply_if(hint.is_empty(), |s| s.hide())
        }),
    ))
    .style(|s| s.gap(2.0).flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0))
}

/// A button whose words may change, given up (dimmed, and a press does nothing) while `able` says no.
pub fn button(
    ui: Ui,
    words: impl Fn() -> String + 'static,
    primary: bool,
    able: impl Fn() -> bool + Copy + 'static,
    on: impl Fn() + 'static,
) -> AnyView {
    let t = ui.colors;
    label(words)
        .on_click_stop(move |_| {
            if able() {
                on();
            }
        })
        .style(move |s| {
            let c = t.get();
            let (bg, fg, hover) = if primary {
                (c.button_bg, c.button_fg, c.button_hover)
            } else {
                (c.button2_bg, c.button2_fg, c.button2_hover)
            };
            let on = able();
            // Given up: half way to the box's own colour.
            let (bg, fg) = if on { (bg, fg) } else { (mix(bg, c.widget, 0.5), mix(fg, c.widget, 0.5)) };
            s.padding_horiz(14.0)
                .padding_vert(4.0)
                .border_radius(4.0)
                .font_size(13.0)
                .line_height(1.4)
                .background(bg)
                .color(fg)
                .selectable(false)
                .apply_if(on, |s| s.cursor(CursorStyle::Pointer).hover(|s| s.background(hover)))
        })
        .into_any()
}

/// The buttons at a form's foot, in a row at its right.
pub fn buttons(views: Vec<AnyView>) -> AnyView {
    floem::views::h_stack_from_iter(views).style(|s| s.gap(8.0).justify_end().width_full()).into_any()
}
