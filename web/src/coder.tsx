// Coder mode: the project's files in a tree, files in tabs of VS Code's own editor (Monaco), terminals in a panel,
// and the agent beside them. Loaded only when Coder mode is first opened.
import "@xterm/xterm/css/xterm.css";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import * as monaco from "monaco-editor/editor/editor.api";
import "monaco-editor/basic-languages/monaco.contribution";
import EditorWorker from "monaco-editor/editor/editor.worker?worker";
import { For, type JSX, Show, createEffect, createSignal, on, onCleanup, onMount } from "solid-js";
import { createStore, produce } from "solid-js/store";
import { listen, send, session } from "./stream";

(self as any).MonacoEnvironment = { getWorker: () => new EditorWorker() };

type Entry = { name: string; dir: boolean; link: boolean };
type Tab = { path: string; dirty: boolean; error?: string; diff?: boolean; onDisk?: "changed" | "gone" };
type Hit = { path: string; line: number; text: string };
type Change = { path: string; before: string | null; after: string | null };

const [tree, setTree] = createStore<Record<string, Entry[] | { error: string }>>({});
const [open, setOpen] = createStore<Record<string, boolean>>({ "": true });
const [tabs, setTabs] = createStore<Tab[]>([]);
const [active, setActive] = createSignal("");
const [view, setView] = createSignal<"files" | "search" | "changes">("files");
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

type Layout = { tabs: string[]; active: string; folders: string[]; view: string; panel: boolean; cursors: Record<string, { lineNumber: number; column: number }>; sizes: Sizes };
type Sizes = { side: number; agent: number; term: number };
export const [sizes, setSizes] = createStore<Sizes>({ side: 240, agent: 380, term: 240 });
export const [panelOpen, setPanelOpen] = createSignal(true);

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
      view: view(),
      panel: panelOpen(),
      cursors: Object.fromEntries(cursors),
      sizes: { ...sizes },
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

const dark = () => window.matchMedia("(prefers-color-scheme: dark)").matches;

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
      if (view() === "changes") send({ type: "changes" });
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
      theme: dark() ? "vs-dark" : "vs",
      fontSize: 13,
      renderSideBySide: true,
    });
    editor = monaco.editor.create(host, {
      model: null,
      automaticLayout: true,
      theme: dark() ? "vs-dark" : "vs",
      fontSize: 13,
      minimap: { enabled: true },
      scrollBeyondLastLine: false,
    });
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => {
      if (active()) save(active());
    });
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyP, () => openQuick());
    editor.onDidChangeCursorPosition((e) => {
      const path = active();
      if (path && !path.startsWith("Δ ")) {
        cursors.set(path, { lineNumber: e.position.lineNumber, column: e.position.column });
        keepLayout();
      }
    });
    const scheme = window.matchMedia("(prefers-color-scheme: dark)");
    const onScheme = () => monaco.editor.setTheme(dark() ? "vs-dark" : "vs");
    scheme.addEventListener("change", onScheme);
    if (active()) showModel(active());
    onCleanup(() => {
      scheme.removeEventListener("change", onScheme);
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

function Terminals() {
  const [ids, setIds] = createSignal<number[]>([]);
  const [shown, setShown] = createSignal(0);
  const add = () => {
    const id = nextTerm++;
    setIds([...ids(), id]);
    setShown(id);
  };
  // The first terminal opens once Code Arena says it has terminals (its state may come after the panel).
  let first = true;
  createEffect(() => {
    if (session.terminals && first) {
      first = false;
      add();
    }
  });
  return (
    <div class="terminals" style={{ height: `${sizes.term}px` }}>
      <div class="term-tabs">
        <For each={ids()}>
          {(id) => (
            <button class="term-tab" classList={{ active: shown() === id }} onClick={() => setShown(id)}>
              Terminal {id}
            </button>
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
      theme: { background: css.getPropertyValue("--panel").trim(), foreground: css.getPropertyValue("--text").trim() },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    fit.fit();
    send({ type: "term_open", id: props.id, cols: term.cols, rows: term.rows });
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
    onCleanup(() => {
      stop();
      ro.disconnect();
      send({ type: "term_close", id: props.id });
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

export default function Coder(props: { agent: () => JSX.Element }) {
  const panel = panelOpen;
  const setPanel = (v: boolean) => {
    setPanelOpen(v);
    keepLayout();
  };
  // Restored as it was left: the folders, the view, the panel, the sizes, the tabs and their cursors, and the
  // changes not saved.
  const restore = () => {
    const l = loadLayout();
    if (!l) return;
    for (const f of l.folders) setOpen(f, true);
    if (l.view === "search" || l.view === "changes" || l.view === "files") setView(l.view);
    setPanelOpen(l.panel);
    if (l.sizes) setSizes(l.sizes);
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
  createEffect(on(view, keepLayout, { defer: true }));
  onMount(() => {
    const keys = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "p" && !e.shiftKey) {
        e.preventDefault();
        openQuick();
      } else if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key.toLowerCase() === "f") {
        e.preventDefault();
        setView("search");
      }
    };
    window.addEventListener("keydown", keys);
    window.addEventListener("focus", checkDisk);
    onCleanup(() => {
      window.removeEventListener("keydown", keys);
      window.removeEventListener("focus", checkDisk);
    });
  });
  return (
    <div class="coder" style={{ "grid-template-columns": `${sizes.side}px 4px minmax(0, 1fr) 4px ${sizes.agent}px` }}>
      <nav class="side" aria-label="Side bar">
        <div class="views" role="tablist">
          <button role="tab" aria-selected={view() === "files"} classList={{ on: view() === "files" }} onClick={() => setView("files")}>
            Files
          </button>
          <button role="tab" aria-selected={view() === "search"} classList={{ on: view() === "search" }} onClick={() => setView("search")}>
            Search
          </button>
          <button role="tab" aria-selected={view() === "changes"} classList={{ on: view() === "changes" }} onClick={() => setView("changes")}>
            Changes
          </button>
        </div>
        <Show when={view() === "files"}>
          <div class="side-head">{session.root || "Project"}</div>
          <div class="tree">
            <Folder path="" depth={0} />
          </div>
        </Show>
        <Show when={view() === "search"}>
          <SearchView />
        </Show>
        <Show when={view() === "changes"}>
          <ChangesView />
        </Show>
      </nav>
      <Splitter dir="col" get={() => sizes.side} set={(n) => setSizes("side", n)} min={140} max={600} />
      <Show when={quick.open}>
        <QuickOpen />
      </Show>
      <div class="center">
        <Editor />
        <Show when={panel()}>
          <Splitter dir="row" get={() => sizes.term} set={(n) => setSizes("term", n)} min={80} max={800} invert />
        </Show>
        <div class="panel-bar">
          <button class="quiet small" aria-expanded={panel()} onClick={() => setPanel(!panel())}>
            {panel() ? "▾" : "▸"} Terminal
          </button>
        </div>
        <Show when={panel()}>
          <Terminals />
        </Show>
      </div>
      <Splitter dir="col" class="agent-split" get={() => sizes.agent} set={(n) => setSizes("agent", n)} min={260} max={900} invert />
      <aside class="agent-side" aria-label="Agent">
        {props.agent()}
      </aside>
    </div>
  );
}
