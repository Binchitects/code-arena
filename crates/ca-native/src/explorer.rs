//! The Explorer, as VS Code's: the folder's section (its name in capitals, 22 px), then the tree in 22 px rows, each
//! level 8 px further in with its twistie, a folder opening on a click, a file opening in the editor.

use floem::IntoView;
use floem::reactive::{SignalGet, SignalWith};
use floem::style::CursorStyle;
use floem::views::{Decorators, dyn_stack, empty, h_stack, label, scroll, v_stack};

use crate::icons::icon;
use crate::state::{Send, Ui};
use crate::widgets::{bold, caps, line};

#[derive(Clone, PartialEq)]
struct Row {
    depth: usize,
    path: String,
    name: String,
    dir: bool,
    open: bool,
}

fn rows(ui: Ui) -> Vec<Row> {
    fn walk(ui: Ui, dir: &str, depth: usize, out: &mut Vec<Row>) {
        let entries = ui.tree.with(|t| t.get(dir).cloned()).unwrap_or_default();
        for e in entries {
            let path = if dir.is_empty() { e.name.clone() } else { format!("{dir}/{}", e.name) };
            let open = e.dir && ui.expanded.with(|x| x.contains(&path));
            out.push(Row { depth, path: path.clone(), name: e.name, dir: e.dir, open });
            if open {
                walk(ui, &path, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(ui, "", 0, &mut out);
    out
}

pub fn view(ui: Ui, send: Send) -> impl IntoView {
    let t = ui.colors;
    let header = h_stack((
        icon("chevron-down", 16.0),
        label(move || ui.folder.get().rsplit('/').next().unwrap_or("").to_uppercase()).style(|s| bold(caps(s))),
    ))
    .style(move |s| {
        let c = t.get();
        line(s, c.section_border, 'b')
            .height(22.0)
            .items_center()
            .gap(2.0)
            .padding_left(2.0)
            .background(c.section_bg)
            .color(c.section_fg)
            .selectable(false)
    });
    let list = dyn_stack(
        move || rows(ui),
        |r: &Row| (r.path.clone(), r.open),
        move |r: Row| {
            let send = send.clone();
            let path = r.path.clone();
            let shown = r.path.clone();
            h_stack((
                empty().style(move |s| s.width(8.0 * r.depth as f64)),
                if r.dir {
                    icon(if r.open { "chevron-down" } else { "chevron-right" }, 16.0).into_any()
                } else {
                    empty().style(|s| s.width(16.0)).into_any()
                },
                label(move || r.name.clone()).style(|s| s.padding_left(4.0).text_ellipsis()),
            ))
            .on_click_stop(move |_| {
                if r.dir {
                    ui.toggle_folder(&path, &send);
                } else {
                    ui.open(&path, &send);
                }
            })
            .style(move |s| {
                let c = t.get();
                let selected = ui.selected.with(|x| *x == shown);
                s.height(22.0)
                    .width_full()
                    .items_center()
                    .padding_left(8.0)
                    .font_size(13.0)
                    .color(c.side_fg)
                    .cursor(CursorStyle::Pointer)
                    .selectable(false)
                    .hover(|s| s.background(c.list_hover))
                    .apply_if(selected, |s| s.background(c.list_inactive))
            })
        },
    )
    .style(|s| s.flex_col().width_full());
    v_stack((header, scroll(list).style(|s| s.flex_grow(1.0_f32).flex_basis(0.0).width_full())))
        .style(|s| s.size_full().min_height(0.0))
}
