import { createEffect, createRoot } from "solid-js";
import { render } from "solid-js/web";
import { App } from "./app";
import { connect, listen, send } from "./stream";
import "./style.css";
import { apply, current, setChoice, setCustom } from "./theme-store";
import type { Theme } from "./themes";

// The theme first, so the page paints in it.
apply(current());
createRoot(() => createEffect(() => apply(current())));
listen((m) => {
  if (m.type === "state") {
    setChoice(m.theme ?? "");
    send({ type: "themes" });
  } else if (m.type === "theme") setChoice(m.theme ?? "");
  else if (m.type === "themes") setCustom((m.items ?? []) as Theme[]);
});

connect();
render(() => <App />, document.getElementById("root")!);
