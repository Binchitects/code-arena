# CHANGELOG

One `##` per release, newest first. Under it, only the sections that release has. Each release's
full notes are also in its annotated tag (`git tag -n99 v0.5.0`) and on its GitHub release.

Before 1.0 the product's own formats may change between versions; compatibility with Farzan Arena
servers from 8.0 on is kept throughout.

Sections used:

- `:rocket:` **Epics and highlights**: the one or two things worth reading
- `:sparkles:` **New features & Enhancements**
- `:bug:` **Bugs fixed**
- `:boom:` **Breaking changes & Deprecations**
- `:arrow_up:` **Deps updates**

## Unreleased (v0.6.0)

### :sparkles: New features & Enhancements

- **Colour themes in VS Code's own format.** Nine built in: Dark Modern, Light Modern, Dark+,
  Light+, Dark and Light High Contrast, Monokai, Solarized Dark and Light. Yours from `themes/` in
  the config folder, `~/.code-arena/themes` or the project's `.code-arena/themes`: a theme's JSON
  (`name`, `type`, `colors`, `tokenColors`, comments allowed, `include` followed) or a VS Code theme
  extension's folder (its `package.json` names its themes). A theme colours the whole page, the
  editor (its token colours too) and the terminals.
- **The Color Theme picker.** The Theme button, Ctrl+K Ctrl+T, or Preferences: Color Theme in the
  palette: ↑ and ↓ show each theme as it would be, Enter keeps it (`workbench.colorTheme` in your
  settings.json), Esc goes back. By default it follows the system: Dark Modern or Light Modern.
- **A setting changed from the IDE is checked as the schema says** (its kind, where it may be set,
  the edition's locks) and kept in your settings.json or the project's, its comments kept.

## v0.5.0 (2026-11-26)

The IDE. `code-arena ide` opens the session in a window of its own, with two faces: Agent mode, a
quiet conversation with the agent, and Coder mode, VS Code's editor with the project's files,
search, the agent's changes as diffs, terminals and the agent beside them. Releases now carry
binaries for Linux (x64 and arm64) and Windows, and are GitHub's Latest.

### :rocket: Epics and highlights

- **`code-arena ide`: the session in a window of its own.** Chrome, Edge, Chromium or Brave in app
  mode with a profile of its own, else the default browser (`--no-open` prints the address);
  `--continue` and `--resume` take up a session. **Agent mode** is the conversation in one column:
  the answer streamed as it is written, a turn's tool calls in one line ("Ran 2 commands, edited
  cart.rs") that opens on their results, the agent's questions as cards in the thread (Allow,
  Always for this session, Deny), messages sent while it works marked so, Stop (or Esc), and one
  status line: model, thinking and mode pickers, the context ring, the folder, the last turn's time
  and tokens.
- **Coder mode: VS Code's editor around the agent.** The project's files in a tree, files in tabs
  of Monaco (VS Code's own editor) with Find and Replace, folding, Go to Line, comments, hover,
  sticky scroll and multiple cursors; search across the files git sees; the agent's changes in
  this run, each a diff of before and now; Ctrl+P to find a file; terminals (xterm.js on your
  shell) in a panel; the agent beside them all. It loads only when first opened, so Agent mode's
  page stays small (41 KB).
- **Binaries for more systems on every release.** Linux x64 and arm64 (static), and Windows x64,
  built in the build image; macOS follows when GitHub Actions runs again for the organization.

### :sparkles: New features & Enhancements

- **The Context panel.** The ring in the status line opens what fills the next request by kind,
  its biggest messages, and Compact now.
- **Edit, Answer again, Rewind to here.** On each of your messages: Edit goes back to before it and
  puts it in the box to change; Answer again asks it again; Rewind to here goes back to before it,
  the conversation, the code, or both. A question the turn no longer waits on says so.
- **The plan, the goal and the jobs as bars above the box.** The plan's title and how many steps
  are done (it opens on the steps and their marks), the goal being worked on and how many wait,
  each job still running with Stop. They change as the turn goes.
- **An outline of the file shown.** For Rust, TypeScript and JavaScript, Python, Go, Java, C#, C and
  C++, Kotlin, Swift and Markdown: the Outline view (the symbol at the cursor marked), Go to Symbol
  (Ctrl+Shift+O) and the breadcrumbs. It reads by pattern, line by line.
- **The command palette (Ctrl+Shift+P).** Saving, going to a file, a symbol or a line, finding,
  folding, the views, the terminal (Ctrl+`), minimap and word wrap, and the agent's mode, context,
  compaction and Stop; the ones used last come first.
- **Search across files (Ctrl+Shift+F).** As typed or as a regular expression, any case or
  matching it, in the files git sees; a result opens at its line.
- **What Coder mode shows is watched.** An open file changed from outside shows at once (one with
  changes of yours asks first: Take the disk's, Keep mine; one gone says so), an open folder lists
  a file made or removed in it, and a file the agent edits takes the change by its changed lines
  only (the cursor stays). Only what is on screen is looked at, once a second.
- **Coder mode as you left it.** Per folder: the tabs and the one shown, each file's cursor, the
  open folders, the side bar's view, the terminal panel and the panes' sizes (drag the dividers).
  Changes not saved are kept aside a second after you stop typing (readable by you only, 5 MB a
  file, 50 MB a project) and come back with their tab.
- **Terminals that outlive the page.** A reload, or going to Agent mode and back, takes up the same
  terminals and draws their last 256 KB again; one closes with its tab's × or when its shell ends.
- **Coder mode's status line.** The git branch, the cursor's line and column, indentation, the
  encoding, the line ends and the language.
- **The IDE's server is for this machine only.** 127.0.0.1; a key for the run in the address it
  prints (kept in `ide.json`, readable by you only) that the first visit turns into an HttpOnly
  SameSite=Strict cookie; the Host, the Origin and Sec-Fetch-Site checked; no page may frame it.
  The editor opens only plain text files inside the project, up to 5 MB: a link leading out, a
  pipe, a device or a binary file is refused.
- **`compat.agents`: other agents' files, when you name the agents.** For an agent named NAME: its
  `.NAME/` folders in the project and the home (commands, skills, sub-agents, output styles, its
  `settings.json`'s hooks and status line), its `NAME.md` instructions, its plugins' `.NAME-plugin/`
  manifests and the variables `${NAME_PLUGIN_ROOT}` and `NAME_PROJECT_DIR`. None by default.
- **The build image builds the page, Linux arm64 and Windows.** Node 22 and npm for `web/` (`make
  web` builds and tests it, and every binary carries it), and the targets and C compilers for
  Linux on arm64 (static, musl) and Windows on x64 (MinGW).

### :bug: Bugs fixed

- **A setting written over several lines broke its file when changed.** Code Arena replaced only
  its first line; a value's brackets are now counted to its end, and a setting taken out takes the
  comma before it when it was the last.

### :boom: Breaking changes & Deprecations

- **Other agents' files are read only when `compat.agents` names the agent.** Plugins keep their
  manifest in `.code-arena-plugin/plugin.json` (or `plugin.json`) and name their folder
  `${CODE_ARENA_PLUGIN_ROOT}`.
- **The sandbox hides every `~/.*.json` file from commands**, where other tools keep their state
  and keys, instead of a list of names.

### :arrow_up: Deps updates

- **The IDE's page:** Node 22.23.5 (build only), solid-js 1.9.20, vite 8.3.8 with vite-plugin-solid
  2.11.17, TypeScript 7.0.2, monaco-editor 0.58.0, @xterm/xterm 6.0.0 with @xterm/addon-fit
  0.11.0.
- **The IDE's server:** tokio-tungstenite 0.26 (its stream); hyper's server side, already in.

## v0.4.0 (2026-11-26)

The whole terminal harness: hooks, sub-agents, goals, model-made commits, steps at once in
worktrees, loops and schedules, plugins, forks, output styles and a status line, Arena's agents and
scheduled tasks; and the engine's own window learned when it refuses a request.

### :rocket: Epics and highlights

- **Hooks, sub-agents and goals.** Hooks run at the agent's moments (`SessionStart`,
  `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Notification`, `Stop`, `SubagentStop`,
  `PreCompact`, `PreCommit`): each gets the moment as JSON and blocks (exit code 2), allows,
  rewrites the arguments or sends the model on; a project's run only once you trust them. The
  `task` tool hands work to a sub-agent (`agents/NAME.md`, with its instructions, model and tools),
  four at once. Goal mode (`/goal TEXT`, `-p --goal`) works turn after turn until a check passes
  (`--check CMD` or `--when CONDITION`), within a budget.
- **Commits the model groups and writes.** `/commit` proposes commits of the working tree's changes
  by hunk, in the repository's own style (a subject under 72 characters and a body that says why);
  `/commit yes` makes them, each staged by its own lines alone, your identity the author.
  `git.autoCommit` (off by default) proposes each turn's own changes after it.
- **Steps at once, in worktrees of their own.** `run_steps` runs 2 to 4 independent plan steps
  together: a read-only step to a sub-agent, one that changes files to a worker in a git worktree
  whose change is merged back (`git apply --3way`) once its check passed.

### :sparkles: New features & Enhancements

- **Loops and schedules.** `/loop 30m PROMPT` runs a prompt now and every so often as a background
  sub-agent (reading only, or `--edits`); `code-arena schedule add NAME --every 1d --at 02:00
  PROMPT` runs one as a session of its own, with `list`, `run`, `remove`, `due` and `install` (a
  systemd user timer, a launchd agent or a Task Scheduler task).
- **Plugins.** `code-arena plugin install PATH|GIT_URL`, `list`, `enable`, `disable`, `remove`: an
  enabled plugin's commands, sub-agents, skills, hooks and MCP servers (`PLUGIN-SERVER`) join yours;
  a project's `plugins.enabled` enables more of those installed.
- **Session forks.** `/fork` carries on in a copy of the session, `/fork N` in a copy of what came
  before question N; `--fork-session` with `--continue` or `--resume`. The session it came from
  stays as it was; the store is upgraded in place (schema 4).
- **Output styles and your own status line.** `/output-style` (default, explanatory, learning, or
  your `output-styles/NAME.md`); `"statusLine": {"type": "command", "command": "…"}` draws your
  command's first line above the prompt after each turn.
- **Arena's agents and scheduled tasks (8.1 on).** `/arena-agents [run N]`, `/arena-tasks [run N |
  runs N]`, `code-arena agents` and `code-arena tasks`; the model gets `arena_agents` and
  `run_arena_agent` when the Arena has them.
- **The engine's own window, learned.** When it refuses a request and says how much it holds
  (`n_ctx`), that is the window from then on, kept for that gateway and model.
- **A full shared cache is asked again.** llama.cpp's "Context size has been exceeded." (other
  requests filled the cache they share) is asked again after two seconds, compacted only if
  refused again; refusals are read as Arena reads them (llama.cpp, LiteLLM, vLLM, SGLang, OpenAI).
- **An assistant's tool groups.** `/assistant N` keeps only Arena's tools of the groups it has on;
  `/assistant off` puts the others back.
- **Making a project.** `/project new NAME[: WHAT]` makes one in Arena with this repository in it
  and puts the session in it; `/project show` shows it as its page does.

### :bug: Bugs fixed

- **A tool call cut off at the answer's limit could write half a file.** It does not run now: the
  model is told to do it in parts (`write_file` with `append: true`), and a call cut twice in the
  same place ends the turn.
- **On Windows, a file another program held open failed at once.** It is written again for up to
  two seconds, then said so: "held open by another program…: close it there".
- **An error with no body said nothing.** It says so ("Internal Server Error (it gave no reason)").

## v0.3.0 (2026-11-26)

Code Arena and Farzan Arena as one: sessions kept in Arena's chats both ways, the plan, checklists,
memory, skills, projects and assistants; and two editions, managed (signed in to Arena) and
standalone (your own provider's key).

### :rocket: Epics and highlights

- **Sessions kept in Arena's chats, both ways.** The chat is made at a session's first message and
  each message goes after the last one Arena has, in the background; a compaction's summary once
  Arena has what it covers; a session that went back sends on a branch of its own. What the web
  adds comes in at the next turn (and at once while the prompt waits, from Arena's stream of the
  person's chats); while a turn runs it holds the chat, so the web queues. `/web` lists your chats
  and continues one here; `--web N` starts in one; `/sync`, `/sync off`, `/sync all`.
- **Two editions in one product.** Managed, signed in to Farzan Arena, has everything. Standalone
  takes your own provider's key (`code-arena login --provider anthropic|openai|deepseek|openrouter|
  mistral|ollama`, or `--url` for any OpenAI-compatible API); what needs Arena is hidden and says how
  to turn it on.
- **The plan, reviewed and grounded.** The `plan` tool keeps numbered steps with what done means
  (a command that must pass since the step began); plan mode proposes it for your review
  (`/plan approve`, `change N`, `another`, `reject`); it is grounded in Farzan Graph and read again
  with fresh eyes before you see it; `/plan undo N` puts the files back; `--plan` carries it on.

### :sparkles: New features & Enhancements

- **What an Arena has, kept per server.** Asked again after an hour, on an unclear probe, or when the
  account's settings stamp moves; a later Arena's own list (`/capabilities`) is read instead of
  probed. Contract tests replay Arena 8.0's and 8.1's answers.
- **Checklists, kept in Arena.** `/checklist` turns yours or the repository's on in the session's
  chat, the model ticks each item with how it checked it, and the web shows the same ticks.
- **Memory.** Arena's (the person's, the project's by its repository's key, the company's): a
  session starts with it and each message brings what matters; `remember`, `recall`, `/memory`,
  `/remember`; a local fallback without Arena; the project's long memory in `.remember/`.
- **Skills.** The 35 built-in skills that come with Arena, Arena's own skills kept for the next
  session, `skill_save`, and `/skills import` and `export` as zips (secrets never in nor out).
- **Projects and assistants (managed).** `/projects` and `/project N` put a session in a project
  (its goals and tasks kept true to the work on 8.1); `/assistant N` works as one of yours.
- **The rest.** Tab skips the model's thinking; `-p --output stream-json`; `/context` counted with
  the model's own tokenizer; compaction following your Arena account; one long turn compacting
  its own steps; ↑ takes back a line sent while a turn works; `search_history` and `open_chat`;
  Arena's settings followed as they change. Measured: a static binary of 11.1 MB, `version` in
  1.9 ms, a 10,000-message session open in 0.6 ms ([docs/measurements/0.3.md](docs/measurements/0.3.md)).

### :bug: Bugs fixed

- **`/context` said 0 for the tokenizer's total.** Arena totals only a whole request: the total is
  the sum of the counts.
- **A session's first turn did not hold its chat**, so the web chat answered what was written there
  meanwhile on its own: the chat is made at the first turn's start now, and held.
- **A CA file moved after `login --ca` broke every request.** Login keeps its own copy now, and
  `doctor` says how to give one that is gone.
- **The agent's git ran the repository's own filters and merge drivers.** They are switched off
  for every git call.
- **A send whose answer was lost was sent twice.** Messages are known by their ref now.
- **`doctor` would have said Arena 8.1 lacks `checklists.edit`.** Its probe now asks something only
  8.1 answers in its own words.
- **A crash lost the answer being written.** It is kept every two seconds while it comes.

## v0.2.0 (2026-11-26)

The conversation in the terminal, on sessions that never lose a message: MCP servers, tools on
demand, compaction that keeps everything, checkpoints, jobs and a sandbox for the agent's commands.

### :rocket: Epics and highlights

- **`code-arena chat`: a conversation in this terminal.** A prompt edited as a shell's is, history
  per folder, approvals with y, n or a, Ctrl+C to stop a turn, typing while it runs, `/help`,
  `/model`, `/mode`, `/thinking`, `/compact`, `/context`, `/rewind`, `/tools`, `/permissions`, `/mcp`,
  `/jobs`, `!command` and the rest; `--continue` and `--resume`.
- **Sessions that never lose a message.** One SQLite store: every message on disk before the turn
  goes on, read a page of turns at a time, searched with FTS5, compactions that change only what
  the model reads (the session keeps every message), results kept aside (`read_result`), the
  answer being written kept as a draft.
- **The agent's commands in a sandbox.** Bubblewrap on Linux, `sandbox-exec` on macOS: they cannot
  see secrets (`~/.ssh`, cloud credentials, browsers' profiles…), write only in the working folders
  and caches, and cannot reach Docker's socket.

### :sparkles: New features & Enhancements

- **MCP servers.** Arena's tools, Farzan Graph's and your own (`mcpServers`: a command or a URL),
  connected in the background, with deadlines and progress, and tools loaded on demand past
  `tools.onDemandChars`; pictures and files a tool returns saved under `.code-arena/media/`.
- **The window, measured and kept.** The model's window from the gateway, results condensed and
  cleared before anything is summarized, compaction past `agent.compactAt` keeping the recent part
  whole, a summary that comes back empty or in a loop replaced, a refused request compacted and
  asked again.
- **Commands, skills and sub-agents as Markdown files.** The project's and yours: `/NAME`, the
  `skill` tool, `agents/NAME.md`; `/commands`, `/skills`.
- **Jobs.** Commands with no time limit run as jobs: the turn goes on, and the model is told when
  one ends; background commands run until stopped; `command_output`, `stop_command`, `/jobs`.
- **Checkpoints and `/rewind`.** Each turn keeps what its tools changed: `/rewind N` puts the files
  back and cuts the conversation there (or only one of them).
- **`@files` in a message**, a line range or a folder; **waiting in a model's line** as the web chat
  does; the model's thinking shown dimmed; an edit's diff under its call.
- **Measured.** A 10,000-message session opens its last 30 turns in 1.25 ms
  ([docs/measurements/0.2.md](docs/measurements/0.2.md)).

## v0.1.0 (2026-11-26)

The first Code Arena in Rust: signing in, one answer from the command line with the agent's tools,
modes and kept rules, and capability probing that tells Arena 8.0 from 8.1 by what it answers.

### :rocket: Epics and highlights

- **`code-arena -p PROMPT`.** One answer on stdout, progress on stderr, `--output json`; `--mode`,
  `--model`, `--thinking`, `-C FOLDER`, `--add-dir`. The agent's tools: `read_file`, `write_file`,
  `edit_file` (UTF-8, UTF-16 and UTF-32, CRLF kept), `list_dir`, `glob`, `grep`, `run_shell` and a
  read-only `git`; modes `ask`, `auto-edit`, `plan` and `yolo`; kept rules; calls mended before
  they run.
- **Every Arena from 8.0 on.** `login` (an API key, or a user name, password and second factor),
  `logout`, `models`, `doctor` and `version`; each optional feature probed once, an Arena told by
  what it answers, never by its version.

### :sparkles: New features & Enhancements

- **One settings schema.** The person's and the project's `settings.json` (comments allowed),
  environment overrides, a project that may only deny; the connection kept readable by its owner
  only; a test that fails when code reads the environment anywhere but the settings.
- **Instructions and the project's folder.** `ARENA.md`, `AGENTS.md` or another agent's file; the
  project's `.code-arena/` made with its default files at a repository's root.
- **Builds and CI.** The build image, static Linux binaries, CI on six systems and a release
  workflow. Measured: 7.1 MB (C# 8.1: 48.8 MB), started in 0.4 ms (106 ms), 2 MB of memory (83 MB)
  ([docs/measurements/0.1.md](docs/measurements/0.1.md)).

### :bug: Bugs fixed

- **`-p` printed blank lines and listed calls before their results.** Each call now shows with its
  result, in order.
- **`.code-arena/` was made in any folder `-p` ran in.** Only at a repository's root now.
