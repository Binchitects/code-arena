// A file's outline (its functions, types, classes, headings) read by line, for the languages people use most. It
// gives the Outline view, Go to Symbol (Ctrl+Shift+O) and the editor's breadcrumbs. It reads by pattern, not by
// grammar: good for finding one's way, not for refactoring.

export type Kind = "function" | "method" | "class" | "struct" | "enum" | "interface" | "type" | "module" | "constant" | "heading";

export type Symbol = { name: string; kind: Kind; line: number; depth: number };

type Rule = { re: RegExp; kind: Kind };

const RUST: Rule[] = [
  { re: /^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:const\s+)?(?:unsafe\s+)?(?:extern\s+"[^"]*"\s+)?fn\s+([A-Za-z_]\w*)/, kind: "function" },
  { re: /^\s*(?:pub(?:\([^)]*\))?\s+)?struct\s+([A-Za-z_]\w*)/, kind: "struct" },
  { re: /^\s*(?:pub(?:\([^)]*\))?\s+)?enum\s+([A-Za-z_]\w*)/, kind: "enum" },
  { re: /^\s*(?:pub(?:\([^)]*\))?\s+)?(?:unsafe\s+)?trait\s+([A-Za-z_]\w*)/, kind: "interface" },
  { re: /^\s*(?:pub(?:\([^)]*\))?\s+)?type\s+([A-Za-z_]\w*)/, kind: "type" },
  { re: /^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_]\w*)/, kind: "module" },
  { re: /^\s*impl(?:<[^>]*>)?\s+(?:[\w:<>, ]+\s+for\s+)?([A-Za-z_][\w:]*)/, kind: "class" },
  { re: /^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const|static)\s+([A-Z_][A-Z0-9_]*)\s*:/, kind: "constant" },
];

const TS: Rule[] = [
  { re: /^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?function\*?\s+([A-Za-z_$][\w$]*)/, kind: "function" },
  { re: /^\s*(?:export\s+)?(?:default\s+)?(?:abstract\s+)?class\s+([A-Za-z_$][\w$]*)/, kind: "class" },
  { re: /^\s*(?:export\s+)?interface\s+([A-Za-z_$][\w$]*)/, kind: "interface" },
  { re: /^\s*(?:export\s+)?type\s+([A-Za-z_$][\w$]*)\s*[=<]/, kind: "type" },
  { re: /^\s*(?:export\s+)?(?:const\s+)?enum\s+([A-Za-z_$][\w$]*)/, kind: "enum" },
  { re: /^\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*(?::[^=]*)?=\s*(?:async\s+)?(?:\([^)]*\)|[A-Za-z_$][\w$]*)\s*(?::[^=]*)?=>/, kind: "function" },
  { re: /^\s+(?:public\s+|private\s+|protected\s+|static\s+|readonly\s+|async\s+|override\s+)*([A-Za-z_$][\w$]*)\s*\([^)]*\)\s*(?::[^{]*)?\{\s*$/, kind: "method" },
];

const PYTHON: Rule[] = [
  { re: /^\s*(?:async\s+)?def\s+([A-Za-z_]\w*)/, kind: "function" },
  { re: /^\s*class\s+([A-Za-z_]\w*)/, kind: "class" },
];

const GO: Rule[] = [
  { re: /^func\s+\([^)]*\)\s*([A-Za-z_]\w*)/, kind: "method" },
  { re: /^func\s+([A-Za-z_]\w*)/, kind: "function" },
  { re: /^type\s+([A-Za-z_]\w*)\s+struct/, kind: "struct" },
  { re: /^type\s+([A-Za-z_]\w*)\s+interface/, kind: "interface" },
  { re: /^type\s+([A-Za-z_]\w*)/, kind: "type" },
];

const C_LIKE: Rule[] = [
  { re: /^\s*(?:public|private|protected|internal|static|sealed|abstract|partial|\s)*\s*(?:class|record)\s+([A-Za-z_]\w*)/, kind: "class" },
  { re: /^\s*(?:public|private|protected|internal|\s)*\s*(?:struct)\s+([A-Za-z_]\w*)/, kind: "struct" },
  { re: /^\s*(?:public|private|protected|internal|\s)*\s*interface\s+([A-Za-z_]\w*)/, kind: "interface" },
  { re: /^\s*(?:public|private|protected|internal|\s)*\s*enum\s+(?:class\s+)?([A-Za-z_]\w*)/, kind: "enum" },
  { re: /^\s*(?:namespace|package)\s+([A-Za-z_][\w.]*)/, kind: "module" },
  {
    re: /^\s*(?:public|private|protected|internal|static|virtual|override|async|final|inline|extern|const|unsafe|\s)*[A-Za-z_][\w<>,:*&\[\] ]*\s+\**([A-Za-z_]\w*)\s*\([^;]*\)\s*(?:const\s*)?(?:\{|$)/,
    kind: "function",
  },
];

const RULES: Record<string, Rule[]> = {
  rust: RUST,
  typescript: TS,
  javascript: TS,
  python: PYTHON,
  go: GO,
  java: C_LIKE,
  csharp: C_LIKE,
  cpp: C_LIKE,
  c: C_LIKE,
  kotlin: C_LIKE,
  swift: C_LIKE,
};

const KEYWORDS = new Set(["if", "for", "while", "switch", "catch", "return", "else", "do", "new", "delete", "sizeof"]);

/** The file's symbols in order, each at its line (from 1) and its depth (by indentation, or a heading's level). */
export function outline(text: string, language: string): Symbol[] {
  const lines = text.split(/\r?\n/);
  if (language === "markdown") {
    const out: Symbol[] = [];
    let fence = false;
    lines.forEach((l, i) => {
      if (/^\s*(```|~~~)/.test(l)) fence = !fence;
      const m = !fence && /^(#{1,6})\s+(.+?)\s*#*\s*$/.exec(l);
      if (m) out.push({ name: m[2], kind: "heading", line: i + 1, depth: m[1].length - 1 });
    });
    return out;
  }
  const rules = RULES[language];
  if (!rules) return [];
  const out: Symbol[] = [];
  const indents: number[] = [];
  let block = false;
  lines.forEach((l, i) => {
    // Inside a block comment: no symbols.
    if (block) {
      if (l.includes("*/")) block = false;
      return;
    }
    if (/^\s*\/\*/.test(l) && !l.includes("*/")) {
      block = true;
      return;
    }
    if (/^\s*(\/\/|#(?!\[)|\*)/.test(l) && language !== "rust") return;
    if (/^\s*\/\//.test(l)) return;
    for (const r of rules) {
      const m = r.re.exec(l);
      if (!m || KEYWORDS.has(m[1])) continue;
      const indent = l.length - l.trimStart().length;
      while (indents.length && indents[indents.length - 1] >= indent) indents.pop();
      out.push({ name: m[1], kind: r.kind, line: i + 1, depth: indents.length });
      indents.push(indent);
      break;
    }
  });
  return out;
}
