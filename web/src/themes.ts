// Colour themes in VS Code's own format: a name, a type (dark, light, hc, hcLight), its workbench `colors` by VS
// Code's ids, and its `tokenColors` (TextMate scopes). The built-in ones are written here; your own come from the
// themes/ folders (a theme's JSON, or a VS Code theme extension's folder) and work the same way.

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

const t = (scope: string, foreground: string, fontStyle?: string): TokenColor => ({ scope, settings: { foreground, fontStyle } });

const DARK_TOKENS: TokenColor[] = [
  t("comment", "#6A9955"),
  t("string", "#CE9178"),
  t("string.regexp", "#D16969"),
  t("constant.numeric", "#B5CEA8"),
  t("constant.language", "#569CD6"),
  t("keyword, storage", "#569CD6"),
  t("keyword.control", "#C586C0"),
  t("keyword.operator", "#D4D4D4"),
  t("entity.name.type, support.type, entity.name.class", "#4EC9B0"),
  t("entity.name.function, support.function", "#DCDCAA"),
  t("variable", "#9CDCFE"),
  t("entity.name.tag", "#569CD6"),
  t("entity.other.attribute-name", "#9CDCFE"),
  t("meta.annotation, entity.name.decorator", "#DCDCAA"),
  t("markup.heading", "#569CD6", "bold"),
  t("invalid", "#F44747"),
];

const LIGHT_TOKENS: TokenColor[] = [
  t("comment", "#008000"),
  t("string", "#A31515"),
  t("string.regexp", "#811F3F"),
  t("constant.numeric", "#098658"),
  t("constant.language", "#0000FF"),
  t("keyword, storage", "#0000FF"),
  t("keyword.control", "#AF00DB"),
  t("keyword.operator", "#000000"),
  t("entity.name.type, support.type, entity.name.class", "#267F99"),
  t("entity.name.function, support.function", "#795E26"),
  t("variable", "#001080"),
  t("entity.name.tag", "#800000"),
  t("entity.other.attribute-name", "#E50000"),
  t("meta.annotation, entity.name.decorator", "#795E26"),
  t("markup.heading", "#800000", "bold"),
  t("invalid", "#CD3131"),
];

const ANSI_DARK = {
  "terminal.ansiBlack": "#000000",
  "terminal.ansiRed": "#CD3131",
  "terminal.ansiGreen": "#0DBC79",
  "terminal.ansiYellow": "#E5E510",
  "terminal.ansiBlue": "#2472C8",
  "terminal.ansiMagenta": "#BC3FBC",
  "terminal.ansiCyan": "#11A8CD",
  "terminal.ansiWhite": "#E5E5E5",
  "terminal.ansiBrightBlack": "#666666",
  "terminal.ansiBrightRed": "#F14C4C",
  "terminal.ansiBrightGreen": "#23D18B",
  "terminal.ansiBrightYellow": "#F5F543",
  "terminal.ansiBrightBlue": "#3B8EEA",
  "terminal.ansiBrightMagenta": "#D670D6",
  "terminal.ansiBrightCyan": "#29B8DB",
  "terminal.ansiBrightWhite": "#E5E5E5",
};

const ANSI_LIGHT = {
  "terminal.ansiBlack": "#000000",
  "terminal.ansiRed": "#CD3131",
  "terminal.ansiGreen": "#00BC00",
  "terminal.ansiYellow": "#949800",
  "terminal.ansiBlue": "#0451A5",
  "terminal.ansiMagenta": "#BC05BC",
  "terminal.ansiCyan": "#0598BC",
  "terminal.ansiWhite": "#555555",
  "terminal.ansiBrightBlack": "#666666",
  "terminal.ansiBrightRed": "#CD3131",
  "terminal.ansiBrightGreen": "#14CE14",
  "terminal.ansiBrightYellow": "#B5BA00",
  "terminal.ansiBrightBlue": "#0451A5",
  "terminal.ansiBrightMagenta": "#BC05BC",
  "terminal.ansiBrightCyan": "#0598BC",
  "terminal.ansiBrightWhite": "#A5A5A5",
};

export const BUILT_IN: Theme[] = [
  {
    id: "dark-modern",
    name: "Dark Modern",
    type: "dark",
    tokenColors: DARK_TOKENS,
    colors: {
      ...ANSI_DARK,
      foreground: "#CCCCCC",
      descriptionForeground: "#9D9D9D",
      errorForeground: "#F85149",
      focusBorder: "#0078D4",
      "editor.background": "#1F1F1F",
      "editor.foreground": "#CCCCCC",
      "editorLineNumber.foreground": "#6E7681",
      "editorLineNumber.activeForeground": "#CCCCCC",
      "editor.lineHighlightBackground": "#2A2A2A",
      "editorWidget.background": "#202020",
      "sideBar.background": "#181818",
      "sideBar.border": "#2B2B2B",
      "activityBar.background": "#181818",
      "activityBar.foreground": "#D7D7D7",
      "activityBar.inactiveForeground": "#868686",
      "activityBar.activeBorder": "#0078D4",
      "statusBar.background": "#181818",
      "statusBar.foreground": "#CCCCCC",
      "statusBar.border": "#2B2B2B",
      "titleBar.activeBackground": "#181818",
      "titleBar.activeForeground": "#CCCCCC",
      "titleBar.border": "#2B2B2B",
      "panel.background": "#181818",
      "panel.border": "#2B2B2B",
      "tab.activeBackground": "#1F1F1F",
      "tab.inactiveBackground": "#181818",
      "tab.activeBorderTop": "#0078D4",
      "tab.border": "#2B2B2B",
      "editorGroupHeader.tabsBackground": "#181818",
      "button.background": "#0078D4",
      "button.foreground": "#FFFFFF",
      "button.secondaryBackground": "#313131",
      "input.background": "#313131",
      "input.border": "#3C3C3C",
      "list.hoverBackground": "#2A2D2E",
      "list.activeSelectionBackground": "#37373D",
      "textCodeBlock.background": "#2B2B2B",
      "widget.border": "#313131",
      "terminal.background": "#181818",
      "terminal.foreground": "#CCCCCC",
      "testing.iconPassed": "#73C991",
    },
  },
  {
    id: "light-modern",
    name: "Light Modern",
    type: "light",
    tokenColors: LIGHT_TOKENS,
    colors: {
      ...ANSI_LIGHT,
      foreground: "#3B3B3B",
      descriptionForeground: "#6F6F6F",
      errorForeground: "#C72E0F",
      focusBorder: "#005FB8",
      "editor.background": "#FFFFFF",
      "editor.foreground": "#3B3B3B",
      "editorLineNumber.foreground": "#6E7681",
      "editor.lineHighlightBackground": "#F5F5F5",
      "editorWidget.background": "#F8F8F8",
      "sideBar.background": "#F8F8F8",
      "sideBar.border": "#E5E5E5",
      "activityBar.background": "#F8F8F8",
      "activityBar.foreground": "#1F1F1F",
      "activityBar.inactiveForeground": "#616161",
      "activityBar.activeBorder": "#005FB8",
      "statusBar.background": "#F8F8F8",
      "statusBar.foreground": "#3B3B3B",
      "statusBar.border": "#E5E5E5",
      "titleBar.activeBackground": "#F8F8F8",
      "titleBar.activeForeground": "#1E1E1E",
      "titleBar.border": "#E5E5E5",
      "panel.background": "#F8F8F8",
      "panel.border": "#E5E5E5",
      "tab.activeBackground": "#FFFFFF",
      "tab.inactiveBackground": "#F8F8F8",
      "tab.activeBorderTop": "#005FB8",
      "tab.border": "#E5E5E5",
      "editorGroupHeader.tabsBackground": "#F8F8F8",
      "button.background": "#005FB8",
      "button.foreground": "#FFFFFF",
      "button.secondaryBackground": "#E5E5E5",
      "input.background": "#FFFFFF",
      "input.border": "#CECECE",
      "list.hoverBackground": "#F2F2F2",
      "list.activeSelectionBackground": "#E8E8E8",
      "textCodeBlock.background": "#F8F8F8",
      "widget.border": "#E5E5E5",
      "terminal.background": "#F8F8F8",
      "terminal.foreground": "#3B3B3B",
      "testing.iconPassed": "#388A34",
    },
  },
  {
    id: "dark-plus",
    name: "Dark+",
    type: "dark",
    tokenColors: DARK_TOKENS,
    colors: {
      ...ANSI_DARK,
      foreground: "#CCCCCC",
      descriptionForeground: "#A0A0A0",
      errorForeground: "#F48771",
      focusBorder: "#007FD4",
      "editor.background": "#1E1E1E",
      "editor.foreground": "#D4D4D4",
      "editorWidget.background": "#252526",
      "sideBar.background": "#252526",
      "sideBar.border": "#333333",
      "activityBar.background": "#333333",
      "activityBar.foreground": "#FFFFFF",
      "activityBar.inactiveForeground": "#FFFFFF66",
      "activityBar.activeBorder": "#FFFFFF",
      "statusBar.background": "#007ACC",
      "statusBar.foreground": "#FFFFFF",
      "titleBar.activeBackground": "#3C3C3C",
      "titleBar.activeForeground": "#CCCCCC",
      "panel.background": "#1E1E1E",
      "panel.border": "#80808059",
      "tab.activeBackground": "#1E1E1E",
      "tab.inactiveBackground": "#2D2D2D",
      "tab.border": "#252526",
      "editorGroupHeader.tabsBackground": "#252526",
      "button.background": "#0E639C",
      "button.foreground": "#FFFFFF",
      "button.secondaryBackground": "#3A3D41",
      "input.background": "#3C3C3C",
      "list.hoverBackground": "#2A2D2E",
      "list.activeSelectionBackground": "#04395E",
      "textCodeBlock.background": "#0A0A0A66",
      "widget.border": "#454545",
      "terminal.background": "#1E1E1E",
      "terminal.foreground": "#CCCCCC",
      "testing.iconPassed": "#73C991",
    },
  },
  {
    id: "light-plus",
    name: "Light+",
    type: "light",
    tokenColors: LIGHT_TOKENS,
    colors: {
      ...ANSI_LIGHT,
      foreground: "#616161",
      descriptionForeground: "#717171",
      errorForeground: "#A1260D",
      focusBorder: "#0090F1",
      "editor.background": "#FFFFFF",
      "editor.foreground": "#000000",
      "editorWidget.background": "#F3F3F3",
      "sideBar.background": "#F3F3F3",
      "sideBar.border": "#E5E5E5",
      "activityBar.background": "#2C2C2C",
      "activityBar.foreground": "#FFFFFF",
      "activityBar.inactiveForeground": "#FFFFFF66",
      "activityBar.activeBorder": "#FFFFFF",
      "statusBar.background": "#007ACC",
      "statusBar.foreground": "#FFFFFF",
      "titleBar.activeBackground": "#DDDDDD",
      "titleBar.activeForeground": "#333333",
      "panel.background": "#FFFFFF",
      "panel.border": "#80808059",
      "tab.activeBackground": "#FFFFFF",
      "tab.inactiveBackground": "#ECECEC",
      "tab.border": "#F3F3F3",
      "editorGroupHeader.tabsBackground": "#F3F3F3",
      "button.background": "#007ACC",
      "button.foreground": "#FFFFFF",
      "button.secondaryBackground": "#5F6A79",
      "input.background": "#FFFFFF",
      "input.border": "#CECECE",
      "list.hoverBackground": "#E8E8E8",
      "list.activeSelectionBackground": "#CCE6FF",
      "textCodeBlock.background": "#DCDCDC66",
      "widget.border": "#C8C8C8",
      "terminal.background": "#FFFFFF",
      "terminal.foreground": "#333333",
      "testing.iconPassed": "#388A34",
    },
  },
  {
    id: "hc-black",
    name: "Dark High Contrast",
    type: "hc",
    tokenColors: [
      t("comment", "#7CA668"),
      t("string", "#CE9178"),
      t("constant.numeric", "#B5CEA8"),
      t("keyword, storage", "#569CD6"),
      t("keyword.control", "#C586C0"),
      t("entity.name.type, support.type", "#4EC9B0"),
      t("entity.name.function", "#DCDCAA"),
      t("variable", "#9CDCFE"),
    ],
    colors: {
      ...ANSI_DARK,
      foreground: "#FFFFFF",
      descriptionForeground: "#FFFFFF",
      errorForeground: "#F48771",
      focusBorder: "#F38518",
      contrastBorder: "#6FC3DF",
      "editor.background": "#000000",
      "editor.foreground": "#FFFFFF",
      "editorWidget.background": "#0C141F",
      "sideBar.background": "#000000",
      "sideBar.border": "#6FC3DF",
      "activityBar.background": "#000000",
      "activityBar.foreground": "#FFFFFF",
      "activityBar.activeBorder": "#F38518",
      "statusBar.background": "#000000",
      "statusBar.foreground": "#FFFFFF",
      "statusBar.border": "#6FC3DF",
      "titleBar.activeBackground": "#000000",
      "titleBar.activeForeground": "#FFFFFF",
      "titleBar.border": "#6FC3DF",
      "panel.background": "#000000",
      "panel.border": "#6FC3DF",
      "tab.activeBackground": "#000000",
      "tab.inactiveBackground": "#000000",
      "tab.border": "#6FC3DF",
      "tab.activeBorderTop": "#F38518",
      "editorGroupHeader.tabsBackground": "#000000",
      "button.background": "#000000",
      "button.foreground": "#FFFFFF",
      "button.border": "#6FC3DF",
      "input.background": "#000000",
      "input.border": "#6FC3DF",
      "list.hoverBackground": "#000000",
      "list.activeSelectionBackground": "#000000",
      "textCodeBlock.background": "#000000",
      "widget.border": "#6FC3DF",
      "terminal.background": "#000000",
      "terminal.foreground": "#FFFFFF",
      "testing.iconPassed": "#89D185",
    },
  },
  {
    id: "hc-light",
    name: "Light High Contrast",
    type: "hcLight",
    tokenColors: [
      t("comment", "#515151"),
      t("string", "#0F4A85"),
      t("constant.numeric", "#096D48"),
      t("keyword, storage", "#0F4A85"),
      t("keyword.control", "#B5200D"),
      t("entity.name.type, support.type", "#185E73"),
      t("entity.name.function", "#5E2CBC"),
      t("variable", "#001080"),
    ],
    colors: {
      ...ANSI_LIGHT,
      foreground: "#292929",
      descriptionForeground: "#292929",
      errorForeground: "#B5200D",
      focusBorder: "#006BBD",
      contrastBorder: "#0F4A85",
      "editor.background": "#FFFFFF",
      "editor.foreground": "#292929",
      "editorWidget.background": "#FFFFFF",
      "sideBar.background": "#FFFFFF",
      "sideBar.border": "#0F4A85",
      "activityBar.background": "#FFFFFF",
      "activityBar.foreground": "#292929",
      "activityBar.activeBorder": "#B5200D",
      "statusBar.background": "#FFFFFF",
      "statusBar.foreground": "#292929",
      "statusBar.border": "#0F4A85",
      "titleBar.activeBackground": "#FFFFFF",
      "titleBar.activeForeground": "#292929",
      "titleBar.border": "#0F4A85",
      "panel.background": "#FFFFFF",
      "panel.border": "#0F4A85",
      "tab.activeBackground": "#FFFFFF",
      "tab.inactiveBackground": "#FFFFFF",
      "tab.border": "#0F4A85",
      "tab.activeBorderTop": "#B5200D",
      "editorGroupHeader.tabsBackground": "#FFFFFF",
      "button.background": "#0F4A85",
      "button.foreground": "#FFFFFF",
      "input.background": "#FFFFFF",
      "input.border": "#0F4A85",
      "list.hoverBackground": "#FFFFFF",
      "list.activeSelectionBackground": "#FFFFFF",
      "textCodeBlock.background": "#F2F2F2",
      "widget.border": "#0F4A85",
      "terminal.background": "#FFFFFF",
      "terminal.foreground": "#292929",
      "testing.iconPassed": "#007100",
    },
  },
  {
    id: "monokai",
    name: "Monokai",
    type: "dark",
    tokenColors: [
      t("comment", "#88846F"),
      t("string", "#E6DB74"),
      t("constant.numeric, constant.language", "#AE81FF"),
      t("keyword, storage", "#F92672"),
      t("storage.type", "#66D9EF", "italic"),
      t("entity.name.type, entity.name.class, support.type", "#A6E22E"),
      t("entity.name.function, support.function", "#A6E22E"),
      t("variable.parameter", "#FD971F", "italic"),
      t("entity.name.tag", "#F92672"),
      t("entity.other.attribute-name", "#A6E22E"),
    ],
    colors: {
      ...ANSI_DARK,
      foreground: "#F8F8F2",
      descriptionForeground: "#CCCCCC",
      errorForeground: "#F92672",
      focusBorder: "#99947C",
      "editor.background": "#272822",
      "editor.foreground": "#F8F8F2",
      "editor.lineHighlightBackground": "#3E3D32",
      "editorWidget.background": "#1E1F1C",
      "sideBar.background": "#1E1F1C",
      "sideBar.border": "#414339",
      "activityBar.background": "#272822",
      "activityBar.foreground": "#F8F8F2",
      "activityBar.activeBorder": "#F92672",
      "statusBar.background": "#414339",
      "statusBar.foreground": "#F8F8F2",
      "titleBar.activeBackground": "#1E1F1C",
      "titleBar.activeForeground": "#F8F8F2",
      "panel.background": "#1E1F1C",
      "panel.border": "#414339",
      "tab.activeBackground": "#272822",
      "tab.inactiveBackground": "#34352F",
      "tab.border": "#1E1F1C",
      "editorGroupHeader.tabsBackground": "#1E1F1C",
      "button.background": "#75715E",
      "button.foreground": "#F8F8F2",
      "input.background": "#414339",
      "list.hoverBackground": "#3E3D32",
      "list.activeSelectionBackground": "#75715E",
      "textCodeBlock.background": "#3E3D32",
      "widget.border": "#414339",
      "terminal.background": "#1E1F1C",
      "terminal.foreground": "#F8F8F2",
      "testing.iconPassed": "#A6E22E",
    },
  },
  {
    id: "solarized-dark",
    name: "Solarized Dark",
    type: "dark",
    tokenColors: [
      t("comment", "#586E75", "italic"),
      t("string", "#2AA198"),
      t("constant.numeric, constant.language", "#D33682"),
      t("keyword, storage", "#859900"),
      t("storage.type", "#93A1A1"),
      t("entity.name.type, entity.name.class, support.type", "#CB4B16"),
      t("entity.name.function, support.function", "#268BD2"),
      t("variable", "#268BD2"),
      t("entity.name.tag", "#268BD2"),
      t("entity.other.attribute-name", "#93A1A1"),
    ],
    colors: {
      ...ANSI_DARK,
      foreground: "#839496",
      descriptionForeground: "#93A1A1",
      errorForeground: "#FFEEEE",
      focusBorder: "#2AA19899",
      "editor.background": "#002B36",
      "editor.foreground": "#839496",
      "editor.lineHighlightBackground": "#073642",
      "editorWidget.background": "#00212B",
      "sideBar.background": "#00212B",
      "sideBar.border": "#003847",
      "activityBar.background": "#003847",
      "activityBar.foreground": "#93A1A1",
      "activityBar.activeBorder": "#2AA198",
      "statusBar.background": "#00212B",
      "statusBar.foreground": "#93A1A1",
      "titleBar.activeBackground": "#002C39",
      "titleBar.activeForeground": "#93A1A1",
      "panel.background": "#00212B",
      "panel.border": "#2B2B4A",
      "tab.activeBackground": "#002B36",
      "tab.inactiveBackground": "#004052",
      "tab.border": "#003847",
      "editorGroupHeader.tabsBackground": "#004052",
      "button.background": "#2AA19899",
      "button.foreground": "#FFFFFF",
      "input.background": "#003847",
      "list.hoverBackground": "#004454AA",
      "list.activeSelectionBackground": "#005A6F",
      "textCodeBlock.background": "#073642",
      "widget.border": "#003847",
      "terminal.background": "#002B36",
      "terminal.foreground": "#839496",
      "testing.iconPassed": "#859900",
    },
  },
  {
    id: "solarized-light",
    name: "Solarized Light",
    type: "light",
    tokenColors: [
      t("comment", "#93A1A1", "italic"),
      t("string", "#2AA198"),
      t("constant.numeric, constant.language", "#D33682"),
      t("keyword, storage", "#859900"),
      t("storage.type", "#073642"),
      t("entity.name.type, entity.name.class, support.type", "#CB4B16"),
      t("entity.name.function, support.function", "#268BD2"),
      t("variable", "#268BD2"),
      t("entity.name.tag", "#268BD2"),
      t("entity.other.attribute-name", "#93A1A1"),
    ],
    colors: {
      ...ANSI_LIGHT,
      foreground: "#657B83",
      descriptionForeground: "#586E75",
      errorForeground: "#DC322F",
      focusBorder: "#B49471",
      "editor.background": "#FDF6E3",
      "editor.foreground": "#657B83",
      "editor.lineHighlightBackground": "#EEE8D5",
      "editorWidget.background": "#EEE8D5",
      "sideBar.background": "#EEE8D5",
      "sideBar.border": "#DDD6C1",
      "activityBar.background": "#DDD6C1",
      "activityBar.foreground": "#584C27",
      "activityBar.activeBorder": "#B58900",
      "statusBar.background": "#EEE8D5",
      "statusBar.foreground": "#586E75",
      "titleBar.activeBackground": "#EEE8D5",
      "titleBar.activeForeground": "#586E75",
      "panel.background": "#EEE8D5",
      "panel.border": "#DDD6C1",
      "tab.activeBackground": "#FDF6E3",
      "tab.inactiveBackground": "#D3CBB7",
      "tab.border": "#DDD6C1",
      "editorGroupHeader.tabsBackground": "#D9D2C2",
      "button.background": "#AC9D57",
      "button.foreground": "#FFFFFF",
      "input.background": "#DDD6C1",
      "list.hoverBackground": "#DFCA8844",
      "list.activeSelectionBackground": "#DFCA88",
      "textCodeBlock.background": "#EEE8D5",
      "widget.border": "#DDD6C1",
      "terminal.background": "#FDF6E3",
      "terminal.foreground": "#657B83",
      "testing.iconPassed": "#859900",
    },
  },
];

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
