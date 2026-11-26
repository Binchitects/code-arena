// Colour themes in VS Code's own format: a name, a type (dark, light, hc, hcLight), its workbench `colors` by VS
// Code's ids, and its `tokenColors` (TextMate scopes). The built-in ones are crates/ca-themes/themes; your own come from the
// themes/ folders (a theme's JSON, or a VS Code theme extension's folder) and work the same way.

import darkModern from "../../crates/ca-themes/themes/dark-modern.json" with { type: "json" };
import lightModern from "../../crates/ca-themes/themes/light-modern.json" with { type: "json" };
import darkPlus from "../../crates/ca-themes/themes/dark-plus.json" with { type: "json" };
import lightPlus from "../../crates/ca-themes/themes/light-plus.json" with { type: "json" };
import hcBlack from "../../crates/ca-themes/themes/hc-black.json" with { type: "json" };
import hcLight from "../../crates/ca-themes/themes/hc-light.json" with { type: "json" };
import monokai from "../../crates/ca-themes/themes/monokai.json" with { type: "json" };
import solarizedDark from "../../crates/ca-themes/themes/solarized-dark.json" with { type: "json" };
import solarizedLight from "../../crates/ca-themes/themes/solarized-light.json" with { type: "json" };

export type TokenColor = {
  name?: string;
  scope?: string | string[];
  settings: { foreground?: string; background?: string; fontStyle?: string };
};

export type Theme = {
  id: string;
  name: string;
  type: "dark" | "light" | "hc" | "hcLight";
  colors: Record<string, string>;
  tokenColors: TokenColor[];
  /** Where it came from: "built in", or the file. */
  from?: string;
};

// The built-in themes are VS Code theme JSON in crates/ca-themes/themes: the native app reads the same files.
const RAW: [string, Omit<Theme, "id">][] = [
  ["dark-modern", darkModern as Omit<Theme, "id">],
  ["light-modern", lightModern as Omit<Theme, "id">],
  ["dark-plus", darkPlus as Omit<Theme, "id">],
  ["light-plus", lightPlus as Omit<Theme, "id">],
  ["hc-black", hcBlack as Omit<Theme, "id">],
  ["hc-light", hcLight as Omit<Theme, "id">],
  ["monokai", monokai as Omit<Theme, "id">],
  ["solarized-dark", solarizedDark as Omit<Theme, "id">],
  ["solarized-light", solarizedLight as Omit<Theme, "id">],
];

export const BUILT_IN: Theme[] = RAW.map(([id, t]) => ({ id, ...t, from: "built in" }));

const isDark = (th: Theme) => th.type === "dark" || th.type === "hc";

/** A VS Code colour id's value, or the first of the fallbacks that the theme has, else the theme type's default. */
function pick(th: Theme, ids: string[], dark: string, light: string): string {
  for (const id of ids) {
    const v = th.colors[id];
    if (v && /^#[0-9a-f]{3,8}$/i.test(v)) return v;
  }
  return isDark(th) ? dark : light;
}

/** The page's own colours (CSS variables) from a theme's workbench colours. */
export function cssVars(th: Theme): Record<string, string> {
  const fg = pick(th, ["foreground", "editor.foreground"], "#CCCCCC", "#3B3B3B");
  return {
    "--bg": pick(th, ["editor.background"], "#1F1F1F", "#FFFFFF"),
    "--panel": pick(th, ["sideBar.background", "panel.background"], "#181818", "#F8F8F8"),
    "--text": fg,
    "--muted": pick(th, ["descriptionForeground"], "#9D9D9D", "#6F6F6F"),
    "--line": pick(th, ["panel.border", "sideBar.border", "widget.border", "contrastBorder"], "#2B2B2B", "#E5E5E5"),
    "--accent": pick(th, ["focusBorder", "button.background"], "#0078D4", "#005FB8"),
    "--button": pick(th, ["button.background"], "#0078D4", "#005FB8"),
    "--accent-text": pick(th, ["button.foreground"], "#FFFFFF", "#FFFFFF"),
    "--user": pick(th, ["list.hoverBackground", "input.background"], "#2A2D2E", "#F2F2F2"),
    "--selection": pick(th, ["list.activeSelectionBackground"], "#37373D", "#E8E8E8"),
    "--code": pick(th, ["textCodeBlock.background", "editorWidget.background"], "#2B2B2B", "#F8F8F8"),
    "--input": pick(th, ["input.background"], "#313131", "#FFFFFF"),
    "--danger": pick(th, ["errorForeground"], "#F85149", "#C72E0F"),
    "--ok": pick(th, ["testing.iconPassed", "terminal.ansiGreen"], "#73C991", "#388A34"),
    "--activity": pick(th, ["activityBar.background"], "#181818", "#F8F8F8"),
    "--activity-fg": pick(th, ["activityBar.foreground"], "#D7D7D7", "#1F1F1F"),
    "--activity-border": pick(th, ["activityBar.activeBorder", "focusBorder"], "#0078D4", "#005FB8"),
    "--title": pick(th, ["titleBar.activeBackground"], "#181818", "#F8F8F8"),
    "--title-fg": pick(th, ["titleBar.activeForeground"], fg, fg),
    "--status": pick(th, ["statusBar.background"], "#181818", "#F8F8F8"),
    "--status-fg": pick(th, ["statusBar.foreground"], "#CCCCCC", "#3B3B3B"),
    "--tab": pick(th, ["tab.inactiveBackground", "editorGroupHeader.tabsBackground"], "#181818", "#F8F8F8"),
    "--tab-active": pick(th, ["tab.activeBackground", "editor.background"], "#1F1F1F", "#FFFFFF"),
    "--tab-border-top": pick(th, ["tab.activeBorderTop", "focusBorder"], "#0078D4", "#005FB8"),
    "--terminal": pick(th, ["terminal.background", "panel.background"], "#181818", "#F8F8F8"),
  };
}

/** Monaco's name for what a TextMate scope colours (its languages' tokens are named so). */
export function monacoTokens(scope: string): string[] {
  const s = scope.trim();
  const out: string[] = [];
  const add = (...t: string[]) => out.push(...t);
  if (s.startsWith("comment")) add("comment");
  else if (s.startsWith("string.regexp")) add("regexp");
  else if (s.startsWith("string")) add("string");
  else if (s.startsWith("constant.numeric")) add("number");
  else if (s.startsWith("constant.language")) add("constant", "keyword.constant");
  else if (s.startsWith("constant")) add("constant");
  else if (s.startsWith("keyword.operator")) add("operator", "delimiter");
  else if (s.startsWith("keyword.control")) add("keyword.flow", "keyword.control");
  else if (s.startsWith("keyword") || s.startsWith("storage")) add("keyword");
  else if (/^(entity\.name\.(type|class)|support\.(type|class))/.test(s)) add("type", "type.identifier");
  else if (/^(entity\.name\.function|support\.function)/.test(s)) add("function", "predefined");
  else if (s.startsWith("variable")) add("variable", "identifier");
  else if (s.startsWith("entity.name.tag")) add("tag");
  else if (s.startsWith("entity.other.attribute-name")) add("attribute.name");
  else if (/^(meta\.annotation|entity\.name\.decorator|storage\.type\.annotation)/.test(s)) add("annotation");
  else if (s.startsWith("markup.heading")) add("keyword.md", "markup.heading");
  else if (s.startsWith("invalid")) add("invalid");
  return out;
}

export type MonacoRule = { token: string; foreground?: string; background?: string; fontStyle?: string };

/** A theme's token colours as Monaco's rules; the more general first, so the more particular win. */
export function monacoRules(th: Theme): MonacoRule[] {
  const rules: MonacoRule[] = [];
  const hex = (c?: string) => (c && /^#[0-9a-f]{3,8}$/i.test(c) ? c.slice(1) : undefined);
  for (const tc of th.tokenColors ?? []) {
    const scopes = Array.isArray(tc.scope) ? tc.scope : (tc.scope ?? "").split(",");
    for (const scope of scopes) {
      for (const token of monacoTokens(scope)) {
        const rule: MonacoRule = { token };
        const f = hex(tc.settings?.foreground);
        const b = hex(tc.settings?.background);
        if (f) rule.foreground = f;
        if (b) rule.background = b;
        if (tc.settings?.fontStyle !== undefined) rule.fontStyle = tc.settings.fontStyle;
        if (f || b || rule.fontStyle !== undefined) rules.push(rule);
      }
    }
  }
  return rules;
}

/** The theme's colours Monaco takes (VS Code's ids, valid colours only). */
export function monacoColors(th: Theme): Record<string, string> {
  return Object.fromEntries(Object.entries(th.colors).filter(([, v]) => /^#[0-9a-f]{3,8}$/i.test(v)));
}

/** The terminal's colours from a theme (xterm.js's names). */
export function terminalColors(th: Theme): Record<string, string> {
  const c = (id: string, d: string) => th.colors[id] ?? d;
  const v = cssVars(th);
  const ansi = ["Black", "Red", "Green", "Yellow", "Blue", "Magenta", "Cyan", "White"];
  const out: Record<string, string> = {
    background: v["--terminal"],
    foreground: c("terminal.foreground", v["--text"]),
    cursor: c("terminalCursor.foreground", v["--text"]),
    selectionBackground: c("terminal.selectionBackground", isDark(th) ? "#264F78" : "#ADD6FF"),
  };
  for (const a of ansi) {
    out[a.toLowerCase()] = c(`terminal.ansi${a}`, "");
    out[`bright${a}`] = c(`terminal.ansiBright${a}`, "");
  }
  return Object.fromEntries(Object.entries(out).filter(([, x]) => x));
}

/** Whether the theme is dark (for the Monaco base, and for "follow the system"). */
export function base(th: Theme): "vs-dark" | "vs" | "hc-black" | "hc-light" {
  return th.type === "hc" ? "hc-black" : th.type === "hcLight" ? "hc-light" : th.type === "dark" ? "vs-dark" : "vs";
}

/** The theme a choice names: by id or name (any case); "" follows the system (Dark Modern, Light Modern). */
export function chosen(all: Theme[], choice: string, systemDark: boolean): Theme {
  const c = choice.trim().toLowerCase();
  const found = c ? all.find((th) => th.id.toLowerCase() === c || th.name.toLowerCase() === c) : undefined;
  return found ?? (systemDark ? BUILT_IN[0] : BUILT_IN[1]);
}
