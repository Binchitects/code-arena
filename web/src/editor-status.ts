// What Coder mode's editor says of the file shown, for the status line (kept apart so Agent mode does not load the
// editor to show it).
import { createStore } from "solid-js/store";

export type EditorStatus = { shown: boolean; line: number; column: number; language: string; indent: string; eol: string };

export const [editorStatus, setEditorStatus] = createStore<EditorStatus>({
  shown: false,
  line: 1,
  column: 1,
  language: "",
  indent: "",
  eol: "",
});
