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
