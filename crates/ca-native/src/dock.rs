//! The window's layout as Visual Studio docks its tool windows: a tree of splits (a row or a column of parts) and
//! tab groups (views, the one shown), with the editor a fixed leaf. A view is docked to a side of any part, or into a
//! group's tabs; each view is in the tree once, the editor once, no group is empty, no split has one part only, and
//! a split in the same direction as its parent is folded into it. Sizes are kept apart (by a part's number), so a
//! sash only sets a size and the tree is not built again.
//!
//! The tree holds the views that are in the window (`layout::ready`), no others. Each view has a first place
//! (`HOMES`): the side bar, the panel or the secondary side bar. A view that a kept layout does not have (it was
//! kept before the view was there) goes into the group that holds the views of its first place, among them in
//! their order; a group of its own is made, where its first place is, only when none of them is in the tree.

use floem::kurbo::{Point, Rect};
use serde_json::{Value, json};

use crate::layout::View;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dir {
    Row,
    Column,
}

/// Where a view goes against a part: a side of it, or into its tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
    Center,
}

impl Side {
    fn dir(self) -> Dir {
        match self {
            Side::Left | Side::Right => Dir::Row,
            _ => Dir::Column,
        }
    }

    fn before(self) -> bool {
        matches!(self, Side::Left | Side::Top)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Split { id: u32, dir: Dir, kids: Vec<Node> },
    Tabs { id: u32, views: Vec<View>, active: View },
    Editor,
}

/// The editor's number (a part's number names its size; the window's whole is `ROOT`).
pub const EDITOR: u32 = 0;
pub const ROOT: u32 = u32::MAX;

impl Node {
    pub fn id(&self) -> u32 {
        match self {
            Node::Split { id, .. } | Node::Tabs { id, .. } => *id,
            Node::Editor => EDITOR,
        }
    }

    pub fn has_editor(&self) -> bool {
        match self {
            Node::Editor => true,
            Node::Tabs { .. } => false,
            Node::Split { kids, .. } => kids.iter().any(Node::has_editor),
        }
    }

    /// The groups in it, by number.
    pub fn groups(&self, out: &mut Vec<u32>) {
        match self {
            Node::Tabs { id, .. } => out.push(*id),
            Node::Split { kids, .. } => kids.iter().for_each(|k| k.groups(out)),
            Node::Editor => {}
        }
    }

    #[cfg(test)]
    fn views(&self, out: &mut Vec<View>) {
        match self {
            Node::Tabs { views, .. } => out.extend(views),
            Node::Split { kids, .. } => kids.iter().for_each(|k| k.views(out)),
            Node::Editor => {}
        }
    }

    fn find(&self, id: u32) -> Option<&Node> {
        if self.id() == id {
            return Some(self);
        }
        match self {
            Node::Split { kids, .. } => kids.iter().find_map(|k| k.find(id)),
            _ => None,
        }
    }

    fn find_mut(&mut self, id: u32) -> Option<&mut Node> {
        if self.id() == id {
            return Some(self);
        }
        match self {
            Node::Split { kids, .. } => kids.iter_mut().find_map(|k| k.find_mut(id)),
            _ => None,
        }
    }

    /// The split holding the part `id`, and its place in it.
    fn parent_of_mut(&mut self, id: u32) -> Option<(&mut Node, usize)> {
        let at = match self {
            Node::Split { kids, .. } => kids.iter().position(|k| k.id() == id),
            _ => return None,
        };
        if let Some(i) = at {
            return Some((self, i));
        }
        match self {
            Node::Split { kids, .. } => kids.iter_mut().find_map(|k| k.parent_of_mut(id)),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dock {
    pub root: Node,
    /// The next part's number.
    pub next: u32,
    /// The views in windows of their own (Visual Studio's floating tool windows), out of the tree.
    pub floating: Vec<View>,
}

/// The parts' first sizes (by number): the default tree's side bar, panel and secondary side bar, as VS Code's.
pub const FIRST_SIZES: [(u32, f64); 3] = [(2, 260.0), (4, 230.0), (5, 380.0)];
/// A new group's first size.
pub const NEW_SIZE: f64 = 280.0;

/// A view's first place: VS Code's side bar, its panel under the editor, or its secondary side bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Home {
    Side,
    Panel,
    Secondary,
}

/// Every view's first place, and its order among the views there (the activity bar's order, the panel's tabs').
pub const HOMES: [(Home, &[View]); 3] = [
    (
        Home::Side,
        &[
            View::Files,
            View::Search,
            View::Scm,
            View::Run,
            View::Extensions,
            View::Changes,
            View::Problems,
            View::Review,
            View::Plan,
            View::Memory,
            View::Skills,
            View::Projects,
            View::Automation,
            View::Sessions,
            View::Harness,
            View::Outline,
        ],
    ),
    (Home::Panel, &[View::Terminal, View::DebugConsole, View::Output, View::Ports]),
    (Home::Secondary, &[View::Agent, View::Tasks, View::Context]),
];

/// A view's first place, and the views of that place in their order.
pub fn home_of(v: View) -> (Home, &'static [View]) {
    HOMES.into_iter().find(|(_, views)| views.contains(&v)).unwrap_or(HOMES[0])
}

impl Default for Dock {
    /// VS Code's own, with the views that are in the window: the side bar, the editor over the panel, the secondary
    /// side bar.
    fn default() -> Dock {
        Dock::of(&crate::layout::ready())
    }
}

impl Dock {
    /// The first layout of these views: each in its first place (a place none of them has is not there). The side
    /// bar is part 2, the panel 4 and the secondary side bar 5, whichever views there are (their first sizes are
    /// kept by those numbers).
    pub fn of(views: &[View]) -> Dock {
        let group = |id: u32, home: Home| {
            let mine: Vec<View> = HOMES
                .into_iter()
                .filter(|(h, _)| *h == home)
                .flat_map(|(_, all)| all.iter().copied())
                .filter(|v| views.contains(v))
                .collect();
            let active = mine.first().copied().unwrap_or(View::Files);
            Node::Tabs { id, views: mine, active }
        };
        let root = Node::Split {
            id: 1,
            dir: Dir::Row,
            kids: vec![
                group(2, Home::Side),
                Node::Split { id: 3, dir: Dir::Column, kids: vec![Node::Editor, group(4, Home::Panel)] },
                group(5, Home::Secondary),
            ],
        };
        // A place with no view is taken out (and the split left with one part folded), nothing put back.
        Dock { root, next: 6, floating: Vec::new() }.tidied_for(views, false)
    }

    pub fn find(&self, id: u32) -> Option<&Node> {
        self.root.find(id)
    }

    /// The same without these views: taken out of their groups (and of the floating ones), a group left empty gone.
    /// The parts that stay keep their numbers. What is drawn while views are withheld.
    pub fn without_all(&self, gone: &[View]) -> Dock {
        if gone.is_empty() {
            return self.clone();
        }
        fn strip(n: &mut Node, gone: &[View]) {
            match n {
                Node::Tabs { views, .. } => views.retain(|v| !gone.contains(v)),
                Node::Split { kids, .. } => kids.iter_mut().for_each(|k| strip(k, gone)),
                Node::Editor => {}
            }
        }
        let mut d = self.clone();
        strip(&mut d.root, gone);
        d.floating.retain(|v| !gone.contains(v));
        d.tidied(false)
    }

    /// The group a view is in.
    pub fn group_of(&self, v: View) -> Option<u32> {
        let mut groups = Vec::new();
        self.root.groups(&mut groups);
        groups.into_iter().find(|g| matches!(self.find(*g), Some(Node::Tabs { views, .. }) if views.contains(&v)))
    }

    /// The side bar of VS Code: the part at the far left of the window when it is a group (the activity bar
    /// switches its views, and it shows its view's title rather than tabs).
    pub fn side_bar(&self) -> Option<u32> {
        match &self.root {
            Node::Split { dir: Dir::Row, kids, .. } => match kids.first() {
                Some(Node::Tabs { id, .. }) => Some(*id),
                _ => None,
            },
            _ => None,
        }
    }

    /// The view shown in its group.
    pub fn shown(&self, v: View) -> Dock {
        let mut d = self.clone();
        if let Some(g) = d.group_of(v)
            && let Some(Node::Tabs { active, .. }) = d.root.find_mut(g)
        {
            *active = v;
        }
        d
    }

    /// The view taken out of its group (the group gone when it was its last), to be put back elsewhere.
    fn without(&self, v: View) -> Dock {
        let mut d = self.clone();
        if let Some(g) = d.group_of(v)
            && let Some(Node::Tabs { views, active, .. }) = d.root.find_mut(g)
        {
            views.retain(|x| *x != v);
            if *active == v
                && let Some(first) = views.first()
            {
                *active = *first;
            }
        }
        d.tidied(false)
    }

    /// `v` taken out of the tree into a window of its own.
    pub fn floated(&self, v: View) -> Dock {
        if self.floating.contains(&v) {
            return self.clone();
        }
        let mut d = self.without(v);
        d.floating.push(v);
        d.normalized()
    }

    /// `v` docked against the part `target` (`ROOT`: the window's edge): beside it, or into its tabs.
    pub fn docked(&self, v: View, target: u32, side: Side) -> Dock {
        if self.floating.contains(&v) {
            let mut d = self.clone();
            d.floating.retain(|x| *x != v);
            // Put back into the tree beside the target (a group's tabs are joined the same way as from the tree).
            let id = d.next;
            d.next += 1;
            let lone = Node::Tabs { id, views: vec![v], active: v };
            d.root = match d.root {
                Node::Split { id: rid, dir, mut kids } => {
                    kids.push(lone);
                    Node::Split { id: rid, dir, kids }
                }
                other => Node::Split { id: id + 1, dir: Dir::Row, kids: vec![other, lone] },
            };
            d.next += 1;
            let d = d.tidied(false);
            return if target == id { d } else { d.docked(v, target, side) };
        }
        let from = self.group_of(v);
        // Its group's only view, docked against its own group: nothing to do.
        if from == Some(target) && matches!(self.find(target), Some(Node::Tabs { views, .. }) if views.len() == 1) {
            return self.clone();
        }
        if side == Side::Center {
            if from == Some(target) || !matches!(self.find(target), Some(Node::Tabs { .. })) {
                return self.shown(v);
            }
            let mut d = self.without(v);
            if let Some(Node::Tabs { views, active, .. }) = d.root.find_mut(target) {
                views.push(v);
                *active = v;
            }
            return d.normalized();
        }
        let mut d = self.without(v);
        if target != ROOT && d.find(target).is_none() {
            return self.clone();
        }
        let id = d.next;
        d.next += 2;
        let group = Node::Tabs { id, views: vec![v], active: v };
        let dir = side.dir();
        if target == ROOT {
            let root = std::mem::replace(&mut d.root, Node::Editor);
            d.root = match root {
                Node::Split { id: rid, dir: rdir, mut kids } if rdir == dir => {
                    if side.before() {
                        kids.insert(0, group)
                    } else {
                        kids.push(group)
                    }
                    Node::Split { id: rid, dir, kids }
                }
                other => {
                    let kids = if side.before() { vec![group, other] } else { vec![other, group] };
                    Node::Split { id: id + 1, dir, kids }
                }
            };
            return d.normalized();
        }
        let same_dir = matches!(d.root.parent_of_mut(target), Some((Node::Split { dir: pdir, .. }, _)) if *pdir == dir);
        if same_dir {
            if let Some((Node::Split { kids, .. }, at)) = d.root.parent_of_mut(target) {
                kids.insert(if side.before() { at } else { at + 1 }, group);
            }
        } else if let Some(node) = d.root.find_mut(target) {
            let old = std::mem::replace(node, Node::Editor);
            let kids = if side.before() { vec![group, old] } else { vec![old, group] };
            *node = Node::Split { id: id + 1, dir, kids };
        }
        d.normalized()
    }

    /// Kept whole: no empty group, no split of one part, same-direction splits folded, each view that is in the
    /// window once (one that is missing put in its first place), no other view, the editor once.
    pub fn normalized(&self) -> Dock {
        self.tidied(true)
    }

    /// The same, a missing view put back only when `fill` (not while one is being moved).
    fn tidied(&self, fill: bool) -> Dock {
        self.tidied_for(&crate::layout::ready(), fill)
    }

    /// A view that is not in the tree put where it belongs: among the views of its first place, in the group that
    /// holds the first of them there is, after the ones that come before it; none of them in the tree, a group of
    /// its own where its first place is. False when it was there already.
    fn put_home(&mut self, v: View) -> bool {
        if self.group_of(v).is_some() || self.floating.contains(&v) {
            return false;
        }
        let (home, mates) = home_of(v);
        let at = mates.iter().position(|m| *m == v).unwrap_or(0);
        if let Some(g) = mates.iter().find_map(|m| self.group_of(*m))
            && let Some(Node::Tabs { views, .. }) = self.root.find_mut(g)
        {
            // After the last of the views that come before it; else before the first that comes after it.
            let after = views.iter().rposition(|x| mates[..at].contains(x)).map(|i| i + 1);
            let before = views.iter().position(|x| mates[at..].contains(x));
            let place = after.or(before).unwrap_or(views.len());
            views.insert(place, v);
            return true;
        }
        let id = self.next;
        self.next += 2;
        let group = Node::Tabs { id, views: vec![v], active: v };
        let root = std::mem::replace(&mut self.root, Node::Editor);
        self.root = match home {
            // Under the editor.
            Home::Panel => {
                fn under(n: Node, group: &mut Option<Node>, id: u32) -> Node {
                    match n {
                        Node::Editor => match group.take() {
                            Some(g) => Node::Split { id, dir: Dir::Column, kids: vec![Node::Editor, g] },
                            None => Node::Editor,
                        },
                        Node::Split { id: sid, dir, kids } => {
                            Node::Split { id: sid, dir, kids: kids.into_iter().map(|k| under(k, group, id)).collect() }
                        }
                        tabs => tabs,
                    }
                }
                under(root, &mut Some(group), id + 1)
            }
            // At the window's left, or its right.
            side => {
                let first = side == Home::Side;
                match root {
                    Node::Split { id: rid, dir: Dir::Row, mut kids } => {
                        if first {
                            kids.insert(0, group)
                        } else {
                            kids.push(group)
                        }
                        Node::Split { id: rid, dir: Dir::Row, kids }
                    }
                    other => {
                        let kids = if first { vec![group, other] } else { vec![other, group] };
                        Node::Split { id: id + 1, dir: Dir::Row, kids }
                    }
                }
            }
        };
        true
    }

    /// Kept whole for the views given (the ones in the window).
    fn tidied_for(&self, ready: &[View], fill: bool) -> Dock {
        fn clean(n: Node, seen: &mut Vec<View>, editor: &mut bool) -> Option<Node> {
            match n {
                Node::Editor => {
                    if *editor {
                        None
                    } else {
                        *editor = true;
                        Some(Node::Editor)
                    }
                }
                Node::Tabs { id, views, active } => {
                    let views: Vec<View> = views
                        .into_iter()
                        .filter(|v| {
                            let fresh = !seen.contains(v);
                            seen.push(*v);
                            fresh
                        })
                        .collect();
                    let first = *views.first()?;
                    let active = if views.contains(&active) { active } else { first };
                    Some(Node::Tabs { id, views, active })
                }
                Node::Split { id, dir, kids } => {
                    let mut out = Vec::new();
                    for k in kids {
                        match clean(k, seen, editor) {
                            Some(Node::Split { dir: kdir, kids: inner, .. }) if kdir == dir => out.extend(inner),
                            Some(k) => out.push(k),
                            None => {}
                        }
                    }
                    match out.len() {
                        0 => None,
                        1 => out.pop(),
                        _ => Some(Node::Split { id, dir, kids: out }),
                    }
                }
            }
        }
        // A floating view is out of the tree: a copy of it there goes, and it is not missing. A view that is not in
        // the window (one a later Code Arena kept) is in neither.
        let mut floating: Vec<View> = self.floating.iter().copied().filter(|v| ready.contains(v)).collect();
        floating.dedup();
        // Seen already, so cleaned out of the tree: the floating ones, and every view that is not in the window.
        let mut seen: Vec<View> =
            floating.iter().copied().chain(crate::layout::VIEWS.into_iter().filter(|v| !ready.contains(v))).collect();
        let mut editor = false;
        let mut root = clean(self.root.clone(), &mut seen, &mut editor).unwrap_or(Node::Editor);
        let mut next = self.next;
        if !editor {
            root = Node::Split { id: next, dir: Dir::Row, kids: vec![root, Node::Editor] };
            next += 1;
        }
        let mut d = Dock { root, next, floating };
        // The ids above every part's before a part is made for a missing view.
        d.next = d.next.max(d.most() + 1);
        // Each missing view is put in its place (all of them: not only until the first that was missing).
        let mut put = false;
        if fill {
            for v in ready {
                put |= d.put_home(*v);
            }
        }
        if put {
            // A group made beside a split of its own direction is folded into it.
            return d.tidied_for(ready, false);
        }
        // Numbers stay above every part's, so a new part never takes one in use.
        d.next = d.next.max(d.most() + 1);
        d
    }

    /// The greatest number a part of the tree has.
    fn most(&self) -> u32 {
        fn all_ids(n: &Node, out: &mut Vec<u32>) {
            out.push(n.id());
            if let Node::Split { kids, .. } = n {
                kids.iter().for_each(|k| all_ids(k, out));
            }
        }
        let mut ids = Vec::new();
        all_ids(&self.root, &mut ids);
        ids.into_iter().filter(|i| *i != ROOT).max().unwrap_or(0)
    }

    pub fn to_json(&self) -> Value {
        fn node(n: &Node) -> Value {
            match n {
                Node::Editor => json!({"editor": true}),
                Node::Tabs { id, views, active } => {
                    json!({"id": id, "views": views.iter().map(|v| v.id()).collect::<Vec<_>>(), "active": active.id()})
                }
                Node::Split { id, dir, kids } => json!({
                    "id": id,
                    "dir": if *dir == Dir::Row { "row" } else { "column" },
                    "kids": kids.iter().map(node).collect::<Vec<_>>(),
                }),
            }
        }
        json!({"root": node(&self.root), "next": self.next, "floating": self.floating.iter().map(|v| v.id()).collect::<Vec<_>>()})
    }

    /// A dock kept before, read kindly: what it lacks put right; one kept in another form (or none) is the default.
    pub fn from_json(v: &Value) -> Dock {
        fn node(v: &Value) -> Option<Node> {
            if v["editor"] == true {
                return Some(Node::Editor);
            }
            let id = u32::try_from(v["id"].as_u64()?).ok().filter(|i| *i != EDITOR && *i != ROOT)?;
            if let Some(views) = v["views"].as_array() {
                let views: Vec<View> = views.iter().filter_map(|x| x.as_str().and_then(View::from_id)).collect();
                let active = v["active"].as_str().and_then(View::from_id).or(views.first().copied())?;
                return Some(Node::Tabs { id, views, active });
            }
            let dir = match v["dir"].as_str()? {
                "row" => Dir::Row,
                "column" => Dir::Column,
                _ => return None,
            };
            Some(Node::Split { id, dir, kids: v["kids"].as_array()?.iter().filter_map(node).collect() })
        }
        match node(&v["root"]) {
            Some(root) => {
                let floating = v["floating"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|x| x.as_str().and_then(View::from_id))
                    .collect();
                Dock { root, next: v["next"].as_u64().unwrap_or(0) as u32, floating }.normalized()
            }
            None => Dock::default(),
        }
    }
}

/// Visual Studio's guide on a part: five squares in a cross at its middle (the centre one joins its tabs).
pub const GUIDE: f64 = 32.0;
const GAP: f64 = 4.0;

pub fn guide(r: Rect) -> [(Side, Rect); 5] {
    let c = r.center();
    let at = |dx: f64, dy: f64| {
        let o = Point::new(c.x + dx * (GUIDE + GAP) - GUIDE / 2.0, c.y + dy * (GUIDE + GAP) - GUIDE / 2.0);
        Rect::from_origin_size(o, (GUIDE, GUIDE))
    };
    [
        (Side::Center, at(0.0, 0.0)),
        (Side::Left, at(-1.0, 0.0)),
        (Side::Right, at(1.0, 0.0)),
        (Side::Top, at(0.0, -1.0)),
        (Side::Bottom, at(0.0, 1.0)),
    ]
}

/// The window's own guides: a square at the middle of each edge.
pub fn edge_guides(w: Rect) -> [(Side, Rect); 4] {
    let c = w.center();
    let m = 10.0;
    let sq = |x: f64, y: f64| Rect::from_origin_size((x, y), (GUIDE, GUIDE));
    [
        (Side::Left, sq(w.x0 + m, c.y - GUIDE / 2.0)),
        (Side::Right, sq(w.x1 - m - GUIDE, c.y - GUIDE / 2.0)),
        (Side::Top, sq(c.x - GUIDE / 2.0, w.y0 + m)),
        (Side::Bottom, sq(c.x - GUIDE / 2.0, w.y1 - m - GUIDE)),
    ]
}

/// The guide square under the pointer, if any.
pub fn zone(squares: &[(Side, Rect)], p: Point) -> Option<Side> {
    squares.iter().find(|(_, r)| r.contains(p)).map(|(s, _)| *s)
}

/// Where a view docked on that side would go: that half of the part (a third of the window at its edge), or the
/// whole part for its tabs.
pub fn preview(r: Rect, side: Side, edge: bool) -> Rect {
    let f = if edge { 1.0 / 3.0 } else { 0.5 };
    let (w, h) = (r.width() * f, r.height() * f);
    match side {
        Side::Left => Rect::new(r.x0, r.y0, r.x0 + w, r.y1),
        Side::Right => Rect::new(r.x1 - w, r.y0, r.x1, r.y1),
        Side::Top => Rect::new(r.x0, r.y0, r.x1, r.y0 + h),
        Side::Bottom => Rect::new(r.x0, r.y1 - h, r.x1, r.y1),
        Side::Center => r,
    }
}

#[cfg(test)]
mod tests {
    use super::View::*;
    use super::*;

    fn ok(d: &Dock) {
        let mut views = Vec::new();
        d.root.views(&mut views);
        let mut sorted = views.clone();
        sorted.sort_by_key(|v| v.id());
        sorted.dedup();
        assert_eq!(sorted.len(), views.len(), "each view once: {d:?}");
        assert_eq!(views.len() + d.floating.len(), crate::layout::ready().len(), "every view: {d:?}");
        fn walk(n: &Node, parent: Option<Dir>, editors: &mut usize) {
            match n {
                Node::Editor => *editors += 1,
                Node::Tabs { views, active, .. } => {
                    assert!(!views.is_empty());
                    assert!(views.contains(active));
                }
                Node::Split { dir, kids, .. } => {
                    assert!(kids.len() >= 2, "no split of one part");
                    assert_ne!(Some(*dir), parent, "same-direction splits folded");
                    kids.iter().for_each(|k| walk(k, Some(*dir), editors));
                }
            }
        }
        let mut editors = 0;
        walk(&d.root, None, &mut editors);
        assert_eq!(editors, 1);
    }

    #[test]
    fn the_default_is_vs_codes() {
        let d = Dock::default();
        ok(&d);
        assert_eq!(d.side_bar(), Some(2));
        assert_eq!(d.group_of(Terminal), Some(4));
        assert_eq!(d.normalized(), d);
    }

    #[test]
    fn a_view_docked_beside_a_part_or_in_its_tabs() {
        // The terminal to the right of the editor: its column (the editor alone then) folds into the window's
        // row, the terminal next after the editor.
        let d = Dock::default().docked(Terminal, EDITOR, Side::Right);
        ok(&d);
        let Node::Split { dir: Dir::Row, kids, .. } = &d.root else { panic!() };
        assert_eq!(kids[1], Node::Editor);
        assert!(matches!(&kids[2], Node::Tabs { views, .. } if *views == [Terminal]));
        // The search under the editor, beside the terminal's column: a column of editor, search, terminal.
        let d = Dock::default().docked(Search, EDITOR, Side::Bottom);
        ok(&d);
        let Node::Split { kids, .. } = &d.root else { panic!() };
        assert!(
            matches!(&kids[1], Node::Split { dir: Dir::Column, kids, .. } if kids.len() == 3 && kids[0] == Node::Editor)
        );
        // Search into the agent's tabs, shown there.
        let d = Dock::default().docked(Search, 5, Side::Center);
        ok(&d);
        assert!(matches!(d.find(5), Some(Node::Tabs { views, active: Search, .. }) if *views == [Agent, Search]));
        // The agent at the window's bottom edge: under everything.
        let d = Dock::default().docked(Agent, ROOT, Side::Bottom);
        ok(&d);
        assert!(
            matches!(&d.root, Node::Split { dir: Dir::Column, kids, .. } if matches!(kids.last(), Some(Node::Tabs { views, .. }) if *views == [Agent]))
        );
        // A group's only view against its own group: as it was.
        assert_eq!(Dock::default().docked(Agent, 5, Side::Left), Dock::default());
    }

    #[test]
    fn docking_beside_a_part_of_the_same_direction_goes_in_line() {
        // Left of the side bar, in the window's row: a part of its own at the far left, no split in a split.
        let d = Dock::default().docked(Agent, 2, Side::Left);
        ok(&d);
        let Node::Split { dir: Dir::Row, kids, .. } = &d.root else { panic!() };
        assert!(matches!(&kids[0], Node::Tabs { views, .. } if *views == [Agent]));
        assert_eq!(kids.len(), 3, "the agent's old part went with it");
    }

    #[test]
    fn many_docks_keep_it_whole() {
        let sides = [Side::Left, Side::Right, Side::Top, Side::Bottom, Side::Center];
        let mut d = Dock::default();
        let mut n: u64 = 7;
        for _ in 0..500 {
            n = n.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let views = crate::layout::ready();
            let v = views[(n >> 33) as usize % views.len()];
            let mut targets = vec![EDITOR, ROOT];
            d.root.groups(&mut targets);
            let target = targets[(n >> 40) as usize % targets.len()];
            d = if (n >> 20).is_multiple_of(9) {
                d.floated(v)
            } else {
                d.docked(v, target, sides[(n >> 50) as usize % sides.len()])
            };
            ok(&d);
            assert_eq!(Dock::from_json(&d.to_json()), d, "kept and read back as it was");
        }
    }

    #[test]
    fn a_view_floated_and_docked_back() {
        let d = Dock::default().floated(Terminal);
        assert_eq!(d.floating, [Terminal]);
        assert_eq!(d.group_of(Terminal), None, "out of the tree");
        assert_eq!(Dock::from_json(&d.to_json()), d);
        let back = d.docked(Terminal, EDITOR, Side::Bottom);
        ok(&back);
        assert!(back.floating.is_empty());
        let Node::Split { kids, .. } = &back.root else { panic!() };
        assert!(matches!(&kids[1], Node::Split { dir: Dir::Column, kids, .. } if kids[0] == Node::Editor));
    }

    #[test]
    fn a_layout_kept_in_the_old_form_is_the_default() {
        let old =
            json!({"primary": ["files"], "secondary": ["agent"], "panel": ["terminal"], "active": {}, "shown": {}});
        assert_eq!(Dock::from_json(&old), Dock::default());
        assert_eq!(Dock::from_json(&Value::Null), Dock::default());
        // A view lost from a kept dock comes back beside the editor.
        let mut kept = Dock::default().to_json();
        kept["root"]["kids"][2]["views"] = json!(["bogus"]);
        let d = Dock::from_json(&kept);
        ok(&d);
        assert!(d.group_of(Agent).is_some());
    }

    #[test]
    fn a_view_new_to_a_kept_layout_goes_to_its_first_place_among_the_views_there() {
        use crate::layout::also_ready;
        // A layout kept with the views the window began with, Search the one shown in the side bar and the terminal
        // docked to the right of the editor.
        let kept = Dock::default().docked(Search, 2, Side::Center).docked(Terminal, EDITOR, Side::Right);
        let saved = kept.to_json();
        let parts = |d: &Dock| {
            let mut groups = Vec::new();
            d.root.groups(&mut groups);
            groups.len()
        };
        // Views written since: each in the group that holds the views of its first place, in their order, and no
        // group more than there were.
        also_ready(&[Scm, Problems, Sessions, Output, Tasks]);
        let d = Dock::from_json(&saved);
        ok(&d);
        assert_eq!(parts(&d), parts(&kept), "no group of their own");
        assert!(
            matches!(d.find(2), Some(Node::Tabs { views, .. })
                if *views == [Files, Search, Scm, Changes, Problems, Sessions, Outline]),
            "{d:?}"
        );
        let terminal = d.group_of(Terminal).expect("there");
        assert_eq!(d.group_of(Output), Some(terminal), "with the terminal, where the person put it");
        assert_eq!(d.group_of(Tasks), d.group_of(Agent));
        assert_eq!(Dock::from_json(&d.to_json()), d);
        // The first layout has them in their first places, under the same numbers.
        let first = Dock::default();
        ok(&first);
        assert!(matches!(first.find(4), Some(Node::Tabs { views, .. }) if *views == [Terminal, Output]));
        assert!(matches!(first.find(5), Some(Node::Tabs { views, .. }) if *views == [Agent, Tasks]));
        assert_eq!(first.side_bar(), Some(2));
        // None of its first place's views in the tree (the terminal floats): a group of its own, under the editor.
        let floating = Dock::default().floated(Terminal).floated(Output);
        also_ready(&[Scm, Problems, Sessions, Output, Tasks, DebugConsole]);
        let d = floating.normalized();
        ok(&d);
        let console = d.group_of(DebugConsole).expect("there");
        let Node::Split { kids, .. } = &d.root else { panic!() };
        assert!(
            matches!(&kids[1], Node::Split { dir: Dir::Column, kids, .. }
                if kids[0] == Node::Editor && kids[1].id() == console),
            "{d:?}"
        );
        // A view that is in the window no more (a layout a later Code Arena kept) is in no group.
        also_ready(&[]);
        let d = Dock::from_json(&d.to_json());
        ok(&d);
        assert_eq!(d.group_of(Scm), None);
        assert_eq!(d.group_of(DebugConsole), None);
    }

    #[test]
    fn views_withheld_are_out_of_what_is_drawn_and_the_parts_that_stay_keep_their_numbers() {
        let d = Dock::default();
        assert_eq!(d.without_all(&[]), d);
        // One of a group's views: the group stays, without it, another shown.
        let shown = d.shown(Search).without_all(&[Search]);
        assert!(matches!(shown.find(2), Some(Node::Tabs { views, active: Files, .. }) if !views.contains(&Search)));
        // A group's only view: the group is gone, the split it left with one part folded, the others as numbered.
        let shown = d.without_all(&[Terminal]);
        assert_eq!(shown.group_of(Terminal), None);
        let Node::Split { dir: Dir::Row, kids, .. } = &shown.root else { panic!() };
        assert_eq!(kids.iter().map(Node::id).collect::<Vec<_>>(), [2, EDITOR, 5]);
        assert!(!d.floated(Agent).without_all(&[Agent]).floating.contains(&Agent));
    }

    #[test]
    fn the_guide_square_under_the_pointer() {
        let r = Rect::new(0.0, 0.0, 400.0, 300.0);
        let g = guide(r);
        assert_eq!(zone(&g, Point::new(200.0, 150.0)), Some(Side::Center));
        assert_eq!(zone(&g, Point::new(200.0 - GUIDE - GAP, 150.0)), Some(Side::Left));
        assert_eq!(zone(&g, Point::new(200.0, 150.0 + GUIDE + GAP)), Some(Side::Bottom));
        assert_eq!(zone(&g, Point::new(10.0, 10.0)), None);
        assert_eq!(preview(r, Side::Right, false), Rect::new(200.0, 0.0, 400.0, 300.0));
        let e = edge_guides(Rect::new(0.0, 0.0, 1000.0, 800.0));
        assert_eq!(zone(&e, Point::new(20.0, 400.0)), Some(Side::Left));
    }
}
