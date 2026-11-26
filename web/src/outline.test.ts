// Run with: npm test (Node's own runner; no test framework to fetch).
import assert from "node:assert/strict";
import { test } from "node:test";
import { outline } from "./outline.ts";

const names = (text: string, lang: string) => outline(text, lang).map((s) => `${"  ".repeat(s.depth)}${s.kind} ${s.name}:${s.line}`);

test("rust: items and what an impl holds", () => {
  const src = [
    "pub struct Cart {",
    "    items: Vec<u32>,",
    "}",
    "",
    "impl Cart {",
    "    pub fn total(&self) -> u32 {",
    "        if self.items.is_empty() { return 0; }",
    "        self.items.iter().sum()",
    "    }",
    "}",
    "// fn not_this() {}",
    "pub(crate) async fn load() {}",
    "const MOST: usize = 3;",
  ].join("\n");
  assert.deepEqual(names(src, "rust"), ["struct Cart:1", "class Cart:5", "  function total:6", "function load:12", "constant MOST:13"]);
});

test("typescript: functions, classes, their methods, and arrow functions", () => {
  const src = [
    "export function score(a: string): number {",
    "  return 1;",
    "}",
    "export class Store {",
    "  private add(x: number): void {",
    "    if (x) {",
    "    }",
    "  }",
    "}",
    "export const go = async (x: number) => x;",
    "interface Tab { path: string }",
    "/* function hidden() {}",
    "*/",
  ].join("\n");
  assert.deepEqual(names(src, "typescript"), ["function score:1", "class Store:4", "  method add:5", "function go:10", "interface Tab:11"]);
});

test("python and markdown", () => {
  assert.deepEqual(names("class A:\n    def run(self):\n        pass\ndef main():\n    pass\n", "python"), ["class A:1", "  function run:2", "function main:4"]);
  assert.deepEqual(names("# Title\n\n## Part\n```\n# not a heading\n```\n### Deeper\n", "markdown"), ["heading Title:1", "  heading Part:3", "    heading Deeper:7"]);
  assert.deepEqual(names("anything", "plaintext"), []);
});
