import assert from "node:assert/strict";
import { test } from "node:test";
import { BUILT_IN, type Theme, chosen, cssVars, monacoRules, monacoTokens, terminalColors } from "./themes.ts";

test("every built-in theme gives every colour the page uses", () => {
  const names = Object.keys(cssVars(BUILT_IN[0]));
  for (const th of BUILT_IN) {
    const v = cssVars(th);
    for (const n of names) assert.match(v[n], /^#[0-9a-f]{3,8}$/i, `${th.name} ${n}`);
  }
  assert.equal(new Set(BUILT_IN.map((t) => t.id)).size, BUILT_IN.length, "ids are unique");
});

test("a theme of one's own with few colours takes the rest from its type", () => {
  const mine: Theme = { id: "mine", name: "Mine", type: "light", colors: { "editor.background": "#fafafa", bogus: "red" }, tokenColors: [] };
  const v = cssVars(mine);
  assert.equal(v["--bg"], "#fafafa");
  assert.equal(v["--panel"], "#F8F8F8", "light's own default");
});

test("TextMate scopes become Monaco's tokens", () => {
  assert.deepEqual(monacoTokens("comment.line.double-slash"), ["comment"]);
  assert.deepEqual(monacoTokens("keyword.control.flow"), ["keyword.flow", "keyword.control"]);
  assert.deepEqual(monacoTokens("entity.name.type.class"), ["type", "type.identifier"]);
  assert.deepEqual(monacoTokens("meta.something.else"), []);
  const rules = monacoRules({
    id: "x",
    name: "X",
    type: "dark",
    colors: {},
    tokenColors: [{ scope: ["string", "constant.numeric"], settings: { foreground: "#ff0000", fontStyle: "italic" } }, { scope: "comment", settings: { foreground: "not a colour" } }],
  });
  assert.deepEqual(rules, [
    { token: "string", foreground: "ff0000", fontStyle: "italic" },
    { token: "number", foreground: "ff0000", fontStyle: "italic" },
  ]);
});

test("the choice by id or name, else the system's", () => {
  assert.equal(chosen(BUILT_IN, "monokai", true).name, "Monokai");
  assert.equal(chosen(BUILT_IN, "Solarized Light", true).id, "solarized-light");
  assert.equal(chosen(BUILT_IN, "", true).id, "dark-modern");
  assert.equal(chosen(BUILT_IN, "", false).id, "light-modern");
  assert.equal(chosen(BUILT_IN, "gone", false).id, "light-modern");
  assert.equal(terminalColors(BUILT_IN[0]).brightGreen, "#23D18B");
});
