// Coder mode: the project's files in a tree, files in tabs of VS Code's own editor (Monaco), terminals in a panel,
// and the agent beside them. Loaded only when Coder mode is first opened.
import "@xterm/xterm/css/xterm.css";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import * as monaco from "monaco-editor/editor/editor.api";
import "./monaco-features";
import "monaco-editor/basic-languages/monaco.contribution";
import EditorWorker from "monaco-editor/editor/editor.worker?worker";
import { For, type JSX, Match, Show, Switch, createEffect, createRoot, createSignal, on, onCleanup, onMount } from "solid-js";
import { createStore, produce, reconcile, unwrap } from "solid-js/store";
import { type Layout as Workbench, type ViewId, VIEWS, type Where, defaults, move, normalize, reveal, toggle, toggleView } from "./layout";
import { setEditorStatus } from "./editor-status";
import { current, setPickerOpen } from "./theme-store";
import { base, monacoColors, monacoRules, terminalColors } from "./themes";
import { type Symbol as OutlineSymbol, outline } from "./outline";
import { listen, send, session, setDetail } from "./stream";

(self as any).MonacoEnvironment = { getWorker: () => new EditorWorker() };

const SYMBOL_KIND: Record<OutlineSymbol["kind"], monaco.languages.SymbolKind> = {
  function: monaco.languages.SymbolKind.Function,
  method: monaco.languages.SymbolKind.Method,
  class: monaco.languages.SymbolKind.Class,
  struct: monaco.languages.SymbolKind.Struct,
  enum: monaco.languages.SymbolKind.Enum,
  interface: monaco.languages.SymbolKind.Interface,
  type: monaco.languages.SymbolKind.TypeParameter,
  module: monaco.languages.SymbolKind.Module,
  constant: monaco.languages.SymbolKind.Constant,
  heading: monaco.languages.SymbolKind.String,
};

// The outline as Monaco's symbols: Go to Symbol (Ctrl+Shift+O) and the breadcrumbs read them.
for (const lang of ["rust", "typescript", "javascript", "python", "go", "java", "csharp", "cpp", "c", "kotlin", "swift", "markdown"]) {
  monaco.languages.registerDocumentSymbolProvider(lang, {
    provideDocumentSymbols(model) {
      return outline(model.getValue(), lang).map((s) => {
        const range = new monaco.Range(s.line, 1, s.line, model.getLineMaxColumn(s.line));
        return { name: s.name, detail: "", kind: SYMBOL_KIND[s.kind], tags: [], range, selectionRange: range };
      });
    },
  });
}

const [symbols, setSymbols] = createSignal<OutlineSymbol[]>([]);
const [cursorLine, setCursorLine] = createSignal(1);

type Entry = { name: string; dir: boolean; link: boolean };
type Tab = { path: string; dirty: boolean; error?: string; diff?: boolean; onDisk?: "changed" | "gone" };
type Hit = { path: string; line: number; text: string };
type Change = { path: string; before: string | null; after: string | null };

const [tree, setTree] = createStore<Record<string, Entry[] | { error: string }>>({});
const [open, setOpen] = createStore<Record<string, boolean>>({ "": true });
const [tabs, setTabs] = createStore<Tab[]>([]);
const [active, setActive] = createSignal("");
/** Where each view is, which is shown, and where the side bar and the panel are (VS Code's layout). */
const [workbench, setWorkbenchStore] = createStore<Workbench>(defaults());
function setWorkbench(l: Workbench) {
  setWorkbenchStore(reconcile(l));
  keepLayout();
}
const plain = () => structuredClone(unwrap(workbench)) as Workbench;
const setView = (v: ViewId) => setWorkbench(reveal(plain(), v));
/** Whether a view is on screen: its container shown, and it the one shown there. */
const isShown = (v: ViewId) => (["primary", "secondary", "panel"] as Where[]).some((w) => workbench.shown[w] && workbench.active[w] === v);
const [found, setFound] = createStore<{ query: string; hits: Hit[]; files: number; more: boolean; error?: string; busy: boolean }>(
  { query: "", hits: [], files: 0, more: false, busy: false },
);
const [changes, setChanges] = createStore<Change[]>([]);
const [quick, setQuick] = createStore<{ open: boolean; paths: string[]; text: string; at: number }>({
  open: false,
  paths: [],
  text: "",
  at: 0,
});
/** When each open file was last read or saved, as the disk said: compared when the window comes back. */
const modifiedAt = new Map<string, number>();
/** Where a file opened from a search should show. */
const goTo = new Map<string, number>();
/** Unsaved changes kept aside, waiting for their file's model. */
const pendingBackup = new Map<string, string>();
/** Where the cursor was in each file. */
const cursors = new Map<string, { lineNumber: number; column: number }>();
/** Each model's timer that keeps its unsaved changes aside a second after typing stops. */
const backupTimers = new Map<string, number>();

type Layout = {
  tabs: string[];
  active: string;
  folders: string[];
  cursors: Record<string, { lineNumber: number; column: number }>;
  sizes: Sizes;
  workbench?: Workbench;
};
/** The primary side bar's width, the secondary's, the panel's height (at the bottom) and width (at a side). */
type Sizes = { side: number; agent: number; term: number; panelWidth: number };
export const [sizes, setSizes] = createStore<Sizes>({ side: 260, agent: 400, term: 260, panelWidth: 440 });
export const panelOpen = () => workbench.shown.panel;
export const setPanelOpen = (v: boolean) => {
  if (v !== workbench.shown.panel) setWorkbench(toggle(plain(), "panel"));
};

const layoutKey = () => `code-arena.coder:${session.folder}`;

function loadLayout(): Layout | undefined {
  try {
    const t = localStorage.getItem(layoutKey());
    return t ? (JSON.parse(t) as Layout) : undefined;
  } catch {
    return undefined;
  }
}

let saveTimer = 0;
/** The tabs, cursors, folders and sizes kept for the next visit (a little after each change). */
function keepLayout() {
  clearTimeout(saveTimer);
  saveTimer = window.setTimeout(() => {
    const layout: Layout = {
      tabs: tabs.filter((t) => !t.diff).map((t) => t.path),
      active: active().startsWith("Δ ") ? "" : active(),
      folders: Object.entries(open).filter(([p, o]) => o && p).map(([p]) => p),
      cursors: Object.fromEntries(cursors),
      sizes: { ...sizes },
      workbench: plain(),
    };
    try {
      localStorage.setItem(layoutKey(), JSON.stringify(layout));
    } catch {
      // Not kept: the next visit starts as a new one.
    }
  }, 400);
}

/** The diff tabs' models: before and now. */
const diffs = new Map<string, { before: monaco.editor.ITextModel; after: monaco.editor.ITextModel }>();

/** Each open file's model, and the version it was saved (or read) at. */
const models = new Map<string, { model: monaco.editor.ITextModel; saved: number }>();

/** The theme in force as Monaco's: defined once per theme (its colours and its token colours), then set. */
const defined = new Set<string>();
function monacoTheme(): string {
  const th = current();
  const id = `ca-${th.id.replace(/[^a-z0-9-]/gi, "-")}`;
  if (!defined.has(id)) {
    monaco.editor.defineTheme(id, { base: base(th), inherit: true, rules: monacoRules(th), colors: monacoColors(th) });
    defined.add(id);
  }
  return id;
}
createRoot(() => createEffect(() => monaco.editor.setTheme(monacoTheme())));

/** The text of a file that changed on disk, put in its model by its changed lines only: the cursor stays. */
function replaceChanged(model: monaco.editor.ITextModel, text: string) {
  const old = model.getLinesContent();
  const next = text.split(/\r?\n/);
  let start = 0;
  while (start < old.length && start < next.length && old[start] === next[start]) start++;
  let endOld = old.length - 1;
  let endNew = next.length - 1;
  while (endOld >= start && endNew >= start && old[endOld] === next[endNew]) {
    endOld--;
    endNew--;
  }
  if (start > endOld && start > endNew) return;
  const range =
    start > endOld
      ? new monaco.Range(start + 1, 1, start + 1, 1)
      : new monaco.Range(start + 1, 1, endOld + 1, model.getLineMaxColumn(endOld + 1));
  let inserted = next.slice(start, endNew + 1).join(model.getEOL());
  if (start > endOld) inserted += model.getEOL();
  model.pushEditOperations([], [{ range, text: inserted }], () => null);
}

function setDirty(path: string, dirty: boolean) {
  setTabs((t) => t.path === path, "dirty", dirty);
}

function openFile(path: string, line?: number) {
  if (!tabs.some((t) => t.path === path)) setTabs(produce((l) => void l.push({ path, dirty: false })));
  setActive(path);
  if (line) goTo.set(path, line);
  if (!models.has(path)) send({ type: "open", path });
  else showModel(path);
}

/** A file's language as Monaco names it, by its name or extension ("plaintext" when none is known). */
function languageOf(path: string): string {
  const name = path.split("/").pop() ?? "";
  const lower = name.toLowerCase();
  for (const l of monaco.languages.getLanguages()) {
    if (l.filenames?.some((f) => f.toLowerCase() === lower)) return l.id;
    if (l.extensions?.some((e) => lower.endsWith(e.toLowerCase()))) return l.id;
  }
  return "plaintext";
}

/** A file the agent changed, before and now, in a tab of its own. */
function openDiff(c: Change) {
  const key = `Δ ${c.path}`;
  diffs.get(key)?.before.dispose();
  diffs.get(key)?.after.dispose();
  const uri = (side: string) => monaco.Uri.from({ scheme: "diff", path: `/${side}/${c.path}` });
  const language = languageOf(c.path);
  diffs.set(key, {
    before: monaco.editor.createModel(c.before ?? "", language, uri("before")),
    after: monaco.editor.createModel(c.after ?? "", language, uri("after")),
  });
  if (!tabs.some((t) => t.path === key)) setTabs(produce((l) => void l.push({ path: key, dirty: false, diff: true })));
  setActive(key);
  showModel(key);
}

/** Back in the window: the open files are asked when they last changed, and the open folders listed again. */
function checkDisk() {
  const paths = tabs.filter((t) => !t.diff).map((t) => t.path);
  if (paths.length) send({ type: "stat", paths });
  for (const [path, isOpen] of Object.entries(open)) if (isOpen) send({ type: "ls", path });
  setQuick("paths", []);
}

function closeTab(path: string) {
  const t = tabs.find((x) => x.path === path);
  if (t?.diff) {
    diffs.get(path)?.before.dispose();
    diffs.get(path)?.after.dispose();
    diffs.delete(path);
  }
  if (t?.dirty && !confirm(`${path} has changes not saved. Close it anyway?`)) return;
  if (t?.dirty) send({ type: "backup_drop", path });
  const i = tabs.findIndex((x) => x.path === path);
  setTabs(produce((l) => void l.splice(i, 1)));
  models.get(path)?.model.dispose();
  models.delete(path);
  if (active() === path) setActive(tabs[Math.max(0, i - 1)]?.path ?? "");
}

function save(path: string) {
  const m = models.get(path);
  if (m) send({ type: "save", path, text: m.model.getValue() });
}

listen((m) => {
  switch (m.type) {
    case "ls":
      setTree(m.path, m.error ? { error: m.error } : m.entries);
      break;
    case "file": {
      if (m.error) {
        setTabs((t) => t.path === m.path, "error", m.error);
        break;
      }
      modifiedAt.set(m.path, m.modified);
      setTabs((t) => t.path === m.path, "onDisk", undefined);
      const had = models.get(m.path);
      if (had) {
        // Read again (the agent changed it): taken in only when the tab has no changes of its own.
        if (had.model.getAlternativeVersionId() === had.saved) {
          replaceChanged(had.model, m.text);
          had.saved = had.model.getAlternativeVersionId();
          setDirty(m.path, false);
        }
        break;
      }
      if (!tabs.some((t) => t.path === m.path)) break;
      const model = monaco.editor.createModel(m.text, undefined, monaco.Uri.file(`/${m.path}`));
      const entry = { model, saved: model.getAlternativeVersionId() };
      models.set(m.path, entry);
      model.onDidChangeContent(() => {
        const dirty = model.getAlternativeVersionId() !== entry.saved;
        setDirty(m.path, dirty);
        clearTimeout(backupTimers.get(m.path));
        if (dirty) backupTimers.set(m.path, window.setTimeout(() => send({ type: "backup", path: m.path, text: model.getValue() }), 1000));
      });
      // Changes not saved when the IDE last closed: back in the tab, still to save.
      const kept = pendingBackup.get(m.path);
      if (kept !== undefined && kept !== m.text) model.setValue(kept);
      pendingBackup.delete(m.path);
      if (active() === m.path) showModel(m.path);
      break;
    }
    case "files":
      setQuick("paths", m.paths);
      break;
    case "backups":
      for (const b of m.items) {
        const had = models.get(b.path);
        if (had) {
          if (had.model.getValue() !== b.text) had.model.setValue(b.text);
        } else {
          pendingBackup.set(b.path, b.text);
          if (!tabs.some((t) => t.path === b.path)) setTabs(produce((l) => void l.push({ path: b.path, dirty: true })));
          send({ type: "open", path: b.path });
        }
      }
      break;
    case "search":
      if (m.query === found.query) setFound({ hits: m.hits ?? [], files: m.files ?? 0, more: !!m.more, error: m.error, busy: false });
      break;
    case "changes":
      setChanges(m.files);
      break;
    case "stats":
      for (const it of m.items) {
        const known = modifiedAt.get(it.path);
        const dirty = tabs.find((t) => t.path === it.path)?.dirty;
        if (it.gone) setTabs((t) => t.path === it.path, "onDisk", "gone");
        else if (known !== undefined && it.modified !== known) {
          if (dirty) setTabs((t) => t.path === it.path, "onDisk", "changed");
          else send({ type: "open", path: it.path });
        }
      }
      break;
    case "turn_end":
      if (isShown("changes")) send({ type: "changes" });
      break;
    case "saved": {
      const had = models.get(m.path);
      if (m.error) alert(`${m.path} was not saved: ${m.error}`);
      else if (had) {
        had.saved = had.model.getAlternativeVersionId();
        modifiedAt.set(m.path, m.modified);
        setDirty(m.path, false);
        setTabs((t) => t.path === m.path, "onDisk", undefined);
      }
      break;
    }
    case "tool_end":
      // The agent wrote a file that is open: read it again.
      if (m.ok && (m.name === "edit_file" || m.name === "write_file") && models.has(m.subject)) {
        send({ type: "open", path: m.subject });
      }
      break;
  }
});

let editor: monaco.editor.IStandaloneCodeEditor | undefined;
let differ: monaco.editor.IStandaloneDiffEditor | undefined;

function showModel(path: string) {
  const d = diffs.get(path);
  if (d) {
    differ?.setModel({ original: d.before, modified: d.after });
    return;
  }
  const m = models.get(path);
  if (editor && m) {
    editor.setModel(m.model);
    const line = goTo.get(path);
    const cursor = cursors.get(path);
    if (line) {
      goTo.delete(path);
      editor.setPosition({ lineNumber: line, column: 1 });
      editor.revealLineInCenter(line);
    } else if (cursor) {
      editor.setPosition(cursor);
      editor.revealPositionInCenter(cursor);
    }
    editor.focus();
  }
}

function Folder(props: { path: string; depth: number }) {
  const entries = () => tree[props.path];
  onMount(() => {
    if (!tree[props.path]) send({ type: "ls", path: props.path });
  });
  return (
    <Show when={entries()} fallback={<div class="tree-row muted" style={{ "padding-left": `${props.depth * 12 + 8}px` }}>…</div>}>
      {(e) => (
        <Show
          when={Array.isArray(e())}
          fallback={<div class="tree-row error-text" style={{ "padding-left": `${props.depth * 12 + 8}px` }}>{(e() as { error: string }).error}</div>}
        >
          <For each={e() as Entry[]}>
            {(entry) => {
              const path = props.path ? `${props.path}/${entry.name}` : entry.name;
              return (
                <>
                  <button
                    class="tree-row"
                    classList={{ active: active() === path, dir: entry.dir }}
                    style={{ "padding-left": `${props.depth * 12 + 8}px` }}
                    title={path}
                    aria-expanded={entry.dir ? !!open[path] : undefined}
                    onClick={() => (entry.dir ? setOpen(path, !open[path]) : openFile(path))}
                  >
                    <span class="twisty">{entry.dir ? (open[path] ? "▾" : "▸") : ""}</span>
                    <span class="name">{entry.name}</span>
                    <Show when={entry.link}>
                      <span class="muted"> ↪</span>
                    </Show>
                  </button>
                  <Show when={entry.dir && open[path]}>
                    <Folder path={path} depth={props.depth + 1} />
                  </Show>
                </>
              );
            }}
          </For>
        </Show>
      )}
    </Show>
  );
}

function Editor() {
  let host!: HTMLDivElement;
  let diffHost!: HTMLDivElement;
  const current = () => tabs.find((t) => t.path === active());
  onMount(() => {
    differ = monaco.editor.createDiffEditor(diffHost, {
      automaticLayout: true,
      readOnly: true,
      originalEditable: false,
      theme: monacoTheme(),
      fontSize: 13,
      renderSideBySide: true,
    });
    editor = monaco.editor.create(host, {
      model: null,
      automaticLayout: true,
      theme: monacoTheme(),
      fontSize: 13,
      minimap: { enabled: true },
      scrollBeyondLastLine: false,
    });
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => {
      if (active()) save(active());
    });
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyP, () => openQuick());
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyMod.Shift | monaco.KeyCode.KeyP, () => setPalette({ open: true, text: "", at: 0 }));
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyMod.Shift | monaco.KeyCode.KeyF, () => setView("search"));
    const report = () => {
      const model = editor?.getModel();
      if (!model) {
        setEditorStatus("shown", false);
        return;
      }
      const o = model.getOptions();
      const pos = editor?.getPosition();
      setEditorStatus({
        shown: true,
        line: pos?.lineNumber ?? 1,
        column: pos?.column ?? 1,
        language: model.getLanguageId(),
        indent: o.insertSpaces ? `Spaces: ${o.tabSize}` : `Tab size: ${o.tabSize}`,
        eol: model.getEOL() === "\r\n" ? "CRLF" : "LF",
      });
      setCursorLine(pos?.lineNumber ?? 1);
    };
    let outlineTimer = 0;
    const reoutline = () => {
      clearTimeout(outlineTimer);
      outlineTimer = window.setTimeout(() => {
        const model = editor?.getModel();
        setSymbols(model ? outline(model.getValue(), model.getLanguageId()) : []);
      }, 300);
    };
    editor.onDidChangeModel(() => {
      report();
      reoutline();
    });
    editor.onDidChangeModelContent(reoutline);
    editor.onDidChangeModelOptions(report);
    editor.onDidChangeCursorPosition(report);
    onCleanup(() => setEditorStatus("shown", false));
    editor.onDidChangeCursorPosition((e) => {
      const path = active();
      if (path && !path.startsWith("Δ ")) {
        cursors.set(path, { lineNumber: e.position.lineNumber, column: e.position.column });
        keepLayout();
      }
    });
    if (active()) showModel(active());
    onCleanup(() => {
      editor?.dispose();
      editor = undefined;
      differ?.dispose();
      differ = undefined;
    });
  });
  return (
    <div class="editor-area">
      <div class="tabs" role="tablist">
        <For each={tabs}>
          {(t) => (
            <div class="tab" classList={{ active: active() === t.path }} role="tab" aria-selected={active() === t.path}>
              <button
                class="tab-name"
                title={t.path}
                onClick={() => {
                  setActive(t.path);
                  showModel(t.path);
                }}
              >
                {t.path.split("/").pop()}
                {t.diff ? " (changes)" : ""}
                <Show when={t.dirty}>
                  <span class="dirty" aria-label="changed"> ●</span>
                </Show>
              </button>
              <button class="tab-close" aria-label={`Close ${t.path}`} onClick={() => closeTab(t.path)}>
                ×
              </button>
            </div>
          )}
        </For>
      </div>
      <Show when={current()?.error}>
        {(e) => <div class="editor-error">{active()}: {e()}</div>}
      </Show>
      <Show when={current()?.onDisk}>
        {(why) => (
          <div class="disk-bar" role="status">
            <Show
              when={why() === "changed"}
              fallback={<span>{active()} is gone from the disk: saving makes it again.</span>}
            >
              <span>{active()} changed on disk, and has changes of yours.</span>
              <button
                class="small"
                onClick={() => {
                  const had = models.get(active());
                  if (had) had.saved = had.model.getAlternativeVersionId();
                  setDirty(active(), false);
                  send({ type: "open", path: active() });
                }}
              >
                Take the disk's
              </button>
              <button class="quiet small" onClick={() => setTabs((t) => t.path === active(), "onDisk", undefined)}>
                Keep mine
              </button>
            </Show>
          </div>
        )}
      </Show>
      <Show when={!tabs.length}>
        <div class="editor-empty muted">Open a file from the tree, or Ctrl+P to find one. Ctrl+S saves it.</div>
      </Show>
      <div class="monaco" ref={host} classList={{ hidden: !tabs.length || !!current()?.error || !!current()?.diff }} />
      <div class="monaco" ref={diffHost} classList={{ hidden: !current()?.diff }} />
    </div>
  );
}

let nextTerm = 1;
/** What a terminal taken up after a reload showed: drawn again before what comes next. */
const screens = new Map<number, string>();

function Terminals() {
  const [ids, setIds] = createSignal<number[]>([]);
  const [shown, setShown] = createSignal(0);
  const add = () => {
    const id = nextTerm++;
    setIds([...ids(), id]);
    setShown(id);
  };
  // Terminals still running (the page was reloaded) are taken up with their screens; else one is opened, once
  // Code Arena says it has terminals.
  let asked = false;
  createEffect(() => {
    if (session.connected && session.terminals && !asked) {
      asked = true;
      send({ type: "terms" });
    }
  });
  const stop = listen((m) => {
    if (m.type !== "terms") return;
    stop();
    const running: { id: number; screen: string }[] = m.terms ?? [];
    for (const t of running) screens.set(t.id, t.screen);
    if (running.length) {
      nextTerm = Math.max(nextTerm, ...running.map((t) => t.id + 1));
      setIds(running.map((t) => t.id));
      setShown(running[running.length - 1].id);
    } else add();
  });
  onCleanup(stop);
  return (
    <div class="terminals">
      <div class="term-tabs">
        <For each={ids()}>
          {(id) => (
            <span class="term-tab" classList={{ active: shown() === id }}>
              <button class="term-name" onClick={() => setShown(id)}>
                Terminal {id}
              </button>
              <button
                class="term-close"
                aria-label={`Close terminal ${id}`}
                onClick={() => {
                  send({ type: "term_close", id });
                  const rest = ids().filter((x) => x !== id);
                  setIds(rest);
                  if (shown() === id) setShown(rest[rest.length - 1] ?? 0);
                }}
              >
                ×
              </button>
            </span>
          )}
        </For>
        <Show when={session.terminals} fallback={<span class="muted">Terminals are not on Windows yet.</span>}>
          <button class="quiet small" aria-label="New terminal" onClick={add}>
            +
          </button>
        </Show>
      </div>
      <For each={ids()}>
        {(id) => (
          <Term
            id={id}
            visible={shown() === id}
            onExit={() => {
              const rest = ids().filter((x) => x !== id);
              setIds(rest);
              if (shown() === id) setShown(rest[rest.length - 1] ?? 0);
            }}
          />
        )}
      </For>
    </div>
  );
}

function Term(props: { id: number; visible: boolean; onExit: () => void }) {
  let host!: HTMLDivElement;
  onMount(() => {
    const css = getComputedStyle(document.documentElement);
    const term = new Terminal({
      fontFamily: css.getPropertyValue("--mono"),
      fontSize: 13,
      cursorBlink: true,
      theme: terminalColors(current()),
    });
    createEffect(() => {
      term.options.theme = terminalColors(current());
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    fit.fit();
    const kept = screens.get(props.id);
    if (kept !== undefined) {
      screens.delete(props.id);
      term.write(kept);
      send({ type: "term_resize", id: props.id, cols: term.cols, rows: term.rows });
    } else {
      send({ type: "term_open", id: props.id, cols: term.cols, rows: term.rows });
    }
    term.onData((data) => send({ type: "term_in", id: props.id, data }));
    term.onResize(({ cols, rows }) => send({ type: "term_resize", id: props.id, cols, rows }));
    const stop = listen((m) => {
      if (m.id !== props.id) return;
      if (m.type === "term_out") term.write(m.data);
      if (m.type === "term_exit") {
        term.write(`\r\n[${m.error ?? `ended${m.code != null ? ` (${m.code})` : ""}`}]\r\n`);
        setTimeout(props.onExit, 1500);
      }
    });
    const ro = new ResizeObserver(() => {
      if (host.offsetParent) fit.fit();
    });
    ro.observe(host);
    // Leaving Coder mode (or the page) leaves the terminal running: it is taken up again on return.
    onCleanup(() => {
      stop();
      ro.disconnect();
      term.dispose();
    });
  });
  return <div class="term" ref={host} classList={{ hidden: !props.visible }} />;
}

function SearchView() {
  let input!: HTMLInputElement;
  const [regex, setRegex] = createSignal(false);
  const [kase, setKase] = createSignal(false);
  const run = () => {
    const q = input.value;
    setFound({ query: q, hits: [], files: 0, more: false, error: undefined, busy: !!q });
    if (q) send({ type: "search", query: q, regex: regex(), case: kase() });
  };
  const byFile = () => {
    const out: { path: string; hits: Hit[] }[] = [];
    for (const h of found.hits) {
      const last = out[out.length - 1];
      if (last?.path === h.path) last.hits.push(h);
      else out.push({ path: h.path, hits: [h] });
    }
    return out;
  };
  onMount(() => input.focus());
  return (
    <div class="search-view">
      <form
        class="search-form"
        onSubmit={(e) => {
          e.preventDefault();
          run();
        }}
      >
        <input ref={input} value={found.query} placeholder="Search the files" aria-label="Search the files" />
        <button type="button" class="toggle" classList={{ on: kase() }} title="Match case" aria-pressed={kase()} onClick={() => setKase(!kase())}>
          Aa
        </button>
        <button type="button" class="toggle" classList={{ on: regex() }} title="Regular expression" aria-pressed={regex()} onClick={() => setRegex(!regex())}>
          .*
        </button>
      </form>
      <Show when={found.busy}>
        <div class="muted small-pad">Searching…</div>
      </Show>
      <Show when={found.error}>{(e) => <div class="error-text small-pad">{e()}</div>}</Show>
      <Show when={!found.busy && found.query && !found.error}>
        <div class="muted small-pad">
          {found.hits.length} result{found.hits.length === 1 ? "" : "s"} in {found.files} file{found.files === 1 ? "" : "s"}
          {found.more ? " (the first ones)" : ""}
        </div>
      </Show>
      <For each={byFile()}>
        {(f) => (
          <div class="hit-file">
            <div class="hit-path" title={f.path}>{f.path}</div>
            <For each={f.hits}>
              {(h) => (
                <button class="hit" onClick={() => openFile(h.path, h.line)}>
                  <span class="hit-line">{h.line}</span>
                  <span class="hit-text">{h.text.trim()}</span>
                </button>
              )}
            </For>
          </div>
        )}
      </For>
    </div>
  );
}

function OutlineView() {
  const current = () => {
    let at = -1;
    symbols().forEach((s, i) => {
      if (s.line <= cursorLine()) at = i;
    });
    return at;
  };
  return (
    <div class="outline-view">
      <Show when={symbols().length} fallback={<div class="muted small-pad">No outline for this file.</div>}>
        <For each={symbols()}>
          {(s, i) => (
            <button
              class="tree-row"
              classList={{ active: i() === current() }}
              style={{ "padding-left": `${s.depth * 12 + 8}px` }}
              onClick={() => {
                editor?.setPosition({ lineNumber: s.line, column: 1 });
                editor?.revealLineInCenter(s.line);
                editor?.focus();
              }}
            >
              <span class={`sym ${s.kind}`}>{s.kind.slice(0, 2)}</span>
              <span class="name">{s.name}</span>
            </button>
          )}
        </For>
      </Show>
    </div>
  );
}

type Command = { id: string; label: string; keys?: string; run: () => void };

/** What the palette offers: Coder mode's, the editor's own, and the agent's. */
function commands(): Command[] {
  const ed = (id: string) => () => {
    editor?.focus();
    editor?.trigger("palette", id, null);
  };
  const list: Command[] = [
    { id: "file.save", label: "File: Save", keys: "Ctrl+S", run: () => active() && save(active()) },
    { id: "file.saveAll", label: "File: Save All", run: () => tabs.filter((t) => t.dirty && !t.diff).forEach((t) => save(t.path)) },
    { id: "file.close", label: "View: Close Editor", run: () => active() && closeTab(active()) },
    { id: "go.file", label: "Go to File…", keys: "Ctrl+P", run: openQuick },
    { id: "go.symbol", label: "Go to Symbol in Editor…", keys: "Ctrl+Shift+O", run: ed("editor.action.quickOutline") },
    { id: "go.line", label: "Go to Line…", keys: "Ctrl+G", run: ed("editor.action.gotoLine") },
    { id: "find", label: "Find", keys: "Ctrl+F", run: ed("actions.find") },
    { id: "replace", label: "Replace", keys: "Ctrl+H", run: ed("editor.action.startFindReplaceAction") },
    { id: "fold.all", label: "Fold All", run: ed("editor.foldAll") },
    { id: "unfold.all", label: "Unfold All", run: ed("editor.unfoldAll") },
    { id: "comment", label: "Toggle Line Comment", keys: "Ctrl+/", run: ed("editor.action.commentLine") },
    { id: "view.files", label: "View: Show Explorer", keys: "Ctrl+Shift+E", run: () => setView("files") },
    { id: "view.search", label: "View: Show Search", keys: "Ctrl+Shift+F", run: () => setView("search") },
    { id: "view.changes", label: "View: Show the Agent's Changes", run: () => setView("changes") },
    { id: "view.outline", label: "View: Show Outline", run: () => setView("outline") },
    { id: "view.agent", label: "View: Show Agent", keys: "Ctrl+Alt+I", run: () => setView("agent") },
    { id: "view.terminal", label: "View: Toggle Terminal", keys: "Ctrl+`", run: () => setWorkbench(toggleView(plain(), "terminal")) },
    { id: "layout.primary", label: "View: Toggle Primary Side Bar Visibility", keys: "Ctrl+B", run: () => setWorkbench(toggle(plain(), "primary")) },
    { id: "layout.secondary", label: "View: Toggle Secondary Side Bar Visibility", keys: "Ctrl+Alt+B", run: () => setWorkbench(toggle(plain(), "secondary")) },
    { id: "layout.panel", label: "View: Toggle Panel Visibility", keys: "Ctrl+J", run: () => setWorkbench(toggle(plain(), "panel")) },
    {
      id: "layout.side",
      label: "View: Toggle Primary Side Bar Position",
      run: () => setWorkbench({ ...plain(), side: workbench.side === "left" ? "right" : "left" }),
    },
    { id: "layout.panelBottom", label: "View: Move Panel to the Bottom", run: () => setWorkbench({ ...plain(), panelAt: "bottom" }) },
    { id: "layout.panelLeft", label: "View: Move Panel Left", run: () => setWorkbench({ ...plain(), panelAt: "left" }) },
    { id: "layout.panelRight", label: "View: Move Panel Right", run: () => setWorkbench({ ...plain(), panelAt: "right" }) },
    { id: "layout.reset", label: "View: Reset View Locations", run: () => setWorkbench(defaults()) },
    {
      id: "editor.minimap",
      label: "View: Toggle Minimap",
      run: () => editor?.updateOptions({ minimap: { enabled: !editor.getOption(monaco.editor.EditorOption.minimap).enabled } }),
    },
    {
      id: "editor.wrap",
      label: "View: Toggle Word Wrap",
      keys: "Alt+Z",
      run: () => editor?.updateOptions({ wordWrap: editor.getOption(monaco.editor.EditorOption.wordWrap) === "on" ? "off" : "on" }),
    },
    { id: "theme", label: "Preferences: Color Theme", keys: "Ctrl+K Ctrl+T", run: () => setPickerOpen(true) },
    { id: "agent.context", label: "Agent: Show the Context", run: () => (setDetail("open", true), send({ type: "context" })) },
    { id: "agent.compact", label: "Agent: Compact Now", run: () => send({ type: "compact" }) },
    { id: "agent.stop", label: "Agent: Stop the Turn", keys: "Esc", run: () => send({ type: "stop" }) },
  ];
  for (const m of ["ask", "auto-edit", "plan", "yolo"]) list.push({ id: `mode.${m}`, label: `Agent: Mode ${m}`, run: () => send({ type: "mode", mode: m }) });
  return list;
}

const RECENT_KEY = "code-arena.palette.recent";

function recent(): string[] {
  try {
    return JSON.parse(localStorage.getItem(RECENT_KEY) ?? "[]");
  } catch {
    return [];
  }
}

const [palette, setPalette] = createStore({ open: false, text: "", at: 0 });

function Palette() {
  let input!: HTMLInputElement;
  const list = () => {
    const all = commands();
    const r = recent();
    if (!palette.text.trim()) {
      return [...r.map((id) => all.find((c) => c.id === id)).filter((c): c is Command => !!c), ...all.filter((c) => !r.includes(c.id))];
    }
    return all
      .map((c) => ({ c, s: score(c.label, palette.text) }))
      .filter((x) => x.s >= 0)
      .sort((a, b) => b.s - a.s)
      .map((x) => x.c);
  };
  const run = (c?: Command) => {
    setPalette("open", false);
    if (!c) return;
    try {
      localStorage.setItem(RECENT_KEY, JSON.stringify([c.id, ...recent().filter((x) => x !== c.id)].slice(0, 8)));
    } catch {
      // Not kept: the palette lists by name only.
    }
    queueMicrotask(c.run);
  };
  onMount(() => input.focus());
  return (
    <div class="quick-back" onClick={() => setPalette("open", false)}>
      <div class="quick" role="dialog" aria-label="Commands" onClick={(e) => e.stopPropagation()}>
        <input
          ref={input}
          placeholder="Type a command"
          value={palette.text}
          aria-label="Command"
          onInput={(e) => setPalette({ text: e.currentTarget.value, at: 0 })}
          onKeyDown={(e) => {
            if (e.key === "Escape") setPalette("open", false);
            else if (e.key === "ArrowDown") setPalette("at", Math.min(palette.at + 1, list().length - 1));
            else if (e.key === "ArrowUp") setPalette("at", Math.max(palette.at - 1, 0));
            else if (e.key === "Enter") run(list()[palette.at]);
            else return;
            e.preventDefault();
          }}
        />
        <ul class="quick-list">
          <For each={list()}>
            {(c, i) => (
              <li>
                <button classList={{ at: i() === palette.at }} onClick={() => run(c)}>
                  <span class="name">{c.label}</span>
                  <Show when={c.keys}>
                    <span class="keys">{c.keys}</span>
                  </Show>
                </button>
              </li>
            )}
          </For>
        </ul>
      </div>
    </div>
  );
}

function ChangesView() {
  onMount(() => send({ type: "changes" }));
  return (
    <div class="changes-view">
      <Show when={changes.length} fallback={<div class="muted small-pad">The agent has changed no file in this run.</div>}>
        <For each={changes}>
          {(c) => (
            <button class="tree-row" onClick={() => openDiff(c)} title={c.path}>
              <span class="badge" classList={{ added: c.before === null, removed: c.after === null }}>
                {c.before === null ? "A" : c.after === null ? "D" : "M"}
              </span>
              <span class="name">{c.path}</span>
            </button>
          )}
        </For>
      </Show>
    </div>
  );
}

/** How well `text` matches `path`: its letters in order, closer and in the file's name better; -1 when not. */
export function score(path: string, text: string): number {
  const p = path.toLowerCase();
  const t = text.toLowerCase().replace(/\s+/g, "");
  if (!t) return 0;
  let at = 0;
  let total = 0;
  let last = -1;
  const nameAt = p.lastIndexOf("/") + 1;
  for (const c of t) {
    const i = p.indexOf(c, at);
    if (i < 0) return -1;
    total += i === last + 1 ? 3 : 1;
    if (i >= nameAt) total += 2;
    last = i;
    at = i + 1;
  }
  return total * 100 - p.length;
}

function openQuick() {
  setQuick({ open: true, text: "", at: 0 });
  if (!quick.paths.length) send({ type: "files" });
}

function QuickOpen() {
  let input!: HTMLInputElement;
  const list = () =>
    quick.paths
      .map((p) => ({ p, s: score(p, quick.text) }))
      .filter((x) => x.s >= 0)
      .sort((a, b) => b.s - a.s)
      .slice(0, 50)
      .map((x) => x.p);
  const choose = (p?: string) => {
    setQuick("open", false);
    if (p) openFile(p);
  };
  onMount(() => input.focus());
  return (
    <div class="quick-back" onClick={() => setQuick("open", false)}>
      <div class="quick" role="dialog" aria-label="Go to a file" onClick={(e) => e.stopPropagation()}>
        <input
          ref={input}
          placeholder={quick.paths.length ? `Go to a file (${quick.paths.length.toLocaleString()})` : "Listing the files…"}
          value={quick.text}
          aria-label="Go to a file"
          onInput={(e) => setQuick({ text: e.currentTarget.value, at: 0 })}
          onKeyDown={(e) => {
            if (e.key === "Escape") setQuick("open", false);
            else if (e.key === "ArrowDown") setQuick("at", Math.min(quick.at + 1, list().length - 1));
            else if (e.key === "ArrowUp") setQuick("at", Math.max(quick.at - 1, 0));
            else if (e.key === "Enter") choose(list()[quick.at]);
            else return;
            e.preventDefault();
          }}
        />
        <ul class="quick-list">
          <For each={list()}>
            {(p, i) => (
              <li>
                <button classList={{ at: i() === quick.at }} onClick={() => choose(p)}>
                  <span class="name">{p.split("/").pop()}</span>
                  <span class="muted"> {p}</span>
                </button>
              </li>
            )}
          </For>
        </ul>
      </div>
    </div>
  );
}

/** Coder mode, with the agent's thread and box (given by Agent mode) beside the editor. */
/** A divider dragged to size the pane beside it. */
function Splitter(props: {
  dir: "col" | "row";
  get: () => number;
  set: (n: number) => void;
  min: number;
  max: number;
  invert?: boolean;
  class?: string;
}) {
  const start = (e: PointerEvent) => {
    e.preventDefault();
    const from = props.dir === "col" ? e.clientX : e.clientY;
    const was = props.get();
    const move = (m: PointerEvent) => {
      const d = (props.dir === "col" ? m.clientX : m.clientY) - from;
      props.set(Math.max(props.min, Math.min(props.max, was + (props.invert ? -d : d))));
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      keepLayout();
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  return (
    <div
      class={`splitter ${props.dir} ${props.class ?? ""}`}
      role="separator"
      aria-orientation={props.dir === "col" ? "vertical" : "horizontal"}
      onPointerDown={start}
    />
  );
}

const DRAG_TYPE = "text/x-code-arena-view";
const [dragging, setDragging] = createSignal<ViewId | null>(null);
const [menu, setMenu] = createStore<{ view: ViewId | null; x: number; y: number }>({ view: null, x: 0, y: 0 });

const WHERE_NAME: Record<Where, string> = { primary: "Primary Side Bar", secondary: "Secondary Side Bar", panel: "Panel" };

/** What a view's icon or tab does: it drags to another container, and its menu moves it. */
function viewHandle(v: ViewId) {
  return {
    draggable: true,
    onDragStart: (e: DragEvent) => {
      e.dataTransfer?.setData(DRAG_TYPE, v);
      if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
      setDragging(v);
    },
    onDragEnd: () => setDragging(null),
    onContextMenu: (e: MouseEvent) => {
      e.preventDefault();
      setMenu({ view: v, x: e.clientX, y: e.clientY });
    },
  };
}

/** Where a view dropped goes: `to`, before the view at `index` when one is given. */
function dropTarget(to: Where, index?: () => number | undefined) {
  return {
    onDragOver: (e: DragEvent) => {
      if (!dragging()) return;
      e.preventDefault();
      e.stopPropagation();
      if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    },
    onDrop: (e: DragEvent) => {
      const v = dragging() ?? (e.dataTransfer?.getData(DRAG_TYPE) as ViewId | undefined);
      if (!v || !(v in VIEWS)) return;
      e.preventDefault();
      e.stopPropagation();
      setDragging(null);
      setWorkbench(move(plain(), v, to, index?.()));
    },
  };
}

function Icon(props: { name: string }) {
  return <span class={`codicon codicon-${props.name}`} aria-hidden="true" />;
}

function ViewBody(props: { v: ViewId; agent: () => JSX.Element }) {
  return (
    <Switch>
      <Match when={props.v === "files"}>
        <div class="tree">
          <Folder path="" depth={0} />
        </div>
      </Match>
      <Match when={props.v === "search"}>
        <SearchView />
      </Match>
      <Match when={props.v === "changes"}>
        <ChangesView />
      </Match>
      <Match when={props.v === "outline"}>
        <OutlineView />
      </Match>
      <Match when={props.v === "agent"}>
        <div class="agent-view">{props.agent()}</div>
      </Match>
      <Match when={props.v === "terminal"}>
        <Terminals />
      </Match>
    </Switch>
  );
}

/** The activity bar: the primary side bar's views as icons (VS Code's), each to show, hide or drag. */
function ActivityBar() {
  return (
    <nav class="activity" aria-label="Views" {...dropTarget("primary")}>
      <For each={workbench.primary}>
        {(v, i) => (
          <button
            class="activity-item"
            classList={{ on: workbench.shown.primary && workbench.active.primary === v }}
            title={`${VIEWS[v].title}${VIEWS[v].keys ? ` (${VIEWS[v].keys})` : ""}`}
            aria-label={VIEWS[v].title}
            aria-pressed={workbench.shown.primary && workbench.active.primary === v}
            onClick={() => setWorkbench(toggleView(plain(), v))}
            {...viewHandle(v)}
            {...dropTarget("primary", () => i())}
          >
            <Icon name={VIEWS[v].icon} />
          </button>
        )}
      </For>
    </nav>
  );
}

/** A container (side bar or panel): its header (the view's title, or its views as tabs), and its views. */
function Container(props: { where: Where; agent: () => JSX.Element }) {
  const views = () => workbench[props.where];
  const active = () => workbench.active[props.where];
  return (
    <section class={`container ${props.where}`} aria-label={WHERE_NAME[props.where]} {...dropTarget(props.where)}>
      <header class="container-head">
        <Show
          when={props.where !== "primary"}
          fallback={
            <span class="container-title" {...(active() ? viewHandle(active() as ViewId) : {})}>
              {active() ? VIEWS[active() as ViewId].title.toUpperCase() : ""}
              <Show when={active() === "files"}>
                <span class="muted">: {session.root || "Project"}</span>
              </Show>
            </span>
          }
        >
          <div class="container-tabs" role="tablist">
            <For each={views()}>
              {(v, i) => (
                <button
                  class="container-tab"
                  role="tab"
                  classList={{ on: active() === v }}
                  aria-selected={active() === v}
                  onClick={() => setWorkbench(reveal(plain(), v))}
                  {...viewHandle(v)}
                  {...dropTarget(props.where, () => i())}
                >
                  {VIEWS[v].title.toUpperCase()}
                </button>
              )}
            </For>
          </div>
        </Show>
        <span class="grow" />
        <button
          class="icon-button"
          title={`Hide ${WHERE_NAME[props.where]}`}
          aria-label={`Hide ${WHERE_NAME[props.where]}`}
          onClick={() => setWorkbench(toggle(plain(), props.where))}
        >
          <Icon name="close" />
        </button>
      </header>
      <For each={views()}>
        {(v) => (
          <div class="view-body" classList={{ hidden: active() !== v }}>
            <ViewBody v={v} agent={props.agent} />
          </div>
        )}
      </For>
    </section>
  );
}

/** While a view is dragged: the edges where a hidden or empty container would take it. */
function DropEdges() {
  const edge = (w: Where) => !workbench.shown[w] || !workbench[w].length;
  return (
    <Show when={dragging()}>
      <div class="drop-edges">
        <Show when={edge("primary")}>
          <div class={`drop-edge ${workbench.side === "left" ? "left" : "right"}`} {...dropTarget("primary")}>
            {WHERE_NAME.primary}
          </div>
        </Show>
        <Show when={edge("secondary")}>
          <div class={`drop-edge ${workbench.side === "left" ? "right" : "left"}`} {...dropTarget("secondary")}>
            {WHERE_NAME.secondary}
          </div>
        </Show>
        <Show when={edge("panel")}>
          <div class={`drop-edge ${workbench.panelAt}`} {...dropTarget("panel")}>
            {WHERE_NAME.panel}
          </div>
        </Show>
      </div>
    </Show>
  );
}

/** A view's menu (a right click on its icon or tab): move it to another container. */
function MoveMenu() {
  const close = () => setMenu("view", null);
  onMount(() => {
    const away = () => close();
    window.addEventListener("pointerdown", away);
    window.addEventListener("blur", away);
    onCleanup(() => {
      window.removeEventListener("pointerdown", away);
      window.removeEventListener("blur", away);
    });
  });
  return (
    <Show when={menu.view}>
      {(v) => (
        <div class="menu" role="menu" style={{ left: `${menu.x}px`, top: `${menu.y}px` }} onPointerDown={(e) => e.stopPropagation()}>
          <div class="menu-title">{VIEWS[v()].title}</div>
          <For each={(["primary", "secondary", "panel"] as Where[]).filter((w) => !workbench[w].includes(v()))}>
            {(w) => (
              <button
                role="menuitem"
                onClick={() => {
                  setWorkbench(move(plain(), v(), w));
                  close();
                }}
              >
                Move to {WHERE_NAME[w]}
              </button>
            )}
          </For>
          <button
            role="menuitem"
            onClick={() => {
              setWorkbench(defaults());
              close();
            }}
          >
            Reset View Locations
          </button>
        </div>
      )}
    </Show>
  );
}

export default function Coder(props: { agent: () => JSX.Element }) {
  // Restored as it was left: the folders, the view, the panel, the sizes, the tabs and their cursors, and the
  // changes not saved.
  const restore = () => {
    const l = loadLayout();
    if (!l) return;
    for (const f of l.folders) setOpen(f, true);
    setWorkbenchStore(reconcile(normalize(l.workbench)));
    if (l.sizes) setSizes({ ...sizes, ...l.sizes });
    for (const [p, c] of Object.entries(l.cursors ?? {})) cursors.set(p, c);
    for (const t of l.tabs) openFile(t);
    if (l.active && l.tabs.includes(l.active)) setActive(l.active);
  };
  let restored = false;
  createEffect(
    on(
      () => session.folder,
      (folder) => {
        if (folder && !restored) {
          restored = true;
          restore();
          send({ type: "backups" });
        }
      },
    ),
  );
  // What is on screen is watched: the open files and folders (sent again after a reconnect).
  createEffect(() => {
    const files = tabs.filter((t) => !t.diff).map((t) => t.path);
    const folders = Object.entries(open).filter(([, o]) => o).map(([p]) => p);
    if (session.connected) send({ type: "watch", files, folders });
    keepLayout();
  });

  onMount(() => {
    const keys = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "p" && e.shiftKey) {
        e.preventDefault();
        setPalette({ open: true, text: "", at: 0 });
      } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "p") {
        e.preventDefault();
        openQuick();
      } else if (e.ctrlKey && e.key === "`") {
        e.preventDefault();
        setWorkbench(toggleView(plain(), "terminal"));
      } else if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key.toLowerCase() === "f") {
        e.preventDefault();
        setView("search");
      } else if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key.toLowerCase() === "e") {
        e.preventDefault();
        setView("files");
      } else if ((e.ctrlKey || e.metaKey) && e.altKey && e.key.toLowerCase() === "b") {
        e.preventDefault();
        setWorkbench(toggle(plain(), "secondary"));
      } else if ((e.ctrlKey || e.metaKey) && e.altKey && e.key.toLowerCase() === "i") {
        e.preventDefault();
        setWorkbench(toggleView(plain(), "agent"));
      } else if ((e.ctrlKey || e.metaKey) && !e.shiftKey && e.key.toLowerCase() === "b") {
        e.preventDefault();
        setWorkbench(toggle(plain(), "primary"));
      } else if ((e.ctrlKey || e.metaKey) && !e.shiftKey && e.key.toLowerCase() === "j") {
        e.preventDefault();
        setWorkbench(toggle(plain(), "panel"));
      } else if (e.key === "Escape" && dragging()) {
        setDragging(null);
      }
    };
    window.addEventListener("keydown", keys);
    window.addEventListener("focus", checkDisk);
    onCleanup(() => {
      window.removeEventListener("keydown", keys);
      window.removeEventListener("focus", checkDisk);
    });
  });
  const shown = (w: Where) => workbench.shown[w] && workbench[w].length > 0;
  const panelSide = () => workbench.panelAt !== "bottom";
  return (
    <div class="workbench" classList={{ "side-right": workbench.side === "right", dragging: !!dragging() }}>
      <ActivityBar />
      <Show when={shown("primary")}>
        <div class="pane" style={{ width: `${sizes.side}px` }}>
          <Container where="primary" agent={props.agent} />
        </div>
        <Splitter dir="col" get={() => sizes.side} set={(n) => setSizes("side", n)} min={170} max={700} invert={workbench.side === "right"} />
      </Show>
      <div class="main" classList={{ [`panel-${workbench.panelAt}`]: true }}>
        <div class="editor-col">
          <Editor />
        </div>
        <Show when={shown("panel")}>
          <Show
            when={panelSide()}
            fallback={<Splitter dir="row" get={() => sizes.term} set={(n) => setSizes("term", n)} min={80} max={900} invert />}
          >
            <Splitter
              dir="col"
              get={() => sizes.panelWidth}
              set={(n) => setSizes("panelWidth", n)}
              min={200}
              max={1000}
              invert={workbench.panelAt === "right"}
            />
          </Show>
          <div class="pane" style={panelSide() ? { width: `${sizes.panelWidth}px` } : { height: `${sizes.term}px` }}>
            <Container where="panel" agent={props.agent} />
          </div>
        </Show>
      </div>
      <Show when={shown("secondary")}>
        <Splitter dir="col" get={() => sizes.agent} set={(n) => setSizes("agent", n)} min={260} max={1000} invert={workbench.side === "left"} />
        <div class="pane" style={{ width: `${sizes.agent}px` }}>
          <Container where="secondary" agent={props.agent} />
        </div>
      </Show>
      <DropEdges />
      <MoveMenu />
      <Show when={quick.open}>
        <QuickOpen />
      </Show>
      <Show when={palette.open}>
        <Palette />
      </Show>
    </div>
  );
}
