//! The window's layout as Visual Studio docks its tool windows: a tree of splits (a row or a column of parts) and
//! tab groups (views, the one shown), with the editor a fixed leaf. A view is docked to a side of any part, or into a
//! group's tabs; each view is in the tree once, the editor once, no group is empty, no split has one part only, and
//! a split in the same direction as its parent is folded into it. Sizes are kept apart (by a part's number), so a
//! sash only sets a size and the tree is not built again.

use floem::kurbo::{Point, Rect};
use serde_json::{Value, json};

use crate::layout::{VIEWS, View};

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
}

/// The parts' first sizes (by number): the default tree's side bar, panel and secondary side bar, as VS Code's.
pub const FIRST_SIZES: [(u32, f64); 3] = [(2, 260.0), (4, 230.0), (5, 380.0)];
/// A new group's first size.
pub const NEW_SIZE: f64 = 280.0;

impl Default for Dock {
    /// VS Code's own: the side bar, the editor over the panel, the secondary side bar.
    fn default() -> Dock {
        Dock {
            root: Node::Split {
                id: 1,
                dir: Dir::Row,
                kids: vec![
                    Node::Tabs {
                        id: 2,
                        views: vec![View::Files, View::Search, View::Changes, View::Outline],
                        active: View::Files,
                    },
                    Node::Split {
                        id: 3,
                        dir: Dir::Column,
                        kids: vec![
                            Node::Editor,
                            Node::Tabs { id: 4, views: vec![View::Terminal], active: View::Terminal },
                        ],
                    },
                    Node::Tabs { id: 5, views: vec![View::Agent], active: View::Agent },
                ],
            },
            next: 6,
        }
    }
}

impl Dock {
    pub fn find(&self, id: u32) -> Option<&Node> {
        self.root.find(id)
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

    /// `v` docked against the part `target` (`ROOT`: the window's edge): beside it, or into its tabs.
    pub fn docked(&self, v: View, target: u32, side: Side) -> Dock {
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

    /// Kept whole: no empty group, no split of one part, same-direction splits folded, each view once (one that is
    /// missing beside the editor, on the right), the editor once.
    pub fn normalized(&self) -> Dock {
        self.tidied(true)
    }

    /// The same, a missing view put back only when `fill` (not while one is being moved).
    fn tidied(&self, fill: bool) -> Dock {
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
        let mut seen = Vec::new();
        let mut editor = false;
        let mut root = clean(self.root.clone(), &mut seen, &mut editor).unwrap_or(Node::Editor);
        let mut next = self.next;
        if !editor {
            root = Node::Split { id: next, dir: Dir::Row, kids: vec![root, Node::Editor] };
            next += 1;
        }
        let missing: Vec<View> = VIEWS.into_iter().filter(|v| fill && !seen.contains(v)).collect();
        let mut d = Dock { root, next };
        if let Some(first) = missing.first() {
            let group = Node::Tabs { id: d.next, views: missing.clone(), active: *first };
            d.next += 1;
            d.root = match d.root {
                Node::Split { id, dir: Dir::Row, mut kids } => {
                    kids.push(group);
                    Node::Split { id, dir: Dir::Row, kids }
                }
                other => {
                    let id = d.next;
                    d.next += 1;
                    Node::Split { id, dir: Dir::Row, kids: vec![other, group] }
                }
            };
        }
        // Numbers stay above every part's, so a new part never takes one in use.
        let mut ids = Vec::new();
        fn all_ids(n: &Node, out: &mut Vec<u32>) {
            out.push(n.id());
            if let Node::Split { kids, .. } = n {
                kids.iter().for_each(|k| all_ids(k, out));
            }
        }
        all_ids(&d.root, &mut ids);
        d.next = d.next.max(ids.iter().copied().filter(|i| *i != ROOT).max().unwrap_or(0) + 1);
        d
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
        json!({"root": node(&self.root), "next": self.next})
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
            Some(root) => Dock { root, next: v["next"].as_u64().unwrap_or(0) as u32 }.normalized(),
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
        assert_eq!(views.len(), VIEWS.len(), "every view: {d:?}");
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
            let v = VIEWS[(n >> 33) as usize % VIEWS.len()];
            let mut targets = vec![EDITOR, ROOT];
            d.root.groups(&mut targets);
            let target = targets[(n >> 40) as usize % targets.len()];
            d = d.docked(v, target, sides[(n >> 50) as usize % sides.len()]);
            ok(&d);
            assert_eq!(Dock::from_json(&d.to_json()), d, "kept and read back as it was");
        }
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
