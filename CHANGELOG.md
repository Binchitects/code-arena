# Changelog

Code Arena's versions, newest first. Before 1.0 the product's own formats may change between
versions; compatibility with Farzan Arena servers from 8.0 on is kept throughout.

## Unreleased (0.4.0)

### Added
- Hooks, in Claude Code's shape (`"hooks"` in settings.json; Claude Code's `~/.claude/settings.json`
  and a project's `.claude/settings.json` read as they are): `SessionStart`, `UserPromptSubmit`,
  `PreToolUse` (block, allow without asking, ask whatever the mode, or rewrite the arguments),
  `PostToolUse` (a word to the model with the result), `Notification` (when Code Arena asks you),
  `Stop` (send the model on: three times a turn at most) and `PreCompact`. Each runs a command
  that gets the moment as JSON on stdin and answers by its exit code (2 blocks, its stderr says
  why) or with JSON. A matcher is a pattern over the tool's name, Code Arena's or Claude Code's
  (`run_shell` or `Bash`). A project's hooks run only once you trust them as they are
  (`/hooks trust`; a change asks again). `/hooks` lists them.
- llama.cpp's "Context size has been exceeded." (the cache the engine's requests share was full,
  others filled it) is asked again after two seconds as it is, compacted only if refused again.
  Refusals are read as Arena reads them (llama.cpp, LiteLLM, vLLM, SGLang, OpenAI), with the
  tokens they state.
- The engine's own window is learned: when it refuses a request and says how much it holds
  (`n_ctx`), that is the window from then on ("The engine holds 20.5k tokens, not the 32.8k the
  gateway says: Code Arena compacts for 20.5k from now on."), kept for the next sessions with
  that gateway and model while the gateway says the same window.
- A tool call cut off at the limit of one answer does not run (mended, it would have written half
  a file): the model is told to do it in parts (`write_file` with `append: true` for each part of
  a long file), and a call cut twice in the same place ends the turn.
- On Windows, a file another program holds open (an editor, a build, a virus scan) is written
  again for up to two seconds, then said so: "held open by another program…: close it there".
- An error with no body says so ("Internal Server Error (it gave no reason)").
- Output styles, in Claude Code's format: `/output-style` lists them and `/output-style NAME`
  uses one from the next message (kept in the settings, `outputStyle`). Built in: default,
  explanatory (explains the codebase's choices as it goes) and learning (leaves small pieces,
  marked `TODO(human)`, for you to write). Your own are `output-styles/NAME.md` (the project's,
  yours, a plugin's) with `name`, `description` and `keep-coding-instructions`: without it, a
  style takes the place of Code Arena's coding instructions.
- A status line of your own: `"statusLine": {"type": "command", "command": "…"}` (or Claude
  Code's in `~/.claude/settings.json`). The command gets the session as JSON on stdin (model,
  workspace, tokens so far, output style, mode, edition) and its first line is drawn above the
  prompt, again after each turn; one that fails or takes over two seconds shows nothing.
- Session forks: `/fork` carries on in a copy of the session, `/fork N` in a copy of what came
  before question N (as `/rewind` numbers them); `--fork-session` with `--continue` or `--resume`
  does it from the shell. The fork has the messages, summaries, results kept aside and (a whole
  fork) the plan and goals; it gets a chat of its own in Arena when it syncs. The session it came
  from stays as it was (`/resume ID` goes back); files are not touched. The store is upgraded in
  place (schema 4: the session a fork came from).
- Your agents and scheduled tasks in Arena (8.1 on; an 8.0 says it lacks them): `/arena-agents`
  lists the Agent Studio agents you may run (yours and those shared with you) and
  `/arena-agents run N [TEXT]` runs one, telling you when it ends; `/arena-tasks` lists your
  scheduled tasks with their next and last runs, `run N` starts one now, `runs N` says how its
  last runs went. From the shell: `code-arena agents [run NAME [TEXT] [--field K=V]]` (waits for
  the answer) and `code-arena tasks [run NAME | runs NAME]`. The model gets `arena_agents` and
  `run_arena_agent` (asks as a command does) when the Arena has them.
- Plugins, in Claude Code's layout (`.claude-plugin/plugin.json`, `commands/`, `agents/`, `skills/`,
  `hooks/hooks.json`, `.mcp.json`): `code-arena plugin install PATH|GIT_URL`, `list`, `enable NAME`,
  `disable NAME`, `remove NAME`. An enabled plugin's commands, sub-agents and skills join yours
  (yours and the project's win by name), its hooks run as yours do, and its MCP servers are
  named `PLUGIN-SERVER`; `${CLAUDE_PLUGIN_ROOT}` is its folder. `plugins.enabled` in a project's
  settings enables more of those installed.
- The `task` tool: the model hands a piece of work to a sub-agent (one of the project's or your
  `agents/NAME.md`, with its instructions, model and tools, or a general one), which works in a
  context of its own and reports back; several in one answer run at once (four at most). Its
  calls ask as the session's do; it has no `task` of its own, nor the session's plan. Its steps
  show as lines of their own; a `SubagentStop` hook may send it on, once.
- Goal mode: `/goal TEXT` (or `code-arena -p "…" --goal TEXT`) and the agent works turn after turn
  until the goal is met as its check says: `--check CMD` (its exit code) or `--when CONDITION` (a
  fresh sub-agent reads the files), else the agent writes one down first with `goal_check`. A
  budget (`--steps`, `--minutes`, `--tokens`; or `--until-met`); it stops when the budget is
  spent, after three turns that do nothing while the check fails, or three turns with no check.
  `/goal` says where it stands; `pause`, `resume` (Ctrl+C pauses), `cancel`; more `/goal TEXT`
  wait their turn (`drop N`). Kept with the session (a 0.3 store is upgraded in place) and shown
  above the web chat.
- Commits the model groups and writes: `/commit` proposes commits of the working tree's changes
  (by hunk, in the repository's own style of messages: a subject under 72 characters and a
  body that says why); they are shown, and `/commit yes` makes them, each staged by its own
  lines alone, your identity the author, with `Co-Authored-By: Code Arena` and
  `Code-Arena-Session:` trailers. With `git.autoCommit` on (off by default), each turn's own
  changes (what it wrote, minus files that held your changes when it began) are proposed after
  it (`git.autoCommit.review`, on: shown first). Never during a merge, rebase, cherry-pick or
  revert, nor while the session's commands run; a `PreCommit` hook (or git's own) that refuses
  stops the series. A proposal that leaves a hunk out is asked once more, then made one commit.
- Steps at once (`run_steps`): 2 to 4 steps of the plan that wait for nothing not done run
  together. A step with no files to change goes to a read-only sub-agent; one with files to a
  worker in a git worktree of its own (a copy of the project as it is now, your uncommitted
  changes in it): it edits there, runs no command but its step's check, and its change is merged
  into the project (`git apply --3way`, every file kept for `/rewind`) once the check passed.
  Each step ends done or blocked, with why in its notes; a worktree that did not merge is kept
  for you to look at. Steps that change files run at once only in auto-edit or yolo mode.
- Loops: `/loop 30m PROMPT` runs a prompt at once and then every so often as a background
  sub-agent while the session is open, reading only (`--edits`: it edits files too; its commands
  are refused, as nobody is there to allow them); a run never overlaps the one before it; its
  report reaches the agent with your next message. `/loop` lists them, `/loop stop N`.
- Schedules: `code-arena schedule add NAME --every 1d --at 02:00 [--mode plan] [--model M]
  [-C FOLDER] PROMPT` runs a prompt on its own as a session of its own (kept with a chat in Arena
  as any session is), its output kept in a log; `list`, `run NAME`, `remove NAME`, `due` (runs
  those due), `install` (a systemd user timer, a launchd agent or a Task Scheduler task running
  `due` every 5 minutes), `uninstall`.

## 0.3.0 (2026-11-26)

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
- `code-arena --web N` (or an id) starts the conversation in one of your chats in Arena.
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

- Skills in and out as files: `/skills import PATH` takes a `.zip` (one skill's folder or several),
  a folder or a lone SKILL.md, into the project (`--to project`, the default), yours (`--to
  mine`) or Arena (`--to arena`, managed: its own checks and safeguards); one of the same name is
  kept unless `--replace`. `/skills export NAME [PATH.zip]`. At most 50 skills and 5 MB; every
  file stays inside its skill's folder; a file that looks like it holds a secret (an API key, a
  token, a private key) never comes in, nor goes out.

- Projects (managed): `/projects` lists yours, `/project N` puts the session's chat in one (`off`:
  none). On an Arena with project work (8.1 on) the model keeps the project's goals and tasks
  true to the work with `project_plan`, `project_goal` and `project_task`, told the project's
  instructions; a resumed session in a project takes it up again. Requests carry `chat:` and
  `project:` tags for Arena's usage log. A session's chat names its repository, so Arena can put
  it in that repository's project.
- Assistants (managed): `/assistant` lists yours; `/assistant N` works as one (its instructions,
  model, thinking and skills) from the next message; `/assistant off`.

- Tab while the model thinks skips its thinking: still only thinking, the request is cut and asked
  again without it, so the model answers at once; the rest of the turn does not think.
- `-p --output stream-json`: one JSON object a line on stdout as the turn goes (`start`, `text`,
  `thinking`, `tool_start`, `tool_end`, `notice`, `step_end`), then the `result`.

- `/context` counts with the model's own tokenizer, in Arena (managed, Arena's models); else it
  estimates from characters, and says which.
- When a session compacts follows your Arena account (Your account → Chat), unless you set
  `agent.compactAt` here; `/compact-at` keeps what you give it in your settings.json (its comments
  and other settings as they were).
- One long turn past the target on its own compacts its own earlier steps: your message stays,
  a summary of the work so far follows it, the last steps stay whole (no result parted from its
  call). The session keeps every step.

- While a turn works: ↑ with nothing typed takes back the last line you sent meanwhile (before the
  model reads it), and it waits at the prompt; a `/command` or `!command` typed meanwhile runs
  when the turn ends, in order, instead of going to the model.

- A plan proposed in plan mode is grounded before your review: each step's symbols are looked up
  in Farzan Graph (`change_impact`: the lines that use them, the repositories that depend on this
  one, the tests that name them), and the plan is read again with fresh eyes (`plan.secondReading`,
  on): what is missing, risky, unchecked or in the wrong order; the agent changes the plan where
  the notes hold, once. `/plan` shows both.
- `/sync all`: every session of this folder that Arena lacks (from before chats were kept, or
  while it was out of reach) sent now, each to its own chat.
- Arena's settings as they change: the chat asks for their stamp every 30 s, and when they moved,
  Arena's skills are read again (used from your next message).

### Fixed
- `/context` said 0 for the tokenizer's total: Arena totals only a whole request, so the total is
  the sum of the counts.
- A session's first turn did not hold its chat (the chat was made only by its first send), so the
  web chat answered what was written there meanwhile on its own: the chat is made at the first
  turn's start now, from the question, and held as every turn's is.
- A CA file moved or deleted after `login --ca` broke every request: login keeps its own copy of it
  now (`arena-ca.pem` in the config folder), and a CA file that cannot be read says how to give it
  again; `doctor` says so instead of stopping with an error.
- The agent's git ran the repository's own filters and merge drivers (commands its settings name,
  such as a clean filter in `.gitattributes`): they are switched off for every git call now.
- A send Arena kept but whose answer was lost: the next take-in would have added the session's
  own messages again as the web's, and sent them twice. They are known by their ref now.
- `doctor` would have said Arena 8.1 lacks `checklists.edit`: its probe asked to change an unknown
  checklist, which 8.1 answers with the same plain 404 as 8.0's missing route. It now asks to
  make one with no name, which 8.1 refuses with its own answer (and makes nothing).
- The answer being written is kept every two seconds while it comes, so a crash leaves what it
  got to (0.2 recovered drafts but never kept them).

### Measured
- The static Linux binary is 11.1 MB; `code-arena version` takes 1.9 ms; a 10,000-message
  session opens with its last 30 turns in 0.6 ms. See [docs/measurements/0.3.md](docs/measurements/0.3.md).

### Moved to 0.4
- A cut tool call redone in parts, files held open by others on Windows, llama.cpp's shared
  cache asked again and the engine's own window learned, assistants' tool groups, making a
  project and its views. The plan's checklist names each.

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
