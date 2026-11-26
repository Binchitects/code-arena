// Agent mode: the conversation in one column, the box, one status line. A turn's tool calls fold into a line, and
// the questions the agent asks are cards in the thread.
import { For, Match, Show, Switch, createEffect, createSignal, onMount } from "solid-js";
import { produce } from "solid-js/store";
import { type Item, type Tool, items, send, session, setItems } from "./stream";

const MODES = ["ask", "auto-edit", "plan", "yolo"];

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
        fallback={<div class="answered">{props.item.answer === "no" ? "Denied" : props.item.answer === "always" ? "Allowed for this session" : "Allowed"}</div>}
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

function Box() {
  const [text, setText] = createSignal("");
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
  const nextMode = () => MODES[(MODES.indexOf(session.mode) + 1) % MODES.length];
  return (
    <footer class="status">
      <span class="dot" classList={{ on: session.connected }} title={session.connected ? "Connected" : "Connecting…"} />
      <span>{session.model || "…"}</span>
      <span class="sep">·</span>
      <button class="link" title="Change the mode" onClick={() => send({ type: "mode", mode: nextMode() })}>
        {session.mode}
      </button>
      <span class="sep">·</span>
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

export function App() {
  let thread!: HTMLDivElement;
  let stick = true;
  createEffect(() => {
    // Read on each change of the thread: kept at its end while the person is there.
    JSON.stringify(items);
    if (stick) queueMicrotask(() => (thread.scrollTop = thread.scrollHeight));
  });
  return (
    <div class="app">
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
                <Match when={item.kind === "user" && item}>{(u) => <div class="user">{u().text}</div>}</Match>
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
      <div class="bottom">
        <div class="column">
          <Box />
          <Status />
        </div>
      </div>
    </div>
  );
}
