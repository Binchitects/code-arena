//! The open files as they are on disk: one read into its tab, the watched ones read again when they change, and the
//! notice above the editor when the shown file changed on disk under changes not saved.

use std::rc::Rc;

use floem::IntoView;
use floem::reactive::{SignalGet, SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::views::editor::core::buffer::rope_text::RopeText;
use floem::views::editor::text::Document;
use floem::views::editor::text_document::TextDocument;
use floem::views::{Decorators, h_stack, label};
use serde_json::Value;

use super::tell;
use crate::icons::icon;
use crate::state::{OnDisk, Send, Tab, Ui};
use crate::widgets::{Theme, action, line};

/// A small button of a bar (the notice above the editor).
fn small(t: Theme, text: &'static str, on: impl Fn() + 'static) -> impl IntoView {
    label(move || text).on_click_stop(move |_| on()).style(move |s| {
        let c = t.get();
        s.padding_horiz(8.0)
            .height(20.0)
            .items_center()
            .border_radius(2.0)
            .font_size(12.0)
            .background(c.button2_bg)
            .color(c.button2_fg)
            .cursor(CursorStyle::Pointer)
            .selectable(false)
            .hover(|s| s.background(c.button2_hover))
    })
}

/// The notice above the editor when the shown file changed on disk (or is gone from it) while it has changes not
/// saved: Revert takes what is on disk, Overwrite saves the tab's text over it, the cross leaves things as they are.
pub(super) fn disk_bar(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    let state = move || ui.active.with(|a| a.as_ref().and_then(|p| ui.on_disk.with(|d| d.get(p).copied())));
    let on_active = move |what: fn(Ui, &str, &Send)| {
        if let Some(path) = ui.active.get_untracked() {
            tell(ui, |send| what(ui, &path, send));
        }
    };
    h_stack((
        icon("warning", 16.0).style(move |s| s.color(t.get().warning)),
        label(move || {
            let name = ui.active.with(|a| a.as_deref().map(|p| p.rsplit('/').next().unwrap_or(p).to_string()));
            let name = name.unwrap_or_default();
            match state() {
                Some(OnDisk::Gone) => format!("{name} was deleted on disk. Saving it makes it again."),
                _ => format!("{name} has changed on disk, and has changes of yours that are not saved."),
            }
        })
        .style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0).text_ellipsis()),
        small(t, "Revert", move || on_active(|ui, path, send| ui.take_disk(path, send)))
            .style(move |s| s.apply_if(state() != Some(OnDisk::Changed), |s| s.hide())),
        small(t, "Overwrite", move || on_active(|ui, path, send| ui.save_tab(path, send))),
        action(t, "close", move || {
            if let Some(path) = ui.active.get_untracked() {
                ui.forget_on_disk(&path);
            }
        }),
    ))
    .style(move |s| {
        let c = t.get();
        line(s, c.notice_border, 'b')
            .width_full()
            .height(28.0)
            .padding_horiz(12.0)
            .gap(8.0)
            .items_center()
            .font_size(13.0)
            .background(c.notice_bg)
            .color(c.fg)
            .selectable(false)
            .apply_if(state().is_none(), |s| s.hide())
    })
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are of the open files as they are on disk (one read, the watched ones changed), taken in. Never taken away from the
/// rest of the window (false).
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
    match m["type"].as_str().unwrap_or("") {
        "file" => {
            let path = s("path");
            // Read again because it changed on disk: its text is taken, its tab is not shown for it.
            let again = ui.reloading.with_untracked(|r| r.contains(&path));
            if again {
                ui.reloading.update(|r| {
                    r.remove(&path);
                });
            }
            if let Some(e) = m["error"].as_str() {
                ui.message.set(format!("Could not open {path}: {e}"));
                return false;
            }
            if let Some(at) = m["modified"].as_u64() {
                ui.modified.update(|k| {
                    k.insert(path.clone(), at);
                });
            }
            ui.forget_on_disk(&path);
            match ui.tab(&path) {
                // Taken in only when the tab has no changes of its own.
                Some(tab) => {
                    if !tab.dirty.get_untracked() {
                        ui.replace(&tab, &s("text"));
                    }
                }
                None if again => return false, // closed meanwhile
                None => {
                    let text = s("text");
                    let doc = Rc::new(TextDocument::new(ui.scope, text));
                    let dirty = ui.scope.create_rw_signal(false);
                    let colours = crate::highlight::Colours::new(doc.clone(), &path);
                    {
                        let (colours, d, edited) = (colours.clone(), doc.clone(), ui.edited);
                        doc.add_on_update(move |u| {
                            dirty.set(true);
                            edited.update(|n| *n += 1);
                            // Parsed again from the first line an edit touched.
                            let first = u.deltas().map(|x| x.summary().0.start()).min().unwrap_or(0);
                            colours.changed_from(d.rope_text().line_of_offset(first));
                        });
                    }
                    ui.tabs.update(|t| {
                        t.push(Tab {
                            path: path.clone(),
                            file: path.clone(),
                            doc,
                            dirty,
                            diff: None,
                            colours,
                            kind: None,
                        })
                    });
                }
            }
            if !again {
                ui.active.set(Some(path));
            }
        }
        // What is watched changed on disk (the agent wrote it, or another program): a tab with no changes of
        // its own reads it again, one with changes says so and leaves the choice to the person.
        "stats" => {
            for it in m["items"].as_array().into_iter().flatten() {
                let path = it["path"].as_str().unwrap_or("").to_string();
                let Some(tab) = ui.tab(&path).filter(|t| t.is_file()) else { continue };
                let dirty = tab.dirty.get_untracked();
                if it["gone"] == true {
                    ui.on_disk.update(|d| {
                        d.insert(path, OnDisk::Gone);
                    });
                } else if let Some(now) = it["modified"].as_u64()
                    && ui.modified.with_untracked(|k| k.get(&path).is_some_and(|known| *known != now))
                {
                    if dirty {
                        ui.on_disk.update(|d| {
                            d.insert(path, OnDisk::Changed);
                        });
                    } else {
                        ui.reload(&path, send);
                    }
                }
            }
        }
        _ => {}
    }
    false
}
