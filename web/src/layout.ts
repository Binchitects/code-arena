// Coder mode's layout as VS Code's: views in three containers (the primary side bar, the secondary side bar, the
// panel), each with the view it shows and whether it is shown, the primary side bar left or right and the panel at the
// bottom, left or right. A view moves to any container; the layout is kept per folder.

export type ViewId = "files" | "search" | "changes" | "outline" | "agent" | "terminal";
export type Where = "primary" | "secondary" | "panel";

export type Layout = {
  primary: ViewId[];
  secondary: ViewId[];
  panel: ViewId[];
  active: Record<Where, ViewId | null>;
  shown: Record<Where, boolean>;
  side: "left" | "right";
  panelAt: "bottom" | "left" | "right";
};

export const VIEWS: Record<ViewId, { title: string; icon: string; keys?: string }> = {
  files: { title: "Explorer", icon: "files", keys: "Ctrl+Shift+E" },
  search: { title: "Search", icon: "search", keys: "Ctrl+Shift+F" },
  changes: { title: "Changes", icon: "git-compare" },
  outline: { title: "Outline", icon: "symbol-class" },
  agent: { title: "Agent", icon: "comment-discussion", keys: "Ctrl+Alt+I" },
  terminal: { title: "Terminal", icon: "terminal", keys: "Ctrl+`" },
};

export const WHERE: Where[] = ["primary", "secondary", "panel"];

export function defaults(): Layout {
  return {
    primary: ["files", "search", "changes", "outline"],
    secondary: ["agent"],
    panel: ["terminal"],
    active: { primary: "files", secondary: "agent", panel: "terminal" },
    shown: { primary: true, secondary: true, panel: true },
    side: "left",
    panelAt: "bottom",
  };
}

const copy = (l: Layout): Layout => ({
  ...l,
  primary: [...l.primary],
  secondary: [...l.secondary],
  panel: [...l.panel],
  active: { ...l.active },
  shown: { ...l.shown },
});

/** The container a view is in. */
export function where(l: Layout, v: ViewId): Where | undefined {
  return WHERE.find((w) => l[w].includes(v));
}

/** `v` moved to `to` (before the view at `index`, else last): shown there; where it was shows another. */
export function move(l: Layout, v: ViewId, to: Where, index?: number): Layout {
  const n = copy(l);
  const from = where(n, v);
  if (from) {
    const at = n[from].indexOf(v);
    n[from].splice(at, 1);
    if (from === to && index !== undefined && index > at) index -= 1;
    if (n.active[from] === v) n.active[from] = n[from][Math.min(at, n[from].length - 1)] ?? null;
    if (!n[from].length) n.shown[from] = false;
  }
  const i = index === undefined ? n[to].length : Math.max(0, Math.min(index, n[to].length));
  n[to].splice(i, 0, v);
  n.active[to] = v;
  n.shown[to] = true;
  return n;
}

/** `v` on screen: its container shown, and it the one shown in it. */
export function reveal(l: Layout, v: ViewId): Layout {
  const w = where(l, v);
  if (!w) return l;
  const n = copy(l);
  n.active[w] = v;
  n.shown[w] = true;
  return n;
}

/** An activity bar's click (and a view's key): shown, or hidden when it is shown already. */
export function toggleView(l: Layout, v: ViewId): Layout {
  const w = where(l, v);
  if (!w) return l;
  if (l.shown[w] && l.active[w] === v) {
    const n = copy(l);
    n.shown[w] = false;
    return n;
  }
  return reveal(l, v);
}

export function toggle(l: Layout, w: Where): Layout {
  const n = copy(l);
  n.shown[w] = !n.shown[w] && n[w].length > 0;
  if (n.shown[w] && !n.active[w]) n.active[w] = n[w][0];
  return n;
}

/** A layout kept before (or by another version): every view once, in a container; what it lacks from the defaults. */
export function normalize(saved: unknown): Layout {
  const d = defaults();
  if (!saved || typeof saved !== "object") return d;
  const s = saved as Partial<Layout>;
  const n = copy(d);
  const seen = new Set<ViewId>();
  for (const w of WHERE) {
    n[w] = (Array.isArray(s[w]) ? s[w] : []).filter((v): v is ViewId => v in VIEWS && !seen.has(v as ViewId) && !!seen.add(v as ViewId));
  }
  // A view the kept layout does not have goes where the defaults put it.
  for (const w of WHERE) for (const v of d[w]) if (!seen.has(v)) n[w].push(v);
  for (const w of WHERE) {
    const a = s.active?.[w];
    n.active[w] = a && n[w].includes(a) ? a : (n[w][0] ?? null);
    n.shown[w] = typeof s.shown?.[w] === "boolean" ? s.shown[w] && n[w].length > 0 : n[w].length > 0;
  }
  n.side = s.side === "right" ? "right" : "left";
  n.panelAt = s.panelAt === "left" || s.panelAt === "right" ? s.panelAt : "bottom";
  return n;
}
