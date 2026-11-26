import assert from "node:assert/strict";
import { test } from "node:test";
import { defaults, move, normalize, reveal, toggle, toggleView, where } from "./layout.ts";

test("a view moves to another container, shown there; where it was shows the next", () => {
  let l = move(defaults(), "search", "panel");
  assert.deepEqual(l.panel, ["terminal", "search"]);
  assert.equal(l.active.panel, "search");
  assert.deepEqual(l.primary, ["files", "changes", "outline"]);
  assert.equal(l.active.primary, "files");
  // The active one moved: the one after it shows.
  l = move(reveal(l, "changes"), "changes", "secondary", 0);
  assert.deepEqual(l.secondary, ["changes", "agent"]);
  assert.equal(l.active.primary, "outline");
  // The last one moved out: its container is hidden.
  l = move(l, "agent", "panel");
  l = move(l, "changes", "primary");
  assert.equal(l.secondary.length, 0);
  assert.equal(l.shown.secondary, false);
  assert.equal(where(l, "agent"), "panel");
});

test("moved within its container: to its new place", () => {
  const l = move(defaults(), "files", "primary", 3);
  assert.deepEqual(l.primary, ["search", "changes", "files", "outline"]);
  assert.deepEqual(move(defaults(), "outline", "primary", 0).primary, ["outline", "files", "search", "changes"]);
});

test("the activity bar's click shows a view, or hides the side bar when it is shown", () => {
  let l = toggleView(defaults(), "search");
  assert.equal(l.active.primary, "search");
  assert.equal(l.shown.primary, true);
  l = toggleView(l, "search");
  assert.equal(l.shown.primary, false);
  assert.equal(toggle(l, "primary").shown.primary, true);
});

test("a kept layout is read kindly: unknown views out, missing ones where the defaults put them", () => {
  const l = normalize({ primary: ["files", "bogus", "files"], secondary: ["terminal"], panel: [], active: { secondary: "terminal" }, shown: { panel: true }, side: "right", panelAt: "sideways" });
  assert.deepEqual(l.primary, ["files", "search", "changes", "outline"]);
  assert.deepEqual(l.secondary, ["terminal", "agent"]);
  assert.deepEqual(l.panel, []);
  assert.equal(l.shown.panel, false, "an empty container is not shown");
  assert.equal(l.side, "right");
  assert.equal(l.panelAt, "bottom");
  assert.deepEqual(normalize(null), defaults());
});
