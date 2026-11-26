# Changelog

Code Arena's versions, newest first. Before 1.0 the product's own formats may change between
versions; compatibility with Farzan Arena servers from 8.0 on is kept throughout.

## Unreleased (0.3.0)

### Added
- What an Arena has is kept per server (`servers/HOST.json` in the data folder) and asked again
  after an hour, when a probe got no clear answer, or when the account's settings stamp moves.
  A later Arena that lists what it has (`GET /api/code-arena/capabilities` answering
  `{"features": [...]}`) is read instead of probed.
- Contract tests replay Arena 8.0's and 8.1's answers to every probe.
- Sessions kept in Arena's chats (`arena.syncChats`, on): the chat is made at a session's first
  message (found again by the session's id, with its folder shown), and each message the session
  keeps goes after the last one Arena has, in the background, again until Arena has it; a tool's
  result goes with its tool's name, an answer with its model; a compaction's summary goes once
  Arena has what it covers. A session that went back to before messages Arena has (`/rewind`)
  sends on a branch of its own. A chat deleted on the web ends the sync for that session. `-p`
  waits up to 10 s for its messages to arrive (not at all when Arena is out of reach); what is
  left goes with the session's next run. `/sync` says where a session stands; `/sync off` keeps
  it here only. Sessions from 0.2 are kept (the store is upgraded in place) and sent when resumed.
- What the web added comes in at the next turn's start: the person's questions and the web's
  answers are written into the session (and said: `From the web: …`), and the model reads them
  before your next words. When the web went another way (a question edited, an answer written
  again), the session follows it from where they part; messages not yet sent go after the web's.
  A summary the web's compaction wrote is read as the session's own.
- While a turn runs it holds the chat: the web queues the person's messages, and those it may
  take join the turn at its next step. A turn waits for the web chat's own answer to end (at
  most 2 min). The answer being written is shown on the web as it comes. The chat is let go
  once Arena has the turn's messages.

- The web's news while the chat waits at its prompt: what is added there shows at once (the
  person's words, and the web's answers in full) and the model reads it at the next turn. It
  comes from Arena's stream of the person's chats' changes; where that cannot be had (a proxy
  that drops it), the chat is asked every 5 s with its stamp, which costs Arena nothing when
  nothing moved.
- `/web` lists your chats in Arena; `/web N` continues one here, in a session of its own (or the
  one this folder already keeps with it), and brings it back to the web's list if it was archived.
- Leaving the chat waits a few seconds for the last turn's messages to reach Arena.
- The `plan` tool: a title, a goal and numbered steps, each with what done means (`check`: a
  command, or what to look at), its files and symbols, the steps it waits for, and notes. A step
  whose check is a command is done only once that command passed with `run_shell` since the step
  began (`cd src && cargo test 2>&1 | tail` counts for `cargo test`). One step is doing at a
  time; steps wait only for steps the plan has, never in a circle. A call is read kindly (steps
  as a string of JSON, `id`/`description`/`dependsOn`, other agents' status words) and what was
  read so is said back. The plan is shown whole when it changes, kept with the session, said
  again after a compaction and when a session is resumed, and kept in Arena on the chat's Plans
  board.
- In plan mode a plan is proposed for your review: `/plan approve` (edits then run; `approve ask`
  asks before each), `/plan change N what`, `/plan another why`, `/plan reject why`.
  `/plan undo N` puts the files back as they were before step N began (and every step begun
  after it); those steps are to do again. `/plan resume` and `code-arena --plan` carry on the plan
  an earlier session of this folder left. `/plan clear`.

- Checklists, kept in Arena: `/checklist` lists yours and the repository's
  (`.code-arena/checklists/*.md`, `# Name` and `- item` lines); `/checklist N` or `NAME` turns one
  on in the session's chat, and the model goes through it, ticking each item with
  `checklist_check` and how it did or checked it (the web chat shows the same ticks). `/checklist
  tick N`, `untick N`, `off`; `new NAME: a; b`, `edit N: a; b` and `delete N` on an Arena that
  has them (8.1 on). A resumed session with a checklist on tells the model again.

- Memory. Signed in, Arena's memory, the web chat's too: the person's for every project, the
  project's by its repository's key (its `origin` remote, `host/group/name`, so every clone
  shares it), the company's. A session starts with Arena's block (pinned, the company's, the
  newest), and each message brings what is remembered that matters to it (not what came before).
  `remember` (a fact, preference, decision, how-to or lesson; for this project or, with scope
  person, every project) and `recall` for the model; `/memory`, `/memory forget N` and
  `/remember [--person] TEXT` for you. Without Arena: `memory.md` in the config folder and this
  project's file in the data folder (never in the repository), with what Arena last gave; they go
  to Arena when it next answers. An Arena that denies `feature.memory` gets nothing remembered
  or recalled, and the model is not offered it.
- The project's long memory in the repository, `.remember/MEMORY.md` with topic files: read at
  each session's start, kept current by the model with `remember_project` (a section added to
  or replaced), shared with the team when committed (`memory.project`, on).
- `skill_save`: a way of doing something worked out in a session, kept as a skill: the project's
  (`.code-arena/skills/NAME/SKILL.md`) or, with scope person, yours in Arena (for every project and
  the web chat). After a turn of 8 steps or more, the next is reminded of it.
- `search_history` and `open_chat`: your past chats and sessions in Arena, found by what they
  were about.
- A session's chat in Arena names its project by the same key.

- Built-in skills: the 35 that come with Arena (code-review, debug, release-notes, security-review
  and the rest), read after the project's, yours and Arena's, which win by name; those an Arena
  admin turned off are left out (`skills.builtIn`, on).
- Arena's skills: those made there that you may use (yours, your groups', the company's) and your
  plugins', kept for the next session (`arena-skills/` in the data folder) and refreshed at each
  start; a file outside its skill's folder is left out.

- Two editions in one product. **Managed**: signed in to Farzan Arena, everything. **Standalone**: no
  Arena, your own provider's key: `code-arena login --provider anthropic` (or openai, deepseek,
  openrouter, mistral, ollama, or your own name with `--url` for any OpenAI-compatible API), or
  just its variable (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`…). Its models are `NAME:MODEL`; an
  OpenAI-compatible provider is sent only what its API takes (none of Arena's own fields), and
  Anthropic is spoken through its Messages API (the instructions cached). In the standalone
  edition what needs Arena (`/web`, `/sync`, `/checklist`, Arena's memory, skills and past
  chats, its tools) is hidden and says how to turn it on; local memory, `.remember/`, skills,
  the plan and your MCP servers work as in the managed one. Managed, your providers' models sit
  beside Arena's where Arena's admins allow them. `version`, `doctor` and the chat name the
  edition; `logout --provider NAME` forgets one. Tested against fake providers only so far.

### Fixed
- `doctor` would have said Arena 8.1 lacks `checklists.edit`: its probe asked to change an unknown
  checklist, which 8.1 answers with the same plain 404 as 8.0's missing route. It now asks to
  make one with no name, which 8.1 refuses with its own answer (and makes nothing).
- The answer being written is kept every two seconds while it comes, so a crash leaves what it
  got to (0.2 recovered drafts but never kept them).

## 0.2.0 (2026-11-26)

### Added
- Sessions kept in one SQLite database in the data folder (`sessions.db`): every message on disk
  before the turn goes on, read back a page of turns at a time, searched with FTS5, with
  compactions, results kept aside and the answer being written kept as a draft (recovered after a
  crash).
- `code-arena chat`, and `code-arena "message"`: a conversation in this terminal. The prompt is
  edited as a shell's is (word moves, Ctrl+A/E/U/K/W, Ctrl+L, multi-line with Alt+Enter or a
  trailing `\`, bracketed paste), ↑ and ↓ bring back what was sent in this folder (kept per
  folder, 1,000 lines), Esc returns to what was being typed; approvals answer with y, n or a;
  Ctrl+C stops a turn and keeps what was said; a line typed while a turn runs is read at its next
  step; Ctrl+Z stops code-arena as a shell stops a job; the bell rings after a long turn and at
  a question.
- `--continue` and `--resume [ID]` (no id: choose from a list), for the chat and for `-p`;
  `-p --output json` names its session.
- MCP servers: Arena's tools (`https://DOMAIN/mcp`), Farzan Graph's (its copy of a tool Arena
  serves left out, so no tool comes twice) and your own under `mcpServers` (a child process with
  `command`, `args`, `env`, `cwd`, or a `url` with `headers`; `${NAME}` from the environment;
  `trust`, `disabled`). They connect in the background: a conversation waits up to 3 s
  (`mcp.startWait`), `-p` up to 25 s, and the wait ends as soon as each has answered once; one that
  answers later joins the session at its next step. A server that went away is connected again at
  the next call; one that fails is tried again after 5 s, then less often (up to every 5 min).
- Tool calls have a deadline (10 min, or what the tool says in `_meta`), which each progress report
  starts again; past it the call is given up and the server told (`notifications/cancelled`).
- Tools on demand: past `tools.onDemandChars` (6,000) characters of a server's definitions, its
  tools are listed in a note and loaded with `load_tools`, or by being called; what is loaded stays
  loaded, so the start of the requests stays the same.
- Pictures, sounds and files a tool returns are saved under `.code-arena/media/` (git-ignored),
  named after the tool and the time, and the model is told where.
- The model's window from the gateway (`/v1/model/info`, else `agent.context`, else 32,768), with
  room kept for the answer (its longest answer, at most an eighth of the window), and the
  characters a token holds learned from the gateway's own counts.
- Before anything is summarized: a result over 16,000 characters is kept aside in the session
  (`r1`, `r2`…) and the model reads its start and end with how to read on (`read_result`, by
  lines or by text); results the model has used are condensed to their key lines; past the
  threshold, results older than your last two messages are cleared (kept aside).
- Compaction past `agent.compactAt` (80%), keeping the recent part whole (`agent.compactTarget`,
  25%) from a message of yours: the summary is asked with the same prompt and tools
  (`tool_choice: none`), in sections; one that comes back empty, cut or in a loop is replaced by
  what the older messages held. The session keeps every message; only what the model reads
  changes, and a resumed session starts from its last summary. A request the window refuses is
  compacted and asked again, once.
- `session_search` and `session_read` (this session's messages, those summarized too), and
  `search_sessions` (the folder's other sessions).
- In the chat: `/compact`, `/context` (what fills the window, by kind, and the biggest messages),
  and after each turn how full the window is (`context 18.2k / 131k (14%), compacts at 80%`).
- Commands, skills and sub-agents as Markdown files, the project's (`.code-arena/`, `.arena/`,
  Claude Code's `.claude/`, Qwen Code's `.qwen/`) over yours (the config folder, `~/.code-arena`,
  `~/.claude`), read again at each message: `commands/NAME.md` is `/NAME`
  (`commands/git/commit.md` is `/git:commit`; `$ARGUMENTS`, `$1`…; front matter `description`
  and `argument-hint`); `skills/NAME/SKILL.md` is listed for the model, loaded with the `skill`
  tool (and the files beside it), or used as `/NAME words`; `agents/NAME.md` (`name`,
  `description`, `tools`, `model`) are read and listed (the `task` tool that runs them comes in
  0.7). `/NAME` works in `-p` too.
- `/commands` and `/skills` in the chat.
- Commands with no time limit (`run_shell` with `no_time_limit`: a full build, a long test suite)
  run as jobs of their own: the turn does not wait, and when one ends the model is told its exit
  code and the end of its output and carries on in a turn of its own. Commands in the
  `background` (a dev server) run until stopped, the model getting their first output after a few
  seconds. `command_output` reads a job's latest output (or waits for it); `stop_command` stops
  one and asks every time, in every mode. Ctrl+C stops the turn and the jobs it started with no
  time limit; those in the background run on. `/jobs [stop N]`. Asking says how a command waits,
  and "always" for one way does not cover another; kept rules hold for every way. `-p`, which ends
  with its answer, refuses them with a reason.
- The agent's commands (`run_shell`, jobs, `!command`) run in a sandbox where the system has one:
  bubblewrap on Linux, `sandbox-exec` on macOS (`tools.sandbox`: `auto`, `on`, `off`). They read
  everything but what holds secrets (`~/.ssh`, `~/.gnupg`, cloud and Kubernetes credentials,
  `.netrc`, `.npmrc`, browsers' profiles, Code Arena's own folders), which they cannot see; they
  write only in the working folders, the build tools' caches, `tools.sandboxWritable` and a
  `/tmp` of their own; Docker's and Podman's sockets are out of reach; `tools.sandboxNetwork`
  takes the network away. Without one, the session says why. `/sandbox` says what applies.
- `@files` in a message: `@src/parser.rs` sends the file with it, numbered as `read_file` numbers
  it; `@src/parser.rs:40-60` (or `#L40-L60`) those lines with three around them; a folder its list;
  `@"a b.txt"` a name with spaces. What is no file the tools may read (`@Override`, an email
  address) stays as typed. 60,000 characters a file, 150,000 a message.
- Waiting in a model's line: while Arena holds a request back because the model's line is full, Code
  Arena waits in it as a person in the web chat does, asking again when Arena says, for up to
  15 minutes.
- In the chat: the model's thinking shown dimmed before its answer; an edit's diff under its call;
  `/tools [on|off NAME]` (a tool, or a server's tools), `/permissions [allow|deny RULE]` and
  `/compact-at [% [%]|off]` for the session.
- Options such as `--mode` and `--model` are taken after `chat` too.
- Checkpoints: each turn keeps what every file its tools wrote held before (the last 50 turns, in
  this run). `/rewind` lists your questions; `/rewind N` goes back to before question N (the files
  put back, a file a turn made removed, and the conversation cut there), `/rewind N code` or
  `/rewind N chat` only one of them. What commands changed is not put back.
- `/mcp` in the chat: each server, connected or not and why.
- In the chat: `/help`, `/model`, `/mode`, `/thinking`, `/cost`, `/clear`, `/resume`, `/search`,
  `/exit`, and `!command`.

### Measured
- A session of 10,000 messages opens with its last 30 turns read in 1.25 ms, the 30 before them
  in 0.75 ms; what the model reads (from the last compaction) in 1.5 ms; each message is on disk
  in 0.35 ms; a search of the folder's sessions takes 7.6 ms. See
  [docs/measurements/0.2.md](docs/measurements/0.2.md).

### Moved to 0.3
- Other providers' models, Tab to skip thinking, `--output stream-json`, the Arena account's
  compaction threshold and Arena's tokenizer for `/context`, git's filters and merge drivers kept
  from running, a long turn compacting its own steps. The plan's checklist names each.

## 0.1.0 (2026-11-26)

### Added
- `code-arena login` (an API key typed without echo, or a user name, password and second
  factor), `logout`, `models` (each with its window and longest answer), `doctor` (the folders,
  the Arena, the account, the models, Arena's tools, and what this Arena has, feature by
  feature, kept for the next start) and `version`.
- `code-arena -p PROMPT` (or `-p -` from stdin): one answer on stdout, the progress on stderr,
  `--output json` with the result, the model and the tokens; `--mode`, `--model`, `--thinking`,
  `-C FOLDER`, `--add-dir`.
- A minimal MCP client over streamable HTTP (initialize, tools page by page, JSON or an event
  stream), for counting Arena's tools.
- A test that fails when code reads an environment variable anywhere but the settings.
- The Rust workspace: `ca-core`, `ca-settings`, `ca-net`, `ca-arena`, `ca-llm`, `ca-tools`,
  `ca-workspace` and the `code-arena` program.
- One settings schema (`ca-settings`), with the person's and the project's `settings.json`
  (comments and trailing commas allowed), environment overrides, and a project that may only
  deny, never allow.
- The Arena connection kept in `credentials.json`, readable by its owner only; `ARENA_*`
  variables override it for one run.
- The Arena client: the API key and `X-Requested-With` on every call, Arena's refusals in its
  own words, password sign-in with the second factor (a key made for this machine), and
  **capability probing**: each optional feature is asked about once, and an Arena 8.0 is told
  apart from 8.1 by what it answers, never by its version.
- The gateway client: models with their windows, answers streamed with thinking and tool calls,
  retries for a gateway briefly away, refusals as Arena words them, a dropped stream kept.
- The agent's tools: `read_file`, `write_file`, `edit_file` (UTF-8, UTF-16 and UTF-32 by their
  mark, CRLF kept), `list_dir`, `glob`, `grep` (no backtracking, `.gitignore` followed),
  `run_shell` (time limit, the process group stopped, the API key never passed on) and a
  read-only `git`.
- The agent's turn: requests and tool calls until the model answers; reading calls run at once,
  changing ones in order; an answer cut at the output limit or by a dropped stream carried on.
- Modes `ask`, `auto-edit`, `plan` and `yolo`; kept rules (`permissions`: `run_shell(npm test*)`,
  commands matched whole and part by part); "always" for the session; a run that cannot ask refuses
  what would ask, with a reason the model passes on.
- Calls mended before they run: other harnesses' tool and argument names, nearly-JSON arguments,
  wrong types; a call missing what it needs is explained with the tool's parameters.
- Instructions from `ARENA.md`, `AGENTS.md`, `CLAUDE.md` or `QWEN.md` (the person's, the
  repository root's and the working directory's).
- Paths resolved as the system resolves them: a link that leads out counts as outside.
- The project's `.code-arena/` folder, made with its default files at a repository's root the
  first time (a folder that is no repository gets none).
- The build image (`docker/builder`), static Linux binaries (musl), Windows binaries with the C
  runtime inside.
- CI on six systems (Linux x64 and arm64, macOS arm64 and x64, Windows x64 and arm64): format,
  clippy and the tests; a release workflow that builds, packages and publishes each system's
  binary with `SHA256SUMS` on a `v*` tag; a workflow that publishes the build image to GHCR.

### Fixed before release
- `-p` printed blank lines before a step's tool calls, and listed calls that ran at once before
  their results; each call now shows with its result, in order.
- `.code-arena/` is made only at a repository's root, not in any folder `-p` runs in.

### Measured
- The program is 7.1 MB (C# 8.1: 48.8 MB), starts in 0.4 ms (106 ms) and uses 2 MB of memory to
  say its version (83 MB). See [docs/measurements/0.1.md](docs/measurements/0.1.md).
