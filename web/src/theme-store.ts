// The theme in force: the one chosen (`workbench.colorTheme`, from Code Arena), or the one being previewed in the
// picker, or the system's light or dark. Applied to the page's colours at once; Coder mode's editor and terminals
// follow it. The last one applied is kept in this browser, so a reload paints in it before Code Arena answers.
import { createMemo, createRoot, createSignal } from "solid-js";
import { BUILT_IN, type Theme, chosen, cssVars } from "./themes";

const CACHE = "code-arena.theme";

const media = window.matchMedia("(prefers-color-scheme: dark)");
const [systemDark, setSystemDark] = createSignal(media.matches);
media.addEventListener("change", () => setSystemDark(media.matches));

/** Your own themes, as Code Arena read them from the themes/ folders. */
export const [custom, setCustom] = createSignal<Theme[]>([]);
/** The choice kept in the settings ("" follows the system). */
export const [choice, setChoice] = createSignal<string | undefined>(undefined);
/** A theme being looked at in the picker, not kept yet. */
export const [preview, setPreview] = createSignal<string | undefined>(undefined);
export const [pickerOpen, setPickerOpen] = createSignal(false);

export const allThemes = () => [...BUILT_IN, ...custom()];

function cached(): Theme | undefined {
  try {
    const t = localStorage.getItem(CACHE);
    return t ? (JSON.parse(t) as Theme) : undefined;
  } catch {
    return undefined;
  }
}

export const current = createRoot(() =>
  createMemo<Theme>(() => {
    const c = preview() ?? choice();
    // Before Code Arena said which: the one this browser last showed.
    if (c === undefined) return cached() ?? chosen(allThemes(), "", systemDark());
    return chosen(allThemes(), c, systemDark());
  }),
);

/** The theme's colours on the page. */
export function apply(th: Theme) {
  const root = document.documentElement;
  for (const [k, v] of Object.entries(cssVars(th))) root.style.setProperty(k, v);
  const light = th.type === "light" || th.type === "hcLight";
  root.style.colorScheme = light ? "light" : "dark";
  root.dataset.themeKind = th.type;
  if (preview() === undefined) {
    try {
      localStorage.setItem(CACHE, JSON.stringify(th));
    } catch {
      // Not kept: a reload paints in the default until Code Arena answers.
    }
  }
}
