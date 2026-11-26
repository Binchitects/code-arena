# Code Arena (Rust): the plan

Code Arena is Farzan Arena's own coding agent with an IDE around it. This repository
rewrites it in Rust as a product of its own, with its own versions: **0.1** first,
**1.0** the first stable release. It is built to stand beside Claude Code, Codex CLI,
OpenCode, Qwen Code, Cursor and Zed, and to work with every Farzan Arena server from
**8.0** on.

The reference is the C# Code Arena of Arena 8.1 (`src/CodeArena`, about 60k lines, and
its web IDE `src/web/src/code-arena`, about 40k lines of TypeScript) and its manual,
`docs/code-arena.md`. The [parity checklist](#parity-checklist) below lists every
feature in that manual. Each one is kept or bettered, and each is given a milestone.

## Contents

1. [What changes, and why](#what-changes-and-why)
2. [Architecture](#architecture)
3. [The UI: two modes](#the-ui-two-modes)
4. [Arena servers 8.0 and later](#arena-servers-80-and-later)
5. [The project's `.code-arena/` folder](#the-projects-code-arena-folder)
6. [Commits made by the model](#commits-made-by-the-model)
7. [Long sessions: performance by design](#long-sessions-performance-by-design)
8. [Settings: everything in one schema](#settings-everything-in-one-schema)
9. [Build, CI/CD and releases](#build-cicd-and-releases)
10. [Milestones 0.1 to 1.0](#milestones-01-to-10)
11. [Parity checklist](#parity-checklist)
12. [Rules this work follows](#rules-this-work-follows)
13. [Open questions](#open-questions)

## What changes, and why

| Today (C# 8.1) | In the rewrite |
|---|---|
| One .NET single-file program of about 70 MB per system, 136 files of C# | A Rust workspace of small crates and one static binary per system (target: under 25 MB with the IDE's page inside) |
| The agent chat has grown many controls in view at once | **Agent mode** is minimal by default: one conversation and one box. Everything else (tasks, plan, context, goal, checklist) appears only while it has something to say, or when you pin it |
| The IDE is "laid out as VS Code is" | **Coder mode** behaves as VS Code does, down to its command ids, keybindings.json, settings.json, `when` clauses, themes and TextMate grammars, with Zed's speed as the target |
| autoCommit is on, and its commit message is the prompt | autoCommit is **off by default**. When on (or on `/commit`), the **model** writes the commits: grouped by intent, each with a real message |
| Long sessions are slow to load, to scroll and to work in | Paged, indexed storage; a virtualized thread; bounded memory; every claim measured in CI (see [Long sessions](#long-sessions-performance-by-design)) |
| Some settings live only in `config.json` or in environment variables | One settings schema. The Settings UI shows every setting, including the formerly hidden ones, and a test fails when code reads a setting the schema lacks |
| Built against one Arena version | Works with Arena **8.0 and 8.1** (and later): features are negotiated by what the server answers, never by its version string |
| Harness features grew one by one | A planned harness: MCP, plugins, skills, hooks, sub-agents, scheduled tasks, checklists, goals, loops, memory, all under one model |
| No `.code-arena/` until a tool writes media | Every project gets `.code-arena/` with sensible default files the first time it is opened |

## Architecture

### One engine, three faces

```
                    +-----------------------------------------------+
                    |               code-arena (one binary)          |
                    |                                                |
  terminal  <-----> |  ca-tui        ca-cli (args, login, doctor)    |
                    |      \            |                            |
  browser / <-----> |  ca-server (axum: HTTP + WebSocket, the IDE)   |
  desktop window    |      \            |                            |
                    |       +----> ca-engine (sessions, turns, events)|
                    |                 |                              |
                    |   ca-agent  ca-tools  ca-mcp  ca-llm  ca-arena |
                    |   ca-session ca-workspace ca-git ca-index      |
                    |   ca-sandbox ca-lsp ca-dap ca-pty ca-vm        |
                    +-----------------------------------------------+
                              |              |             |
                      Arena gateway /v1   Arena /mcp   Arena /api/code-arena
```

The **engine** owns sessions and turns and publishes typed events. The terminal UI, the
IDE's server and `-p` are views of it. A turn runs the same way in each. The IDE and the
terminal can watch the same session at once.

### Crates

| crate | what it holds | key libraries |
|---|---|---|
| `ca-core` | ids, paths (XDG, `%APPDATA%`, `CODE_ARENA_HOME`), the `Secret` type (never printed, redacted in `Debug`), errors, versions | `serde`, `thiserror` |
| `ca-settings` | the settings schema and its layers (defaults, extensions, user, workspace file, folder, language), JSONC with comments kept on write, environment overrides | `schemars`, `jsonc-parser` |
| `ca-arena` | Arena's API: key sign-in (`Authorization: Bearer`, `X-Requested-With: code-arena`), password sign-in, capability probing, chat sync, memory, skills, checklists, goals, plans, projects, assistants, automation, attachments, tokenize, broadcasts, feedback, watch | `reqwest` (rustls, system roots plus the company's CA) |
| `ca-llm` | the OpenAI-compatible gateway and other providers (Anthropic Messages, OpenAI, DeepSeek, OpenRouter, Mistral, any OpenAI-compatible URL): streaming, thinking levels, tool calls, call mending, retries, stall detection, line waiting, safeguard refusals | `reqwest`, `eventsource-stream` |
| `ca-mcp` | MCP clients: stdio and streamable HTTP, progress, cancellation, deadlines, keep-alive, reconnect, tools on demand, resources | `rmcp` (the official Rust SDK) |
| `ca-agent` | the turn loop, modes, permissions and kept rules, approvals, tool registry, hooks, sub-agents, parallel steps, plan, goals, loops, checklists, compaction, context accounting, result condensing | |
| `ca-tools` | local tools: files, grep and glob, shell, jobs, git (read-only), screenshot, `run_code`, `ask_agent`, own command tools | `grep-searcher`, `ignore`, `globset` (ripgrep's crates) |
| `ca-session` | session storage, paged history, search, checkpoints and rewind, drafts, input history | `rusqlite` (bundled SQLite, WAL, FTS5) |
| `ca-workspace` | the folder, added folders, `.code-workspace` files, trust, file watching, path safety (links resolved one part at a time), `.code-arena/` creation | `notify`, `ignore` |
| `ca-git` | status, diff, log, blame, branches, stage by hunk, merge state, model-made commits | `gix` for reads, the `git` program for writes and hooks |
| `ca-index` | the code index: symbols by tree-sitter grammars, BM25, call and import graph, impact | `tree-sitter` and its grammars |
| `ca-sandbox` | bubblewrap on Linux, `sandbox-exec` on macOS, a restricted token and job object on Windows (new), Laya's command check | |
| `ca-pty` | terminals: openpty, ConPTY, the macOS session helper | `portable-pty` |
| `ca-lsp`, `ca-dap` | language-server and debug-adapter relays: URI rewriting, limits, takeover, snapshots | |
| `ca-vm` | VirtualBox, VMware, libvirt, Hyper-V | |
| `ca-server` | the IDE's API: one-run key, cookie, Host and Origin checks, JSON only, WebSockets, the embedded page | `axum`, `rust-embed` |
| `ca-tui` | the terminal conversation: line editor, history, streaming, approvals, job lines | `ratatui`, `crossterm` |
| `ca-cli` | the `code-arena` binary: commands, login, update, install, doctor | `clap` |
| `web/` | the IDE's page (TypeScript) | SolidJS, Monaco, xterm.js, vscode-textmate |

Rules for the code: `#![forbid(unsafe_code)]` outside `ca-pty` and `ca-sandbox`,
`clippy -D warnings`, `rustfmt`, no `unwrap` outside tests, `tracing` for logs with a
filter that blanks anything typed `Secret`. The Rust toolchain is pinned in
`rust-toolchain.toml` at the stable release current when 0.1 is cut.

### Why SQLite for sessions

Today each session is a JSON Lines file read from its last compaction. The rewrite keeps
one SQLite database per data folder (WAL, `synchronous=NORMAL`, each message committed
before the turn goes on). That gives:

- a page of 30 turns read by an index, never the whole file;
- FTS5 for `session_search`, `search_sessions` and `/search`, with no scan;
- results kept aside (`read_result`) as rows, not loose files;
- crash safety without a draft file: the answer being written is a row updated every
  few seconds.

`code-arena session export ID` writes a session as JSON Lines, for people and tools.

## The UI: two modes

### The stack, and why not a native GPU UI

The IDE's page is **TypeScript with SolidJS, Monaco and xterm.js**, served by the Rust
binary. It opens in a window of its own: Chrome, Edge, Chromium or Brave in app mode, as
today, or a system web view through an optional `code-arena-desktop` launcher (WebView2
on Windows, WKWebView on macOS, WebKitGTK on Linux).

I considered a native GPU UI (Zed's GPUI, egui, iced, Slint) and chose the web view.

- **"Exactly VS Code" means Monaco.** Monaco is VS Code's editor. It brings the same
  multi-cursor editing, IntelliSense widgets, diff editor, command ids, keybinding model
  and theme tokens. Rebuilding that natively would take years and would still differ in
  a thousand small ways. Zed's own editor crates are GPL-3, which an MIT product cannot
  take.
- **Code Studio needs a page.** Arena's Code Studio opens a folder's IDE in the browser
  from any computer. A native UI cannot be relayed there.
- **VS Code's ecosystem is web-shaped**: colour themes, icon themes, TextMate grammars,
  snippets, notebook HTML outputs, Markdown preview.
- **The speed comes from the design, not the toolkit.** The engine does all heavy work in
  Rust (search, index, diff, git, watching, storage), off the page's thread. The page
  renders only what is on screen, and SolidJS updates the exact DOM node that changed,
  with no virtual DOM diff per streamed token.

Why SolidJS over React: fine-grained updates suit streaming. One token changes one text
node. Long threads and trees stay cheap without memo plumbing.

TypeScript is checked by `tsc --noEmit` and linted by Biome's linter. Prettier is not
used.

### Coder mode

Coder mode is the workbench as VS Code lays it out: activity bar, side bar, editor groups,
panel, status bar, secondary side bar. It behaves as VS Code does:

- VS Code's command ids, `keybindings.json` rules (chords, `when` clauses, `-command`),
  `settings.json` layers and the Settings editor;
- colour themes, icon themes and **TextMate grammars** (vscode-textmate with oniguruma in
  a worker). Today's IDE leaves grammars unused, so this is a step past it;
- Zed-like touches where VS Code is slower: instant quick open from the Rust index, a
  project search that streams results, and a cold start measured in CI.

### Agent mode

Agent mode is a quiet place to work with the agent. On first run it shows only:

- the conversation, as one column;
- the box, with `@` for files and `/` for commands and skills;
- one status line: model, mode and the context ring.

Everything else shows up only while it is relevant, and folds away after:

- a turn's tool calls collapse into one line ("Read 4 files, ran tests, edited
  `cart.rs`"), with details one click away;
- sub-agents, the plan, a goal, a checklist and jobs each appear as a slim bar while they
  run;
- approvals are a card in the thread, never a modal.

Customizable:

- **Pin** any panel (Tasks, Plan, Context, Changes, Files, Terminal) to the right or the
  bottom. The layout is kept per person.
- **Density:** compact, comfortable or spacious.
- **Show or hide** each element. Every one is a setting, so the Settings UI and
  `settings.json` both reach it.
- Every action is in the command palette (Ctrl+Shift+P) and on `/` in the box. Nothing
  needs a button on screen.

Agent mode reaches the whole harness:

- MCP servers: Arena's, Farzan Graph's, your own and the project's;
- plugins, skills, commands and sub-agents;
- hooks;
- scheduled tasks, loops and goals;
- checklists, plans and memory;
- projects, assistants and other providers.

Each has a page in the **Harness** view, one entry in the activity bar of either mode.

Switching: **Ctrl+Alt+M** or the mode switch at the top left. Both modes are one window
and one session. Coder mode keeps the chat as a side bar view, and Agent mode can open a
file or a diff in a side editor.

### The terminal

`code-arena chat` stays a first-class interface: streaming, dimmed thinking, Tab to skip
thinking, typing while a turn runs, ↑ history shared with the IDE, Ctrl+Z, approvals,
job lines that never break into a question. It is built on `ratatui` with an inline
viewport, not the alternate screen, so the scrollback stays the terminal's.

## Two editions: managed and standalone

One product, one binary; the edition is what it is connected to, and `version`, `doctor` and the chat's banner name it.

- **Managed** (the enterprise edition): signed in to Farzan Arena. Everything: Arena's gateway and models, its MCP
  tools and Farzan Graph, chats kept on the web both ways, Arena's memory, skills and checklists, projects, assistants,
  automation. And everything of the standalone edition, the person's own providers included, where Arena's admins
  allow them (its chat config's `codeArenaProviders`; not said: allowed).
- **Standalone** (the home edition): no Arena, the person's own provider: Anthropic (its Messages API), OpenAI,
  DeepSeek, OpenRouter, Mistral, a local Ollama, or any OpenAI-compatible API (`code-arena login --provider NAME
  [--url URL]`, or the provider's key variable, `OPENAI_API_KEY` and so on). Its models are `NAME:MODEL`. Everything
  that needs no Arena: the tools, sessions, compaction, the plan, local memory files and `.remember/`, skills (the
  project's, the person's, the built-in ones), commands and sub-agents, the person's MCP servers, the sandbox, jobs.
- **What needs Arena** is marked once (`ca-cli/src/edition.rs`): in the standalone edition it is hidden from `/help`,
  its tools are not offered, and a command for it says which edition has it and how to turn it on. The settings
  editor (0.4) shows the same marks as locks.
- A provider's key is asked without showing it, or read from its variable at each run (never written then), and kept
  in `credentials.json` (its owner's only), never printed.

## Arena servers 8.0 and later

### What each server has

Both servers serve `/api/code-arena` with the person's API key. The v8.1 code
(`/home/ghadyani/arena-v8.1/src/Llm.Api`) and the live v8.0 (`/home/ghadyani/llm-service`,
branch v8.0) differ in these routes:

| route | 8.0 | 8.1 |
|---|---|---|
| `GET/POST /chats`, `GET /chats/{id}`, `POST /chats/{id}/messages`, `POST,DELETE /chats/{id}/turn`, `PUT /chats/{id}/draft`, `POST /chats/{id}/queue/take`, `POST /chats/{id}/continued` | yes | yes |
| `GET /attachments/{id}`, `GET /preferences`, `GET /config`, `POST /tokenize`, `GET /settings`, `GET /security`, `GET /me`, `POST /sign-out`, `GET /watch`, `GET /broadcasts`, `POST /broadcasts/{id}/dismiss`, `POST /feedback`, message feedback | yes | yes |
| `GET/POST /skills`, `PUT /skills/{name}`, `POST /skills/import`, `POST /skills/learn` | yes | yes |
| `/memories` (list, keep, forget, recall, import), `/history/search`, `/history/open` | yes | yes |
| `GET /checklists`, `GET/PUT/DELETE /chats/{id}/checklist`, `POST …/checklist/tick` | yes | yes |
| `POST /checklists`, `PUT/DELETE /checklists/{id}` (make, edit and delete checklists) | **no** | yes |
| `GET/PUT/DELETE /chats/{id}/goal`, `GET /assistants` | yes | yes |
| `GET/PUT/DELETE /chats/{id}/plan` | yes | yes |
| `GET /projects`, `PUT /chats/{id}/project` | yes | yes |
| `GET /chats/{id}/project-work`, `POST /chats/{id}/project-work/{function}` | **no** | yes |
| `GET /agents`, `POST /agents/{id}/run`, `GET /agents/runs/{id}`, `GET /scheduled`, `POST /scheduled/{id}/run`, `GET /scheduled/{id}/runs` | **no** | yes |
| `GET /connect` (Code Studio's relay) | yes | yes |

Code Arena also uses:

- the gateway's `/v1/models`, `/v1/model/info` and `/v1/chat/completions`;
- `/mcp` (Arena's tools);
- `code-intel.DOMAIN/mcp` (Farzan Graph);
- `/api/chat/attachments` and its file routes;
- `/api/downloads/code-arena/*` (updates and signatures);
- the project routes `/api/projects/*`.

This table is in the repository as test fixtures (`crates/ca-arena/tests/fixtures/`): each
version's answers to the probes, read from its routes. The contract tests replay them.

### How features are negotiated

- **Probe, don't read the version.** At sign-in and at each start, `ca-arena` asks
  `GET /api/code-arena/config` and `GET /api/code-arena/me`, which both versions have. It
  then probes each optional route once with a harmless request.
- **Telling "no such route" from "no such thing".** ASP.NET answers an unknown route with
  404 and an empty body. Arena's own 404 carries `{"status", "error"}`. A route that
  answers 401 or 403 exists, and the key or the role is the problem. These rules have
  tests against both servers' recorded answers.
- **The result is a `Capabilities` set**, for example `checklists.edit`, `project_work`,
  `automation.agents`, `automation.scheduled`, `tokenize`. It is cached per server
  (`servers/HOST.json` in the data folder) and probed again when the server's settings stamp
  (`GET /settings`) changes, or after an hour. `code-arena doctor` probes afresh and prints it.
  `watch` is not probed (a probe would hold the stream open): sync opens the stream and polls
  when it cannot.
- **The UI follows the set, not a version:**
  - Without `automation.*`, the Automation view shows Code Arena's own schedules only and
    says why Arena's are missing.
  - Without `checklists.edit`, checklists can be turned on and ticked, but not made.
  - Without `project_work`, the project tools are left out of the model's list.
- **A `/capabilities` route for later Arenas.** I will propose
  `GET /api/code-arena/capabilities` to Arena, answering `{"features": ["chat_sync", …]}`
  (the names `doctor` prints). When it is there,
  Code Arena reads it first and probes nothing else. Old servers keep working by probing.
- **The wire shapes** are typed from both servers' C# records. Fields new in 8.1 are
  `Option` in Rust, so an 8.0 answer parses. Contract tests replay recorded answers from
  both servers.

## The project's `.code-arena/` folder

The first time a repository is opened (IDE, `chat` or `-p`), Code Arena creates `.code-arena/`
at its root. A folder that is no repository (the Desktop, a downloads folder) gets none. It never overwrites
a file that exists, and it says in one line what it made.

```
.code-arena/
  README.md          what this folder is, what to commit, how each file works
  settings.json      the project's settings: deny rules, tool settings, hooks (comments explain each)
  mcp.json           the project's MCP servers (empty; enabled per person, never unasked)
  commands/          /NAME commands (one example, commented)
  agents/            sub-agents (one example: a read-only reviewer)
  skills/            skills (agentskills.io format)
  checklists/        the repository's checklists (one: "Before a merge")
  tools/             the project's command tools (empty)
  hooks/             hook scripts (empty; hooks are declared in settings.json)
  .gitignore         ignores local/, media/, plots/, vm/ and *.local.json
  local/             per-person state, never committed
```

`ARENA.md` stays at the root, beside `AGENTS.md`, `CLAUDE.md` and `QWEN.md`, which are
read as today. When none exists, Code Arena offers once to write a starter `ARENA.md`.
The model drafts it from the repository: how it builds and tests, and its conventions.

Compatibility inside the folder: `.arena/` (today's place), `.claude/` and `.qwen/` are
read as today. `.code-arena/` wins over them by name.

`"projectFolder": false` in the user's settings stops the folder from being created.
The folder is still read when it exists.

## Commits made by the model

`git.autoCommit` is **off** by default.

When it is on, or when you type `/commit` (or use **Commit with Code Arena** in Source
Control), commits are made like this:

1. **What may go in.** Code Arena works out the turn's own changes: files the turn's tools
   wrote, minus files that held your own uncommitted changes when the turn began, as
   today. `/commit` instead takes all of the working tree's changes that you choose.
2. **The model proposes.** It gets the diff, by hunk with ids, and the last 20 commit
   subjects so it can follow the repository's style (Conventional Commits or not). It
   answers with one tool call, `propose_commits`: a list of groups, each with a message
   (a subject under 72 characters and a body saying why) and the hunks it takes.
3. **Code Arena checks.** Every hunk is in exactly one group, and only allowed hunks
   appear. A proposal that fails is asked once more with the reason. If it fails again,
   one commit is made with the model's best summary.
4. **Code Arena commits.** Each group is staged by hunk (the index gets exactly those
   lines) and committed in order. Your git identity is the author. A
   `Co-Authored-By: Code Arena <…>` trailer and `Code-Arena-Session:` are added. Hooks
   run as for any commit. A hook that refuses stops the series and says which group
   failed.
5. **Review first** (`git.autoCommit.review`, on by default in the IDE): the groups show
   as a card. You can edit a message, move a hunk, or drop a group before **Commit**.

It never commits during a merge, rebase, cherry-pick or revert, nor while the turn's
background jobs run. Stop skips it.

## Long sessions: performance by design

### Targets

These are measured in CI on a synthetic session of **10,000 messages** (3,000 tool calls,
500 edits, 20 compactions) and a 50,000-file repository. The C# 8.1 version is measured on
the same inputs, so each number has a "before".

| what | target |
|---|---|
| open the session: server answers the first page | ≤ 30 ms |
| open the session: the thread is painted | ≤ 250 ms |
| scroll the whole thread | no frame over 16.7 ms at p95, 60 fps |
| stream an answer at 150 tokens/s | page CPU ≤ 15% of one core; engine ≤ 5% |
| engine memory, idle, with the session open | ≤ 80 MB RSS |
| page memory, with the session open | ≤ 150 MB JS heap, flat while scrolling end to end |
| switch sessions | ≤ 150 ms to paint |
| quick open across 50,000 files | first results ≤ 30 ms |
| `code-arena --version` | ≤ 15 ms |
| `-p` to the first request, network excluded | ≤ 120 ms |

### Measurement

- `bench/` holds Criterion benches for the engine: storage, paging, search, compaction
  accounting, diff and index.
- Playwright traces drive the page with a session of 10,000 messages and record long
  tasks, the heap and frame times.
- A **perf** job in CI fails when a number gets more than 10% worse than `main`.
- Each release's notes carry the table.

### Design

- **Paged history.** The page asks for the last 30 turns, then 30 more as you scroll up.
  The server reads them by index in SQLite. A turn's tool results and diffs are fetched
  only when unfolded.
- **A virtualized thread.** Only the turns on screen and a margin are in the DOM. Heights
  are measured once and kept, so the scrollbar is stable and a jump to a turn is
  instant.
- **Incremental Markdown.** An answer is split into blocks as it streams. Finished blocks
  are frozen and never parsed again, and only the open block re-renders, once per frame.
  Code blocks are highlighted in a worker.
- **Bounded memory.** Turns scrolled far away drop their DOM and their big strings, and
  keep their measured height. The engine keeps the model's window and an LRU of recent
  turns. Everything else is on disk.
- **Typed events, not reloads.** After a turn or a compaction the page gets the changed
  rows, never the whole session. Today's `reset` event, which sends the whole
  conversation again, goes away.
- **Nothing heavy on the page's thread.** Search, diff, the code index, git and file
  listings run in the engine and stream their results.
- **Cheap idle.** No polling where a push will do: file watching, Arena's `watch`,
  settings changes and LSP all push. Timers that must poll back off when the window is
  hidden.

## Settings: everything in one schema

- **Everything is in the schema.** Every setting is declared once in `ca-settings`: key,
  type, default, description, scope (user, workspace, folder, language, machine),
  whether it needs a restart, and which environment variable overrides it. This covers
  VS Code's settings that Code Arena reads, Code Arena's own, and today's `config.json`
  keys and `CODE_ARENA_*` variables.
- **The Settings editor is generated from it.**
  - It has VS Code's search, `@modified`, `@lang:`, `@id:`, and the User, Workspace and
    Folder tabs.
  - **Machine** (today's "Code Arena on this computer") holds the browser, hypervisors,
    the libvirt pool and the home folder.
  - **Harness** holds MCP, hooks, plugins, skills, permissions, the sandbox, memory and
    sync.
  - **Experimental** shows whatever is still behind a flag.
- **Nothing is hidden.** A test reads every key the code asks for and every environment
  variable it reads, and fails when one is missing from the schema. It also fails when a
  schema entry is never read. A few values are not settings: the IDE's run key and the
  mark of a start without a console. They are listed in the test, each with the reason.
- **One format.** `settings.json` is the person's settings, with comments kept on write.
  `config.json` is gone. The Arena connection (address, API key, gateway, CA) is kept apart in
  `credentials.json` (0600, in a 0700 folder), so settings can be shown and shared. The OS
  keyring comes later as an option. No key is ever written into `settings.json`.

## Build, CI/CD and releases

### Repository layout

```
Cargo.toml            workspace
crates/ca-*/          the crates above
web/                  the IDE's page (pnpm, Vite, SolidJS)
bench/                benches and perf fixtures
tests/compat/         recorded Arena 8.0 and 8.1 answers, contract tests
docker/builder/       the build image
.github/workflows/    ci.yml, perf.yml, release.yml
docs/                 plan.md, the manual, measurements
CHANGELOG.md
```

### The Docker build image

`docker/builder/Dockerfile` builds everything for Linux and Windows, from Linux:

- the pinned Rust toolchain (1.100.0), rustfmt and clippy, and the musl target, so the Linux
  binary is static and runs on any distribution;
- Node and pnpm for the page, once there is one (0.4);
- `scripts/in-builder.sh make dist` writes `dist/` with the package and `SHA256SUMS`.

Windows and macOS builds come from their own CI runners. Cross-building Windows from Linux
would need Microsoft's SDK in the image, under Microsoft's licence, which was declined. macOS
needs Apple's SDK, which cannot go in an image. Linux arm64 is built on GitHub's arm64 runner.

The image is built and pushed to `ghcr.io/binchitects/code-arena-builder` by CI. On this
host it is built through the proxy that the Docker daemon already uses.

### CI (`ci.yml`, on every push and pull request)

| job | runners |
|---|---|
| lint: `cargo fmt --check`, `clippy -D warnings`, `cargo deny`, Biome lint, `tsc` | ubuntu-24.04 |
| test: `cargo nextest`, Vitest | ubuntu-24.04 (x64), ubuntu-24.04-arm, macos-15 (arm64), macos-15-intel (x64), windows-2025 (x64), windows-11-arm (arm64) |
| e2e: Playwright against the built binary | ubuntu-24.04, windows-2025, macos-15 |
| compat: contract tests against recorded Arena 8.0 and 8.1 answers | ubuntu-24.04 |
| perf: benches and page traces, compared with `main` | ubuntu-24.04 (`perf.yml`, on `main` and on PRs labelled `perf`) |

### Releases (`release.yml`, on a `v*` tag)

1. Build the six targets, each on its own OS's runner, so each is tested where it runs:
   `linux-x64`, `linux-arm64`, `osx-arm64`, `osx-x64`, `win-x64` and `win-arm64`.
2. Package each as `code-arena-<version>-<rid>.tar.gz` (`.zip` on Windows), with
   `README.txt` and `LICENSE`, and write `SHA256SUMS`.
3. Attach them to a GitHub release made with `gh release create`. Its title is
   `Code Arena <version>` and its body is that version's section of `CHANGELOG.md`.
4. Mark 0.x versions as pre-releases. 1.0 is the first stable one.

Arena's own signing (`SHA256SUMS.sig`, Authenticode by the stack's CA) stays the Arena
installer's job. `code-arena update` checks the same signature as today.

### Versions

The crates share the workspace's version. `0.1.0` is the first tag. Minor versions
follow the milestones below, and patch versions fix them. Before 1.0 the product's own
formats may change between versions, without migration. Compatibility with Arena servers
8.0 and later is kept from 0.1 on.

## Milestones 0.1 to 1.0

Each milestone ends with a tagged release, a CHANGELOG section, binaries for all six
systems, and a **headline**: one thing to show.

| version | headline | contents |
|---|---|---|
| **0.1** | *"It answers, on Arena 8.0 and 8.1."* `code-arena -p "…"` reads, edits and runs commands in a repository, on both servers, from one static binary on all six systems | workspace and crates' skeleton; CI on six runners; release workflow; build image; `login` (key and password, CA), `logout`, `models`, `doctor` (capabilities); gateway streaming with tool calls and mending; local file, grep, glob, shell and git tools; modes and approvals in `-p`; settings schema v0; `.code-arena/` creation |
| **0.2** | *"A terminal agent you'd switch to."* `code-arena chat` with sessions, resume, compaction and MCP | TUI (line editor, history, Tab skips thinking, typed-while-running, Ctrl+Z); SQLite sessions; `--continue` and `--resume`; rewind and checkpoints; compaction and context accounting; big results kept aside; result condensing; Arena MCP, Farzan Graph and own MCP servers; tools on demand; ARENA.md and instruction files; commands, skills and sub-agent files; kept rules; sandbox; jobs (no time limit, background) |
| **0.3** | *"Same chat, web and terminal."* A session continued on the web and back again | chat sync both ways (ids, branches, compaction, drafts, queue, turn lock, attachments); memory (Arena's and local); `.remember/`; skills from Arena and import/export; checklists; plans; goals; projects; assistants; capability negotiation complete with contract tests; and what 0.2 left (the checklist says each): other providers, Tab to skip thinking, `stream-json`, the Arena account's compaction threshold and its tokenizer, git's filters kept from running, a long turn compacting itself |
| **0.4** | *"Agent mode."* The IDE's minimal agent window, smooth on a 10,000-message session | the server (key, cookie, checks); SolidJS shell; Agent mode (thread, box, status line, pins, density); approvals; queued messages; Answer again; Edit; Rewind to here; Context panel; Tasks panel; Settings editor generated from the schema (the editions' locks shown); perf CI with the first numbers against C# 8.1; and what 0.3 left (the checklist says each): a cut tool call redone in parts, files held open on Windows, llama.cpp's cache asked again, the engine's own window, assistants' tool groups, making a project and its views |
| **0.5** | *"Coder mode."* VS Code's workbench, keys and settings | Monaco; explorer; tabs and previews; editor groups; quick open and palette; search and replace; terminals (pty); Source Control (stage by hunk, branches, sync); compare; quick diff; themes (Arena's and VS Code's); keybindings.json and the Keyboard Shortcuts editor; settings.json layers; hot exit; breadcrumbs; outline; zen mode |
| **0.6** | *"Language and debug."* Go to definition, breakpoints and formatting in five languages | LSP relay and the catalog; DAP relay and adapters; launch.json and tasks.json; formatters; Problems (check, LSP, security review); notebooks; merge conflicts and the merge editor; multi-root workspaces; TextMate grammars |
| **0.7** | *"The whole harness."* Hooks, plugins, sub-agents, goals, loops and schedules from one Harness view | hooks (below); plugins (below); parallel steps in worktrees; goal mode and queued goals; loops; schedules (OS timers); Arena agents and scheduled tasks; code index on tree-sitter; `run_code` (code mode); Laya; other providers; your own subscriptions (`ask_agent`); model-made commits |
| **0.8** | *"Everything the old one did."* The parity checklist is complete | extensions (themes, icon themes, snippets, languages, keybindings, `configurationDefaults` from VSIX and from VS Code); snippets; review view; code completion; screenshots; VMs; Code Studio relay (`--connect`) and ports; update, install scripts, desktop entries ("Open in Code Arena"), Windows launcher; accounts menu, feedback, broadcasts, notifications; accessibility settings; files in answers; RTL |
| **0.9** | *"Measured and hardened."* Release candidate | every perf target met and published; fuzzing of the parsers (JSONC, tool-call mending, the URI rewriter, path safety); security review of the server and the sandbox; a ten-run proof of each flaky test fixed; the manual written for the Rust product; an importer for the C# version's sessions and config (one-shot, optional) |
| **1.0** | *"Stable."* | formats frozen from here on, with migrations; the support policy for Arena servers stated; the parity checklist re-checked by hand on the six systems |

### New harness features

These go past today's Code Arena, with Claude Code, Codex and OpenCode as the bar.

- **Hooks**: `SessionStart`, `UserPromptSubmit`, `PreToolUse` (allow, deny or ask, and
  rewrite the arguments), `PostToolUse`, `Notification`, `Stop`, `SubagentStop`,
  `PreCompact` and `PreCommit`. Each runs a command that gets JSON on stdin and answers
  JSON. They are declared in `settings.json`; Claude Code's hook format is read as it is.
  A project's hooks run only after the person enables them, as its tools do today.
- **Plugins**: a folder or git URL bundling commands, agents, skills, hooks, MCP servers
  and command tools, with a manifest. Claude Code's plugin layout is read as it is. They
  can be installed from Arena's plugins (Admin → Plugins), a git URL or a folder, and
  enabled per person and per project.
- **Sub-agent definitions** with their own model, tools and permission mode. The Tasks
  panel shows each one live.
- **Output styles** and **custom status lines**: the Agent mode status line can run a
  command, as Claude Code's does.
- **Session forks**: branch a session at any message into a new one.
- **Headless JSON streaming**: `-p --output stream-json`, for CI and other tools.

## Parity checklist

Every feature of today's manual (`docs/code-arena.md` in Arena 8.1), with the milestone
that brings it. "Better" says what the rewrite changes. Items are ticked as they land.

### Getting it and signing in

- [x] 0.1 Builds for linux-x64, linux-arm64, osx-arm64, osx-x64, win-x64, win-arm64
- [ ] 0.8 `install.sh` and `install.ps1` from Arena, checked against SHA-256 and the signed list (`SHA256SUMS.sig`, code-signing certificate)
- [ ] 0.8 Windows: `code-arena-ide` launcher with no console, desktop and Start menu shortcuts, Authenticode-friendly (version info, icon)
- [ ] 0.8 "Open in Code Arena" on folders: Explorer, GNOME Files, Dolphin, Nemo, Caja, Thunar, Finder Quick Actions; `uninstall` removes them
- [ ] 0.8 `update`: daily check, the status bar's Update, size, SHA-256 and signature checked, old binary moved aside on Windows; `updateCheck` setting
- [x] 0.1 User-Agent `code-arena/X.Y.Z` on every request
- [ ] 0.8 Packages with README.txt, LICENSE and SHA256SUMS
- [x] 0.1 `login --url`, key typed without echo, checked with `/v1/models`, MCP counted; `logout`; `models` with windows and longest answers
- [x] 0.1 Sign-in by user name and password (lockout, second factor), key made as "Code Arena on NAME"
- [ ] 0.8 Accounts menu: whose key, sign in or out (revoking), Your account, People, Send feedback, signed-out dot
- [x] 0.1 Paths per system; `CODE_ARENA_HOME` (portable); `ARENA_URL`, `ARENA_API_KEY`, `ARENA_GATEWAY_URL`, `ARENA_MCP_URL`, `ARENA_MODEL`; `--gateway`
- [x] 0.3 Without an Arena: other providers, the standalone edition, what needs Arena locked there (the IDE without a model: 0.4)
- [x] 0.1 Company CA: `--ca`, `ARENA_CA_CERT`, the saved path, `SSL_CERT_FILE`; never a switch to turn checks off

### Working with it (CLI)

- [ ] 0.4 `code-arena` opens the IDE; `web`; `--port`, `--no-open`
- [x] 0.2 `chat`, a prompt argument, `-p` (stdin with `-`), `--output json`, `--continue`, `--resume [id]`, `--version`
- [x] 0.3 `--output stream-json`, `--plan`, `--web N`
- [x] 0.2 Streaming with dimmed thinking; `/thinking off`
- [x] 0.3 Tab skips thinking
- [x] 0.2 Tool lines with results (a diff for an edit, the first line otherwise); tokens and cache share per turn; the window's use; room kept for the answer
- [x] 0.2 Answers cut at the output limit carried on (eight times a turn at most)
- [ ] 0.4 A cut tool call redone in parts; cuts that repeat the same words end the turn (moved from 0.3)
- [x] 0.2 Ctrl+C semantics; typing while a turn runs (read at the next step, or after the turn); a question reads only what is typed after it
- [x] 0.3 ↑ takes back a pending line; commands typed meanwhile run when the turn ends
- [x] 0.2 Ctrl+Z and `fg`; a trailing `\` continues the line; the line editor's keys; history per folder shared with the IDE (1,000 kept); multi-line editing; plain lines without a TTY
- [ ] 0.2 Every slash command in the manual's table: `/help /model /mode /thinking /tools /plan /checklist /review /agent /assistant /tasks /goal /compact /compact-at /context /cost /mcp /jobs /clear /resume /web /sync /project /projects /memory /remember /rewind /search /index /permissions /sandbox /commands /skills !cmd /NAME /exit` (each lands with its feature's milestone) (each lands with its feature; 0.2 has `/help /model /mode /thinking /tools /compact /compact-at /context /cost /mcp /jobs /clear /resume /rewind /search /permissions /sandbox /commands /skills !cmd /NAME /exit`)

### The IDE: window, security, layout

- [ ] 0.4 Its own window (Chrome, Edge, Chromium, Brave in app mode; its own profile); browser choice and version check; a tab as the fallback; plus the system web view launcher (better)
- [ ] 0.4 The browsers supported (Chrome/Edge 109+, Firefox 115+, Safari 16+) and the "too old" page
- [ ] 0.4 The log: `code-arena.log`, rotated at 5 MB, three kept, path in `--help` and About
- [ ] 0.4 Only this machine: 127.0.0.1, one-run key with an HttpOnly SameSite=Strict cookie, launcher file (0600), Host and Origin and Sec-Fetch checks, JSON only, no framing, paths relative only, links resolved one part at a time, pipes and devices refused
- [ ] 0.8 Code Studio (`web --connect`): the relay over WebSocket, reconnect with backoff, the folder's id kept
- [ ] 0.5 Activity bar: views, counts, drag to reorder or to the panel, hide and show, position (side, top, hidden), reset
- [ ] 0.5 Side bar left or right; panel bottom, left or right; sizes and visibility kept across runs and ports
- [ ] 0.8 Help over the page, with `?` per panel and a link to the manual on the Arena
- [ ] 0.5 Open Folder (VS Code's simple dialog), Open Recent, restart on the new folder at the same address
- [ ] 0.6 Add Folder to Workspace; `.code-workspace` files (folders, names, settings layer, save as, close, open its configuration)
- [ ] 0.5 Explorer: new, rename, delete, multi-select, cut/copy/paste, drag to move or copy, drop from the computer (256 MB), Copy Path and Relative Path, Find in Folder, Open in Terminal, auto-reveal, decorations (agent's A/M, git's, ignored dimmed, links, problems), Compare Selected
- [ ] 0.5 Hot exit and restored tabs (pinned, previews, cursors); backups 0600, 5 MB a file, 50 MB a folder
- [ ] 0.5 Outline view (better: tree-sitter for every grammar, not four scanners)
- [ ] 0.5 Editor: language by name, `files.associations`, first line; previews; pin; middle click; drag to reorder; breadcrumbs with folder and symbol pickers; picture, sound and video tabs; Markdown preview that follows; save, save all, changed on disk asks; close asks; binary or over 5 MB refused; a line when code-arena stops answering
- [ ] 0.4 The agent's edits reload open files by changed lines only (cursor and breakpoints kept)
- [ ] 0.5 Compare: agent's changes, git's (unstaged, staged, commits, file history, revisions), saved, clipboard, another file, two files; stage, unstage and revert by change; accept and revert agent changes in part; Your changes list
- [ ] 0.5 Editor groups (up to 8): split, move, focus by number, orthogonal split, join, layout toggle, kept
- [ ] 0.5 Revert File, Zen Mode, Full Screen, minimap, sticky scroll, whitespace toggles
- [ ] 0.8 A tab or the chat in a window of its own, and back
- [ ] 0.5 Changes on disk show at once (watching; re-read on focus where watching is limited)
- [ ] 0.5 Search and replace across files (`$1`, line endings kept, unsaved edits respected)
- [ ] 0.5 Quick diff in the gutter, overview and minimap (`scm.diffDecorations`), next and previous change
- [ ] 0.5 Quick open (`:line`, `#symbol` with initials, `@`, `>`, `?`), Go to Symbol in Workspace from the index
- [ ] 0.5 Command palette with recent first, category and keys
- [ ] 0.5 Terminals: tabs, links to files in output, reattach after reload with 256 KB of screen, Debug Console and Output tabs
- [x] 0.3 Settings from Arena applied live (the stamp polled every 30 s; the page told by push: 0.4)
- [ ] 0.4 Chat: model and thinking pickers as Arena's chat names them; Skip thinking; files attached (10, Arena's limits, uploaded through code-arena); Answer again (as was, shorter, longer, other model, other thinking, files as they were); Edit; Rewind to here (conversation, code, both) with its preview
- [ ] 0.4 Tool cards with diffs; "N files changed"; approval card (Allow, Always for this session, Deny); jobs above the box with Stop; ↑ history; queued messages (edit, cancel, clear) marked "Sent while it worked"
- [ ] 0.4 Media from Arena's tools under `.code-arena/media/`, shown in place
- [ ] 0.4 Compaction leaves the thread whole, marked, with Show summary
- [ ] 0.4 The context ring and the Context panel (by kind, biggest, freeable, Turn off, Compact now)
- [ ] 0.4 Paged thread (30 turns, Show earlier); better: virtualized, no `reset` reloads, measured
- [ ] 0.5 Status bar: branch, project, agent changes, terminals, jobs, chord wait, cursor, indentation (detect, convert), EOL, formatter, language server, language, MCP servers, context, mode, model, version and About

### Settings, keys and themes

- [ ] 0.5 settings.json (user, folder, language-specific, VS Code's language id mapping), comments and trailing commas, schema checked
- [ ] 0.5 Editor, saving (autoSave, trim, final newline, eol), diffEditor.*, terminal settings, themes, icon theme, completion setting (user-only)
- [ ] 0.5 keybindings.json: VS Code's rules, chords, `when` clauses, `-command`, editor commands; rules for unknown commands listed
- [ ] 0.4 Settings editor: categories, search, `@modified`, `@lang:`, `@id:`, User and Workspace (and Folder), comments kept, reset, copy id or JSON, unused settings listed, **From VS Code** import; Machine section (browser, hypervisors, pool, home) with env overrides shown
- [ ] 0.5 Keyboard Shortcuts editor: record keys, conflicts counted, written as VS Code writes
- [ ] 0.5 Arena's themes (Code Arena, Sakura, Midnight, Copper, Forest, Ocean, Lavender, light and dark), WCAG AA checks, terminal minimum contrast
- [ ] 0.5 VS Code's popular themes by their names and aliases, High Contrast
- [ ] 0.5 Accessibility: zoom, auto high contrast, reduce motion, more contrast, text size, focus rings, colour-blind colours, palette toggles
- [ ] 0.5 Keys that skip the terminal (`commandsToSkipShell`, chords); macOS ⌘; browser-reserved keys remapped; F5, Ctrl+S never reach the browser
- [ ] 0.5 Everything embedded (Monaco, workers, xterm, fonts); lazy loading

### Formatting, Run and Debug, Extensions, Snippets, Notebooks, LSP

- [ ] 0.6 Format document and selection, Format With, default formatter (VS Code ids), format on save (timeout), Save without Formatting, format on paste, `prettier.*` as flags
- [ ] 0.6 Formatters found by files: Prettier (project, then global), ruff and black, gofmt, rustfmt (edition), clang-format, dotnet format; trust rules; sandboxed; four at once; `formatters` in settings
- [ ] 0.6 Run and Debug view: variables, watch, call stack, breakpoints (conditional, logpoints, run to cursor), toolbar, Debug Console with evaluation, inline values, hollow and grey breakpoints kept per folder
- [ ] 0.6 launch.json (system parts, envFile, pre and post tasks, inputs, compounds, variables, `pickProcess`), detected configurations (.NET, npm, Python, Go, Cargo, CMake, Make), Run without debugging
- [ ] 0.6 tasks.json (shell, process, npm, typescript; groups; dependsOn; background; problem matchers), detected tasks; Debug or Release choice
- [ ] 0.6 Trust gates tasks, launch configurations and debug sessions
- [ ] 0.6 Adapters: gdb, lldb-dap, netcoredbg, debugpy, Delve, js-debug; `debugAdapters` setting; nothing downloaded; eight sessions
- [ ] 0.8 Extensions view: installed, recommended (with status), VS Code's on this computer (import), Install from VSIX; limits and safety
- [ ] 0.8 Contributions used: themes, icon themes, snippets, languages, keybindings, configurationDefaults; better: **grammars used** (0.6)
- [ ] 0.6 Snippets: project, user, extension; scope, include and exclude; Configure, Insert, Surround With
- [ ] 0.6 Notebooks: cells, run, run all, interrupt, restart, outputs (rich, sandboxed HTML), Jupyter keys, kernel discovery, trust, written as Jupyter writes
- [ ] 0.6 LSP: completions, hovers, signature help, definitions (and peek), references, highlights, symbols, rename across files, code actions, formatting, diagnostics; editor's own services turned off while a server runs
- [ ] 0.6 The server catalog, lookup order, `languageServers` setting, trust, sandboxed and offline, status and Output channel, restarts, six at once, takeover

### Agent from the editor; source control; problems; review

- [ ] 0.4 `@` files (best first, whole paths outside, granted for the run), the dashed chip, Add to chat (Ctrl+L), Explain, Fix and Complete this, Fix with Code Arena in the light bulb
- [ ] 0.4 Right-to-left text in the chat
- [ ] 0.4 Reply to part of an answer: Reply, Ask about this, Copy; `codeArena.chat.quoteSelection`; quote shown both ways
- [ ] 0.5 Source Control: staged, changes, merge changes, commit (amend, push, sync), sync, publish, pull, push, fetch, checkout, branches, undo last commit, autofetch, list or tree, kept folds
- [ ] 0.6 Several repositories (`git.autoRepositoryDetection`, scan depth, ignored folders)
- [ ] 0.6 Merge conflicts: decorations, CodeLens actions, palette commands, the 3-way merge editor, Complete Merge
- [ ] 0.6 Problems: the check command (detected), parsed formats, LSP's and the editor's problems, filters, copy, quick fix; Arena's security review as problems with Fix
- [ ] 0.8 Review view and `/review`: read-only reviewer, refute before reporting, verdict and findings, Apply fix, Ask the agent, Dismiss
- [ ] 0.8 Code completion (FIM templates, waits while the agent works, rate-limit rest, `completionModel`)
- [ ] 0.4 Files in answers: `path:line` forms, chips on file tools, paths in outputs, named code blocks
- [ ] 0.5 Terminals: pty on all systems, shell choice, environment without the key, 10 at once, 256 KB kept, process-group cleanup
- [ ] 0.8 Ports view in Code Studio

### Modes, safety and tools

- [x] 0.1 Modes ask, auto-edit, plan, yolo; y, n, a; tools stay in allowed paths (links resolved); API key never in a child's environment
- [ ] 0.7 Laya command check (parts, probabilities shown, 60% asks even in yolo, `-p` refuses flagged, never removes a question)
- [x] 0.1 `read_file` (ranges, a big file's first lines), `write_file`, `edit_file` (line endings kept), `list_dir`, `glob`, `grep` (no backtracking, limits)
- [ ] 0.7 `read_file`'s outline of a big file and reading by symbol (with the code index)
- [x] 0.1 `run_shell` (bash, Git bash, PowerShell, cmd; limits; middle cut with error lines listed; `read_result`)
- [x] 0.2 `command_output`, `stop_command` (always asks), jobs with no time limit and in the background, carry-on turns
- [x] 0.1 `git` read-only: no hooks, fsmonitor, external diff, text conversion, pager, network or outside paths
- [x] 0.3 `git`: the repository's clean and smudge filters and merge drivers kept from running too
- [ ] 0.8 `screenshot` (headless browser, or the whole screen)
- [ ] 0.7 `ask_agent` and `/agent` (Claude Code, Codex, Gemini CLI by their own sign-ins)
- [x] 0.1 Read-only calls at once, changing calls in order
- [x] 0.1 Encodings (UTF-8, UTF-16, UTF-32 by BOM); Git bash paths on Windows
- [ ] 0.4 Files held open by others on Windows (moved from 0.3)
- [x] 0.2 Arena's tools over MCP: names, `arena_` prefix, instructions passed on, media saved under `.code-arena/media/` (git-ignored), deadlines, progress, cancellation, keep-alive
- [x] 0.2 Farzan Graph's tools (dedup with Arena's), background connection, retry with backoff, `/mcp`, address validation
- [x] 0.2 Per-session tool choice (`/tools on|off`); tools on demand (`load_tools`, `tools.onDemandChars`)
- [ ] 0.7 Code index: `code_search`, `code_symbol`, `code_outline`; secrets never read; CLI `index` and `search` (better: tree-sitter for many languages)
- [ ] 0.8 VMs: list, power, snapshot, copy, run, screenshot, input, test, create, clone, delete on four hypervisors; guest sign-in from the keyring; CLI `vm …`
- [x] 0.2 ARENA.md and AGENTS.md, CLAUDE.md, QWEN.md (person's, root, folder), 20,000 characters each, environment facts

### Checklists, plan, sub-agents, goals, automation

- [x] 0.3 Checklists from Arena: list, turn on, `checklist_check`, tick and untick; make, edit and delete (on servers that have it); the repository's `.code-arena/checklists/`
- [x] 0.3 The plan tool: steps with checks, files, symbols, waits, notes, status; checks enforced; checkpoints and `/plan undo N`; plans outlive sessions (`/plan resume`, `--plan`); synced to Arena's Plans
- [ ] 0.7 Steps at once (`run_steps`): read-only sub-agents and workers in git worktrees, merged with `git apply --3way`
- [x] 0.3 Reviewing a proposed plan: impact from Farzan Graph, second reading, approve, approve asking, change step, another way, reject
- [ ] 0.7 Sub-agents (`task`): reading tools by default, named tools, never ask, at once up to Arena's limit, as an agent, an assistant or a model; Tasks panel with live work; your own background tasks
- [ ] 0.7 Goal mode: check by command or condition, `goal_check`, budgets, pause, resume, cancel, the bar; several goals queued; until met
- [ ] 0.7 Loops (`/loop`)
- [ ] 0.7 Schedules (`schedule add/list/run/off/remove/install/due`, OS timers); Arena's agents and scheduled tasks (on servers that have them)
- [x] 0.3 Assistants (instructions, model, thinking, skills; their tool groups: 0.4; for sub-agents: 0.7)
- [ ] 0.7 Code mode (`run_code`): kernel, plots, `import arena` calling tools, checks per call, tool mode direct, code or both

### Sessions, window, memory

- [x] 0.2 Sessions stored durably, listed fast, resumed with the count of messages and of those summarized
- [x] 0.2 Window from `/v1/model/info`, threshold and target, structured summary (the plan restated comes with the plan, 0.3)
- [x] 0.2 Big results kept aside; used results summarized; calls mended; results cleared before compacting; `session_search` and `session_read`; `search_sessions`; `search_history` and `open_chat`
- [x] 0.2 `/context` by kind, with the biggest messages
- [x] 0.3 `/context` counted with Arena's tokenizer when it has one
- [x] 0.2 Never stuck on the window: summary failures and loops, refusals of the window (compact and ask again), measured token size
- [x] 0.3 One long turn compacting its own steps
- [ ] 0.4 llama.cpp's shared cache asked again after two seconds; the engine's own window learned (moved from 0.3)
- [x] 0.2 `/compact-at` for the session, `agent.compactAt` and `agent.compactTarget` in settings
- [x] 0.3 `/compact-at` kept in settings; the Arena account's threshold
- [x] 0.3 Memory: Arena's (`remember`, `recall`, per project by origin, person, company), local fallback and import, denied by `feature.memory`
- [x] 0.3 `.remember/` long project memory (`remember_project`)
- [x] 0.3 `skill_save` (project or person), reminder after long turns
- [ ] 0.4 Memory view
- [x] 0.2 `@files` and ranges; checkpoints and `/rewind` (code, chat, both)
- [ ] 0.7 Rewind committed when commits are on (with the commits)

### Commands, skills, rules, sandbox

- [x] 0.2 Commands (`$ARGUMENTS`, `$1`, front matter), sub-agent files, skills (agentskills.io), read again each turn; from `.code-arena/`, `.arena/`, `.claude/`, `.qwen/`, the home and the config folder
- [ ] 0.4 Skills view (by source, open or use, New Skill, Refresh)
- [x] 0.3 Skills from Arena and plugins (cached, refreshed); built-in skills
- [x] 0.3 Import and export of skills (zip, folder, SKILL.md; conflicts; secrets never come in; Arena's safeguards; limits)
- [x] 0.2 Kept rules (`allow`, `deny`, patterns, commands matched by parts; the project may only deny)
- [x] 0.2 Sandbox (bubblewrap, sandbox-exec; secrets hidden, writes limited, Docker sockets out of reach, network off); better: a Windows sandbox (0.9)

### Arena chats, projects, providers, commits, MCP, own tools

- [x] 0.3 Chats in Arena both ways: polling with the stamp (better: `watch` push where the server has it), the answer being written shown on the web, nothing lost on crash, ids on both sides, branches, compaction both ways, turn lock and the web's queue, `/web`, `/sync`, `/sync all`, background send of what Arena lacks
- [x] 0.3 Projects: choose, list, tags on gateway requests, goals and tasks tools (on servers that have `project_work`)
- [ ] 0.4 Projects: make one; the Projects view and the project tab (Overview, Goals and tasks, In it, People, Settings), export (moved from 0.3)
- [x] 0.3 Other providers: Anthropic, OpenAI, DeepSeek, OpenRouter, Mistral, Ollama, OpenAI-compatible; keys from the environment or `credentials.json`; Arena's admin switch (tested against fakes; the system keyring: 0.8)
- [ ] 0.7 Commits: **off by default**, made by the model in groups (see above), co-author trailer, only the turn's files, hooks, never mid-merge
- [ ] 0.4 Terminal bell; the IDE's bubbles; desktop notifications; broadcasts
- [x] 0.2 The person's MCP servers (stdio and HTTP, `${NAME}` from the environment, `trust`, `disabled`, plan mode read-only)
- [ ] 0.7 Own command tools (JSON, arguments on stdin and in the environment, kinds, timeouts); the project's tools and MCP servers enabled per person
- [ ] 0.7 Tool settings: on or off, how it asks, description, fixed or default arguments, per project; the project may only turn off or ask
- [x] 0.1 Recovery from the gateway: retries, stalls, a dropped stream kept, the model's line full (waited in, 0.2), safeguard refusals in Arena's words

## Rules this work follows

- The plan comes first, and you see it before any code is written.
- I work step by step myself, with no sub-agents or parallel agents. Each step is
  committed and pushed to `Binchitects/code-arena`. Each commit ends with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- No Prettier.
- No backward compatibility inside the product before 1.0. Compatibility with Arena
  servers from 8.0 on is required.
- A flaky test is fixed at its cause, never with retries or sleeps, and the fix is proven
  with ten consecutive runs.
- Every release gets `gh release create` with a title and its CHANGELOG section.
  `CHANGELOG.md` is kept from the first commit of code.
- Downloads need your yes, with their size, first. No Rust toolchain is installed here,
  so building happens in Docker. I will ask before pulling the Rust image (about 1.5 GB
  for `rust:1-bookworm`), and before crates and npm packages are fetched through
  `http://172.10.0.138:10808`.
- No secrets in samples, docs or logs; hints at how to make them instead.
  `~/.qwen/settings.json` is never read. No Docker socket in any web app.
- The C# checkouts (`arena-v8.1`, `llm-service`, `arena-v9.0`) are read, never changed.
- Heavy runs, such as builds and the perf suite, run few at a time so the host's CPU and
  GPU stay moderately loaded.

## Open questions

These are for you to decide. I have assumed the answer in brackets.

1. **The UI stack.** [A web view with SolidJS, Monaco and xterm.js, in Chromium's app
   mode or the system web view; not a native GPU UI. The reasons are above.]
2. **Session storage in SQLite** instead of JSON Lines. [Yes, with `session export` to
   JSON Lines.]
3. **`config.json` folded into `settings.json`, with the key in the OS keyring.** [Yes.]
4. **The capabilities route.** Should I propose `GET /api/code-arena/capabilities` to the
   Arena repository as a separate change? [Not now; probing covers 8.0 and 8.1.]
5. **The build image registry.** [`ghcr.io/binchitects/code-arena-builder`.]
6. **Where steps land.** [Each step is pushed to a branch and merged into `main` by fast
   forward once you've seen it; or straight to `main`, if you prefer.]
