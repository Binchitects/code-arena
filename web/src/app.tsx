// Agent mode: the conversation in one column, the box, one status line. A turn's tool calls fold into a line, and
// the questions the agent asks are cards in the thread.
import { For, Match, Show, Suspense, Switch, createEffect, createSignal, lazy, onMount } from "solid-js";
import { produce } from "solid-js/store";
import { type Item, type Tool, detail, items, send, session, setDetail, setItems } from "./stream";

const MODES = ["ask", "auto-edit", "plan", "yolo"];
const THINKING = ["default", "off", "low", "medium", "high", "xhigh"];

/** What the box holds: Edit puts a question back in it. */
const [draft, setDraft] = createSignal("");

/** Tokens as people read them: 900, 18.2k, 131k. */
export function short(n: number): string {
  if (n < 1000) return String(n);
  const k = n / 1000;
  return k >= 100 || Math.abs(k - Math.round(k)) < 0.05 ? `${Math.round(k)}k` : `${k.toFixed(1)}k`;
}

/** A turn's calls in a line: "Read 4 files, ran 2 commands, edited cart.rs". */
export function summary(tools: Tool[]): string {
  const count = (names: string[]) => tools.filter((t) => names.includes(t.name)).length;
  const parts: string[] = [];
  const read = count(["read_file"]);
  if (read) parts.push(`read ${read} file${read > 1 ? "s" : ""}`);
  const searched = count(["grep", "glob", "list_dir", "search_sessions", "session_search"]);
  if (searched) parts.push(`searched ${searched} time${searched > 1 ? "s" : ""}`);
  const ran = count(["run_shell"]);
  if (ran) parts.push(`ran ${ran} command${ran > 1 ? "s" : ""}`);
  const edited = [...new Set(tools.filter((t) => ["edit_file", "write_file"].includes(t.name)).map((t) => t.subject))];
  if (edited.length) parts.push(`edited ${edited.length > 2 ? `${edited.length} files` : edited.map((p) => p.split("/").pop()).join(", ")}`);
  const known = ["read_file", "grep", "glob", "list_dir", "search_sessions", "session_search", "run_shell", "edit_file", "write_file"];
  const other = tools.filter((t) => !known.includes(t.name)).length;
  if (other) parts.push(`${other} other call${other > 1 ? "s" : ""}`);
  const failed = tools.filter((t) => t.done && t.ok === false).length;
  const line = parts.join(", ");
  return (line.charAt(0).toUpperCase() + line.slice(1)) + (failed ? ` (${failed} failed)` : "");
}

/** A paragraph with its `inline code` shown as code. */
function Inline(props: { text: string }) {
  const parts = () => props.text.split("`");
  return (
    <p>
      <For each={parts()}>{(t, i) => (i() % 2 && i() < parts().length - 1 ? <code class="inline">{t}</code> : t)}</For>
    </p>
  );
}

/** Text with its fenced code blocks shown as code. */
function Prose(props: { text: string }) {
  const parts = () => props.text.split(/```[^\n]*\n?/);
  return (
    <div class="prose">
      <For each={parts()}>{(p, i) => (i() % 2 ? <pre class="code">{p.replace(/\n$/, "")}</pre> : <Inline text={p.trim() ? p : ""} />)}</For>
    </div>
  );
}

function Ask(props: { item: Extract<Item, { kind: "ask" }> }) {
  const answer = (a: string) => send({ type: "answer", id: props.item.id, answer: a });
  return (
    <div class="card ask" role="group" aria-label="The agent asks">
      <div class="ask-what">
        <span class="tool-name">{props.item.tool}</span>
        <code class="subject">{props.item.subject}</code>
      </div>
      <Show
        when={!props.item.answer}
        fallback={
          <div class="answered">
            {{ no: "Denied", always: "Allowed for this session", yes: "Allowed", gone: "No longer asked: the turn ended" }[props.item.answer ?? ""] ?? ""}
          </div>
        }
      >
        <div class="buttons">
          <button class="primary" onClick={() => answer("yes")}>Allow</button>
          <button onClick={() => answer("always")}>Always, this session</button>
          <button class="quiet" onClick={() => answer("no")}>Deny</button>
        </div>
      </Show>
    </div>
  );
}

function Assistant(props: { item: Extract<Item, { kind: "assistant" }>; index: number }) {
  const running = () => props.item.tools.some((t) => !t.done);
  return (
    <div class="assistant">
      <Show when={props.item.tools.length}>
        <button
          class="tools-line"
          aria-expanded={props.item.open}
          onClick={() =>
            setItems(
              props.index,
              produce((x) => {
                if (x.kind === "assistant") x.open = !x.open;
              }),
            )
          }
        >
          <span class="chevron">{props.item.open ? "▾" : "▸"}</span>
          {summary(props.item.tools)}
          <Show when={running()}>
            <span class="spinner" aria-label="running" />
          </Show>
        </button>
        <Show when={props.item.open}>
          <ul class="tools">
            <For each={props.item.tools}>
              {(t) => (
                <li classList={{ failed: t.ok === false }}>
                  <div class="tool-head">
                    <span class="tool-name">{t.name}</span>
                    <code class="subject">{t.subject}</code>
                  </div>
                  <Show when={t.text}>
                    <pre class="result">{t.text}{t.cut ? "\n…" : ""}</pre>
                  </Show>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </Show>
      <Show when={props.item.text}>
        <Prose text={props.item.text} />
      </Show>
    </div>
  );
}

/** One of the person's messages, with what can be done from it: Edit, Answer again, Rewind to here. */
function User(props: { item: Extract<Item, { kind: "user" }> }) {
  const [choosing, setChoosing] = createSignal(false);
  const q = () => props.item.q;
  const rewind = (what: string) => {
    setChoosing(false);
    send({ type: "rewind", q: q(), what });
  };
  return (
    <div class="user-wrap">
      <div class="user">{props.item.text}</div>
      <Show when={q() && !session.running}>
        <div class="user-actions">
          <button
            class="quiet small"
            title="Go back to before this message, and put it in the box to change it"
            onClick={() => {
              send({ type: "rewind", q: q(), what: "both" });
              setDraft(props.item.text);
            }}
          >
            Edit
          </button>
          <button
            class="quiet small"
            title="Go back to before this message, and ask it again"
            onClick={() => send({ type: "again", q: q(), text: props.item.text })}
          >
            Answer again
          </button>
          <button class="quiet small" onClick={() => setChoosing(!choosing())} aria-expanded={choosing()}>
            Rewind to here
          </button>
        </div>
      </Show>
      <Show when={choosing()}>
        <div class="card rewind" role="group" aria-label="Rewind to before this message">
          <p>Go back to before this message? What came after it goes; the files its turns changed are put back.</p>
          <div class="buttons">
            <button class="primary" onClick={() => rewind("both")}>Conversation and code</button>
            <button onClick={() => rewind("chat")}>Conversation only</button>
            <button onClick={() => rewind("code")}>Code only</button>
            <button class="quiet" onClick={() => setChoosing(false)}>Cancel</button>
          </div>
        </div>
      </Show>
    </div>
  );
}

/** How full the window is, as a ring; the share at which it compacts marked. */
function Ring() {
  const share = () => {
    const c = session.context;
    return c && c.size ? Math.min(1, c.used / c.size) : 0;
  };
  const r = 7;
  const around = 2 * Math.PI * r;
  return (
    <button
      class="ring"
      title={session.context ? `Context: ${short(session.context.used)} of ${short(session.context.size)} tokens${session.context.compact_at ? `, compacts at ${session.context.compact_at}%` : ""}` : "Context"}
      aria-label="Context"
      aria-expanded={detail.open}
      onClick={() => {
        const open = !detail.open;
        setDetail("open", open);
        if (open) send({ type: "context" });
      }}
    >
      <svg width="18" height="18" viewBox="0 0 18 18" aria-hidden="true">
        <circle cx="9" cy="9" r={r} class="ring-back" />
        <circle
          cx="9"
          cy="9"
          r={r}
          class="ring-used"
          classList={{ high: share() * 100 >= (session.context?.compact_at ?? 90) }}
          stroke-dasharray={`${share() * around} ${around}`}
          transform="rotate(-90 9 9)"
        />
      </svg>
      <span>{Math.round(share() * 100)}%</span>
    </button>
  );
}

/** What fills the next request: by kind, the biggest messages, and Compact now. */
function ContextPanel() {
  const total = () => (detail.data?.kinds ?? []).reduce((a, k) => a + k.tokens, 0) || 1;
  return (
    <section class="context-panel" aria-label="Context">
      <header>
        <strong>Context</strong>
        <Show when={detail.data}>
          {(d) => (
            <span class="muted">
              {short(d().context.used)} of {short(d().context.size)} tokens
              {d().context.compact_at ? ` · compacts at ${d().context.compact_at}%` : ""}
            </span>
          )}
        </Show>
        <span class="grow" />
        <button class="small" disabled={session.running} onClick={() => send({ type: "compact" })}>
          Compact now
        </button>
        <button class="quiet small" aria-label="Close" onClick={() => setDetail("open", false)}>
          ✕
        </button>
      </header>
      <Show when={detail.data} fallback={<p class="muted">Counting…</p>}>
        {(d) => (
          <>
            <ul class="kinds">
              <For each={d().kinds.filter((k) => k.tokens > 0)}>
                {(k) => (
                  <li>
                    <span class="kind">{k.kind}</span>
                    <span class="bar">
                      <span style={{ width: `${(100 * k.tokens) / total()}%` }} />
                    </span>
                    <span class="num">{short(k.tokens)}</span>
                  </li>
                )}
              </For>
            </ul>
            <Show when={d().biggest.length}>
              <p class="muted small-heading">The biggest messages</p>
              <ul class="biggest">
                <For each={d().biggest}>
                  {(b) => (
                    <li>
                      <span class="num">{short(b.tokens)}</span>
                      <span class="role">{b.role}</span>
                      <span class="start">{b.start}</span>
                    </li>
                  )}
                </For>
              </ul>
            </Show>
          </>
        )}
      </Show>
    </section>
  );
}

function Box() {
  const text = draft;
  const setText = setDraft;
  let area!: HTMLTextAreaElement;
  const grow = () => {
    area.style.height = "auto";
    area.style.height = `${Math.min(area.scrollHeight, 240)}px`;
  };
  const submit = () => {
    const t = text().trim();
    if (!t) return;
    send({ type: "send", text: t });
    setText("");
    queueMicrotask(grow);
  };
  onMount(() => area.focus());
  // A question put back by Edit: the box grows to it and takes the focus.
  createEffect(() => {
    draft();
    queueMicrotask(() => {
      grow();
      if (document.activeElement !== area) area.focus();
    });
  });
  return (
    <form
      class="box"
      onSubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      <textarea
        ref={area}
        rows={1}
        value={text()}
        placeholder={session.running ? "Sent while it works…" : "Ask Code Arena…  (/ for commands)"}
        aria-label="Message"
        onInput={(e) => {
          setText(e.currentTarget.value);
          grow();
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
            e.preventDefault();
            submit();
          } else if (e.key === "Escape" && session.running) {
            send({ type: "stop" });
          }
        }}
      />
      <Show
        when={session.running}
        fallback={
          <button type="submit" class="primary" disabled={!text().trim() || !session.connected}>
            Send
          </button>
        }
      >
        <button type="button" onClick={() => send({ type: "stop" })}>
          Stop
        </button>
      </Show>
    </form>
  );
}

function Status() {
  return (
    <footer class="status">
      <span class="dot" classList={{ on: session.connected }} title={session.connected ? "Connected" : "Connecting…"} />
      <label class="pick" title="The model">
        <span class="sr">Model</span>
        <select value={session.model} disabled={session.running} onChange={(e) => send({ type: "model", model: e.currentTarget.value })}>
          <For each={session.models.includes(session.model) ? session.models : [session.model, ...session.models]}>
            {(m) => <option value={m}>{m}</option>}
          </For>
        </select>
      </label>
      <label class="pick" title="Thinking">
        <span class="sr">Thinking</span>
        <select value={session.thinking} disabled={session.running} onChange={(e) => send({ type: "thinking", level: e.currentTarget.value })}>
          <For each={THINKING}>{(t) => <option value={t}>{t === "default" ? "thinking: default" : `thinking: ${t}`}</option>}</For>
        </select>
      </label>
      <label class="pick" title="The mode: what runs without asking">
        <span class="sr">Mode</span>
        <select value={session.mode} onChange={(e) => send({ type: "mode", mode: e.currentTarget.value })}>
          <For each={MODES}>{(m) => <option value={m}>{m}</option>}</For>
        </select>
      </label>
      <Ring />
      <span class="folder" title={session.folder}>{session.folder.split("/").pop()}</span>
      <Show when={session.lastTurn}>
        {(t) => (
          <span class="last">
            {(t().ms / 1000).toFixed(1)} s · {t().prompt.toLocaleString()} in
            {t().cached ? ` (${Math.round((100 * t().cached) / Math.max(1, t().prompt))}% cached)` : ""} · {t().completion.toLocaleString()} out
          </span>
        )}
      </Show>
    </footer>
  );
}

/** The conversation, kept at its end while the person is there. */
function Thread() {
  let thread!: HTMLDivElement;
  let stick = true;
  createEffect(() => {
    // Read on each change of the thread.
    JSON.stringify(items);
    if (stick) queueMicrotask(() => (thread.scrollTop = thread.scrollHeight));
  });
  return (
    <main
      class="thread"
      ref={thread}
      onScroll={() => (stick = thread.scrollHeight - thread.scrollTop - thread.clientHeight < 60)}
      aria-live="polite"
    >
      <div class="column">
        <Show when={!items.length}>
          <div class="empty">
            <h1>Code Arena</h1>
            <p>Ask about this project, or ask for a change. The agent reads, edits and runs commands as your mode allows.</p>
          </div>
        </Show>
        <For each={items}>
          {(item, i) => (
            <Switch>
              <Match when={item.kind === "user" && item}>{(u) => <User item={u()} />}</Match>
              <Match when={item.kind === "assistant" && item}>{(a) => <Assistant item={a()} index={i()} />}</Match>
              <Match when={item.kind === "ask" && item}>{(a) => <Ask item={a()} />}</Match>
              <Match when={item.kind === "queued" && item}>
                {(q) => (
                  <div class="user queued">
                    {q().text}
                    <span class="tag">Sent while it worked</span>
                  </div>
                )}
              </Match>
              <Match when={item.kind === "notice" && item}>{(n) => <div class="notice">{n().text}</div>}</Match>
              <Match when={item.kind === "error" && item}>{(e) => <div class="error">{e().text}</div>}</Match>
            </Switch>
          )}
        </For>
        <Show when={session.running && !items.some((x, i) => i === items.length - 1 && x.kind === "assistant" && x.text)}>
          <div class="working">
            <span class="spinner" /> Working…
          </div>
        </Show>
      </div>
    </main>
  );
}

/** The agent: its thread, then the Context panel when open and the box. */
function Agent() {
  return (
    <>
      <Thread />
      <div class="bottom">
        <div class="column">
          <Show when={detail.open}>
            <ContextPanel />
          </Show>
          <Box />
        </div>
      </div>
    </>
  );
}

const Coder = lazy(() => import("./coder"));

/** Which face the IDE shows, kept for the next visit. */
const [face, setFace] = createSignal<"agent" | "coder">(
  (() => {
    try {
      return localStorage.getItem("code-arena.face") === "coder" ? "coder" : "agent";
    } catch {
      return "agent";
    }
  })(),
);

function choose(f: "agent" | "coder") {
  setFace(f);
  try {
    localStorage.setItem("code-arena.face", f);
  } catch {
    // Not kept: the next visit opens Agent mode.
  }
}

export function App() {
  return (
    <div class="app">
      <header class="top">
        <div class="faces" role="tablist" aria-label="Mode">
          <button role="tab" aria-selected={face() === "agent"} classList={{ on: face() === "agent" }} onClick={() => choose("agent")}>
            Agent
          </button>
          <button role="tab" aria-selected={face() === "coder"} classList={{ on: face() === "coder" }} onClick={() => choose("coder")}>
            Coder
          </button>
        </div>
      </header>
      <Show when={face() === "coder"} fallback={<div class="agent-face"><Agent /></div>}>
        <Suspense fallback={<div class="loading muted">Opening the editor…</div>}>
          <Coder agent={() => <Agent />} />
        </Suspense>
      </Show>
      <div class="status-row">
        <div class="column wide">
          <Status />
        </div>
      </div>
    </div>
  );
}
