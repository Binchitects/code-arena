# Downloads the milestones need

Each needs the person's yes before it is fetched. Sizes from the registries' own metadata (unpacked), 2026-11-26.

## The IDE (Agent mode, Coder mode): the milestones after the terminal harness

| what | version | size | for |
|---|---|---|---|
| Node.js in the build image (`node:22-bookworm-slim`, or Node's own tarball) | 22 | about 50 MB | building the front end |
| solid-js | 2.0.4 | 0.8 MB | the UI |
| vite, vite-plugin-solid | 8.3.8, 2.11.17 | 2.4 MB, and their own packages (rollup, esbuild: about 15 MB more) | building it |
| typescript | 7.0.2 | 2.4 MB | the front end's types |
| monaco-editor | 0.58.0 | 76.4 MB (the build keeps what it uses, a few MB) | the editor (Coder mode) |
| @xterm/xterm, @xterm/addon-fit | 6.0.0, 0.11.0 | 5.6 MB | terminals |
| a WebSocket crate (tokio-tungstenite) | 0.26 | about 0.3 MB with tungstenite | the IDE's live streams and terminals |

The server itself (hyper's server side) is built from crates already in the cache: nothing to fetch.

## Later

| what | for |
|---|---|
| tree-sitter and its grammars (Rust crates) | the code index |
| language servers and debug adapters (fetched on the person's machine, by the person's choice, not built in) | LSP, DAP |
| cargo-fuzz | 0.9's fuzzing |
