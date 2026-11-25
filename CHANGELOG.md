# Changelog

Code Arena's versions, newest first. Before 1.0 the product's own formats may change between
versions; compatibility with Farzan Arena servers from 8.0 on is kept throughout.

## Unreleased (0.2.0)

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
- `/mcp` in the chat: each server, connected or not and why.
- In the chat: `/help`, `/model`, `/mode`, `/thinking`, `/cost`, `/clear`, `/resume`, `/search`,
  `/exit`, and `!command`.

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
