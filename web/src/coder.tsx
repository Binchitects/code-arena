// Coder mode: the project's files in a tree, files in tabs of VS Code's own editor (Monaco), terminals in a panel,
// and the agent beside them. Loaded only when Coder mode is first opened.
import "@xterm/xterm/css/xterm.css";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import * as monaco from "monaco-editor/editor/editor.api";
import "monaco-editor/basic-languages/monaco.contribution";
import EditorWorker from "monaco-editor/editor/editor.worker?worker";
import { For, type JSX, Show, createSignal, onCleanup, onMount } from "solid-js";
import { createStore, produce } from "solid-js/store";
import { listen, send, session } from "./stream";

(self as any).MonacoEnvironment = { getWorker: () => new EditorWorker() };

type Entry = { name: string; dir: boolean; link: boolean };
type Tab = { path: string; dirty: boolean; error?: string };

const [tree, setTree] = createStore<Record<string, Entry[] | { error: string }>>({});
const [open, setOpen] = createStore<Record<string, boolean>>({ "": true });
const [tabs, setTabs] = createStore<Tab[]>([]);
const [active, setActive] = createSignal("");

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

function openFile(path: string) {
  if (!tabs.some((t) => t.path === path)) setTabs(produce((l) => void l.push({ path, dirty: false })));
  setActive(path);
  if (!models.has(path)) send({ type: "open", path });
}

function closeTab(path: string) {
  const t = tabs.find((x) => x.path === path);
  if (t?.dirty && !confirm(`${path} has changes not saved. Close it anyway?`)) return;
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
      model.onDidChangeContent(() => setDirty(m.path, model.getAlternativeVersionId() !== entry.saved));
      if (active() === m.path) showModel(m.path);
      break;
    }
    case "saved": {
      const had = models.get(m.path);
      if (m.error) alert(`${m.path} was not saved: ${m.error}`);
      else if (had) {
        had.saved = had.model.getAlternativeVersionId();
        setDirty(m.path, false);
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

function showModel(path: string) {
  const m = models.get(path);
  if (editor && m) {
    editor.setModel(m.model);
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
  onMount(() => {
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
    const scheme = window.matchMedia("(prefers-color-scheme: dark)");
    const onScheme = () => monaco.editor.setTheme(dark() ? "vs-dark" : "vs");
    scheme.addEventListener("change", onScheme);
    if (active()) showModel(active());
    onCleanup(() => {
      scheme.removeEventListener("change", onScheme);
      editor?.dispose();
      editor = undefined;
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
      <Show when={tabs.find((t) => t.path === active())?.error}>
        {(e) => <div class="editor-error">{active()}: {e()}</div>}
      </Show>
      <Show when={!tabs.length}>
        <div class="editor-empty muted">Open a file from the tree. Ctrl+S saves it.</div>
      </Show>
      <div class="monaco" ref={host} classList={{ hidden: !tabs.length || !!tabs.find((t) => t.path === active())?.error }} />
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
  onMount(() => {
    if (session.terminals) add();
  });
  return (
    <div class="terminals">
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

/** Coder mode, with the agent's thread and box (given by Agent mode) beside the editor. */
export default function Coder(props: { agent: () => JSX.Element }) {
  const [panel, setPanel] = createSignal(true);
  return (
    <div class="coder">
      <nav class="side" aria-label="Explorer">
        <div class="side-head">{session.root || "Project"}</div>
        <div class="tree">
          <Folder path="" depth={0} />
        </div>
      </nav>
      <div class="center">
        <Editor />
        <div class="panel-bar">
          <button class="quiet small" aria-expanded={panel()} onClick={() => setPanel(!panel())}>
            {panel() ? "▾" : "▸"} Terminal
          </button>
        </div>
        <Show when={panel()}>
          <Terminals />
        </Show>
      </div>
      <aside class="agent-side" aria-label="Agent">
        {props.agent()}
      </aside>
    </div>
  );
}
