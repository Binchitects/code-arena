// The live stream to Code Arena, and the thread as the page shows it. Everything the session does arrives here as
// one JSON message at a time; the thread is a store, so a streamed token changes one text node.
import { createStore, produce } from "solid-js/store";

export type Tool = { name: string; subject: string; done: boolean; ok?: boolean; text?: string; cut?: boolean };

export type Item =
  | { kind: "user"; text: string }
  | { kind: "assistant"; text: string; reasoning: string; tools: Tool[]; open: boolean }
  | { kind: "notice"; text: string }
  | { kind: "error"; text: string }
  | { kind: "queued"; text: string }
  | { kind: "ask"; id: number; tool: string; subject: string; answer?: string };

export type Session = {
  connected: boolean;
  running: boolean;
  model: string;
  mode: string;
  folder: string;
  session: string;
  edition: string;
  version: string;
  lastTurn?: { ms: number; prompt: number; cached: number; completion: number };
};

type Stored = { role: string; text: string; tools: string[] };

export const [session, setSession] = createStore<Session>({
  connected: false,
  running: false,
  model: "",
  mode: "",
  folder: "",
  session: "",
  edition: "",
  version: "",
});

export const [items, setItems] = createStore<Item[]>([]);

let socket: WebSocket | undefined;
let wait = 500;

export function send(v: Record<string, unknown>): void {
  if (socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify(v));
}

/** The answer being written: the last assistant item, made when there is none after the last message. */
function current(list: Item[]): Extract<Item, { kind: "assistant" }> {
  const last = list[list.length - 1];
  if (last?.kind === "assistant") return last;
  const made: Item = { kind: "assistant", text: "", reasoning: "", tools: [], open: false };
  list.push(made);
  return made as Extract<Item, { kind: "assistant" }>;
}

function fromStored(messages: Stored[]): Item[] {
  const out: Item[] = [];
  for (const m of messages) {
    if (m.role === "user") {
      if (m.text.startsWith("(Code Arena")) continue;
      out.push({ kind: "user", text: m.text });
    } else if (m.role === "assistant") {
      const a = current(out);
      if (m.text) a.text += (a.text ? "\n\n" : "") + m.text;
      for (const t of m.tools) a.tools.push({ name: t, subject: "", done: true, ok: true });
    }
  }
  return out;
}

function onMessage(m: any): void {
  switch (m.type) {
    case "state":
      setSession({
        model: m.model,
        mode: m.mode,
        folder: m.folder,
        session: m.session,
        edition: m.edition,
        version: m.version,
        running: m.running,
      });
      setItems(fromStored(m.messages ?? []));
      break;
    case "status":
      setSession({ model: m.model, mode: m.mode, running: m.running });
      break;
    case "user":
      setItems(produce((l) => void l.push({ kind: "user", text: m.text })));
      break;
    case "turn_start":
      setSession("running", true);
      break;
    case "text":
      setItems(produce((l) => void (current(l).text += m.delta)));
      break;
    case "reasoning":
      setItems(produce((l) => void (current(l).reasoning += m.delta)));
      break;
    case "tool_start":
      setItems(produce((l) => void current(l).tools.push({ name: m.name, subject: m.subject, done: false })));
      break;
    case "tool_end":
      setItems(
        produce((l) => {
          const a = current(l);
          const t = [...a.tools].reverse().find((x) => !x.done && x.name === m.name && x.subject === m.subject);
          if (t) Object.assign(t, { done: true, ok: m.ok, text: m.text, cut: m.cut });
          else a.tools.push({ name: m.name, subject: m.subject, done: true, ok: m.ok, text: m.text, cut: m.cut });
        }),
      );
      break;
    case "notice":
      // The engine's own word for a message sent meanwhile: the page shows that message already.
      if (/^› .*\(sent while it worked\)$/.test(m.text)) break;
      setItems(produce((l) => void l.push({ kind: "notice", text: m.text })));
      break;
    case "ask":
      setItems(produce((l) => void l.push({ kind: "ask", id: m.id, tool: m.tool, subject: m.subject })));
      break;
    case "answered":
      setItems(
        produce((l) => {
          const a = l.find((x) => x.kind === "ask" && x.id === m.id);
          if (a && a.kind === "ask") a.answer = m.answer;
        }),
      );
      break;
    case "queued":
      setItems(produce((l) => void l.push({ kind: "queued", text: m.text })));
      break;
    case "turn_end":
      setSession("running", false);
      if (m.ok) setSession("lastTurn", { ms: m.ms, ...m.usage });
      else if (m.stopped) setItems(produce((l) => void l.push({ kind: "notice", text: "Stopped." })));
      else setItems(produce((l) => void l.push({ kind: "error", text: m.error ?? "The turn failed." })));
      break;
    case "behind":
      send({ type: "state" });
      break;
  }
}

export function connect(): void {
  socket = new WebSocket(`ws://${location.host}/ws`);
  socket.onopen = () => {
    wait = 500;
    setSession("connected", true);
  };
  socket.onmessage = (e) => {
    try {
      onMessage(JSON.parse(e.data));
    } catch {
      // A message the page does not know: left out.
    }
  };
  socket.onclose = () => {
    setSession("connected", false);
    setTimeout(connect, wait);
    wait = Math.min(wait * 2, 5000);
  };
}
