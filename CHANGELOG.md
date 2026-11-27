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

- **The IDE is a native app, drawn on the GPU, laid out as VS Code's workbench.** `code-arena ide`
  opens its own window (Floem, Lapce's UI); `code-arena ide --browser` serves the page as before.
  VS Code's title bar (the menu, the command center, the layout buttons, the window's buttons),
  its 48 px activity bar with VS Code's own icons (Codicons), the Explorer, the editor with its
  tabs, breadcrumbs and line numbers (Ctrl+S saves), the panel, the Agent as VS Code's chat view in
  the secondary side bar (answers streamed, a turn's calls in a line that opens, Allow / Always /
  Deny), and the status bar; each part resized by its sash, Ctrl+B, Ctrl+Alt+B and Ctrl+J show
  and hide them. Coloured by VS Code's colour ids from the theme in force, with VS Code's own
  defaults where a theme names none.
- **The native window's views move as VS Code's.** Right-click a view's icon or its title: Move to
  Primary Side Bar, Secondary Side Bar or Panel, Hide it, or Reset View Locations. Where the views
  are and the parts' sizes are kept for the folder (in the data folder's `ide/`) and come back the
  next time it opens.
- **Search in the native window**, as VS Code's: it searches as you type (Match Case and Regular
  Expression toggles), says how many results in how many files, lists them by file with the match
  lit, and a click opens the file at that line.
- **The agent's changes in the native window**, as VS Code's Source Control list: each file it
  changed this session with its letter (M, A, D) in VS Code's colours, read again after each
  turn; a click opens the file's changes as an inline diff (removed lines red, added green, the
  numbers before and after).
- **Syntax colours in the native editor, from the theme's own token colours.** Each file's grammar
  (TextMate's, as VS Code's are) names its runs of text, and the theme's `tokenColors` colour them
  by VS Code's rules; an edit parses again from its line only. The status bar names the language.
  Dark+, Dark Modern, Light+ and Light Modern now carry VS Code's fuller token rules (macros,
  namespaces, constants, parameters, escapes and more), in the browser face too. The editor takes
  VS Code's font for the system (the first of them installed), and the files open in it are
  opened again the next time.
- **The terminal in the native window**, as VS Code's: the session's own shell (the driver's, on
  `ca-pty`), its output understood by `alacritty_terminal`'s emulator and drawn in the theme's
  terminal colours (xterm's 256 and true colour, bold as bright, the caret), fitted to the panel
  as it is resized, the wheel scrolling back through 10,000 lines. Keys go as xterm's (the
  application cursor mode too); Ctrl+Shift+V pastes (bracketed when the program asks); the
  workbench's own keys (Ctrl+B, Ctrl+J, Ctrl+`) stay the workbench's. When the shell ends, a key
  starts another.
- **The native window's views are dragged, and its parts resized, as in VS Code.** Drag a view's
  icon or tab onto a side bar, the panel or the activity bar (where it would go lights up); a part
  that is hidden takes it at its edge of the window. A sash follows the pointer exactly, with VS
  Code's limits: a side bar at least 170 px and the panel 77, as large as the window allows while
  the editor keeps 220 × 70; dragged under half its least, a part closes, and a double-click on
  its sash gives it its first size. The parts fit the window again when it is resized. Long lines
  in the editor scroll, as VS Code's, instead of wrapping.
- **Views dock as in Visual Studio.** The native window's layout is a dock tree: drag a view by its
  icon, title or tab and Visual Studio's guide shows on the part under the pointer (dock left,
  right, above, below, or into its tabs) and at the window's four edges, the place it would take
  shaded; release to dock it there, Escape to give up. Any number of groups, in rows and columns;
  each part keeps its size, the editor takes the rest. Right-click a view to dock it beside the
  editor or with another group. Where everything is, what is hidden and each part's size are kept
  per folder; Reset View Locations gives VS Code's layout back.
- **Floating views, as Visual Studio's floating tool windows.** Release a dragged view away from
  every guide, or right-click it and Float: it opens in a window of its own (in the theme, with a
  button that docks it back). Closing that window docks the view back beside the editor; floating
  views open again with the folder.
- **The Agent's answers drawn from their Markdown**, as VS Code's chat draws them: headings, bold
  and italic, inline code, fenced code in a box with the theme's syntax colours, lists, quotes and
  links; as an answer streams in, only its last block is drawn again. The mode, the model and the
  thinking are picked from menus over the input (as the page offers them; not while a turn runs).
- **The Outline, find, and files changed on disk, in the native window.** The Outline lists the
  file's symbols from its grammar (functions, methods, types, traits, modules, constants,
  headings) with VS Code's symbol icons, nested, the one at the caret lit, a click going there;
  it is read off the UI thread. Ctrl+F opens VS Code's find widget (match case, regular
  expressions, N of M, F3 and Shift+F3). A file changed on disk is taken in when the tab has no
  changes of its own, else a bar offers Revert or Overwrite; closing a tab with changes asks Save,
  Don't Save or Cancel. The workbench's keys work while the editor has the focus.
- **Several terminals, and text selected in them.** New Terminal (Ctrl+Shift+` or the panel's +)
  and Kill (its bin); with two or more, VS Code's list of them at the panel's right. Drag to
  select (a double click a word, a triple click a line), Ctrl+Shift+C copies, a right-click copies
  a selection or else pastes; Terminal: Clear in the palette.
- **The Explorer's file operations, as VS Code's.** New File and New Folder typed in a row of the
  tree, a row renamed in place (F2, open tabs following it), a file or a folder deleted after a
  question (Delete) with a copy kept among the session's backups, a row's menu on a right-click
  (Copy Path and Copy Relative Path too), and the arrows to go through the tree. The driver makes,
  moves and deletes only inside the project, a link as a link.
- **Quick Open and the Command Palette in the native window**, VS Code's quick input: Ctrl+P (or a
  click on the command center) finds a file among 50,000 in about 2 ms a key, its matched letters
  lit; Ctrl+Shift+P (or `>`) runs a command, its keys shown; Ctrl+K Ctrl+T picks a colour theme,
  shown as the arrows move and kept on Enter.
- **Development builds optimise their dependencies**, so the native window built for trying it is
  as quick as the released one (Floem's layout, text and drawing are far too slow unoptimised).
- **Coder mode is VS Code's workbench.** An activity bar (VS Code's own icons), the primary side bar,
  the editor, the panel and a secondary side bar, with VS Code's sizes and the theme's colours.
  Six views (Explorer, Search, Changes, Outline, Agent, Terminal) start where VS Code puts them:
  the four first in the side bar, the agent in the secondary side bar, the terminal in the panel.
- **Views move as in VS Code and Visual Studio.** Drag a view's icon or tab to another container
  (where it may go lights up; a hidden one takes it at the window's edge), or right-click it:
  Move to Primary Side Bar, Secondary Side Bar or Panel. The side bar goes left or right, the panel
  to the bottom, left or right; Ctrl+B, Ctrl+Alt+B and Ctrl+J show and hide them, Ctrl+Shift+E,
  Ctrl+Shift+F, Ctrl+Alt+I and Ctrl+` show a view; Reset View Locations puts them back. Kept per
  folder, with the panes' sizes.

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
- **The IDE's session can carry on by itself.** What a command hands the agent as the next message
  (a plan's review, a goal's next turn) is run as the next turn in the IDE as in the terminal; Stop,
  or a turn that fails, ends the chain.
- **The IDE knows what the About box and Help need.** The session's state now carries the licence
  and its text, the source link, the log's place, the link to the Arena's manual and what this
  edition locks, each with why.
- **Show Outline is in the Command Palette.** `View: Show Outline` shows the Outline view from the
  palette of the native window, like the other views.
- **The IDE's settings are real settings.** Every editor, files, workbench, Explorer, source
  control, compare, terminal, debug, accessibility and language server setting of the old Code
  Arena (138, under VS Code's names) can be set in settings.json and is checked as it is read. A
  value of the wrong kind is not taken, and the status bar says which and why.
- **Settings in layers, and per language.** A setting comes from its default, then your
  settings.json, then the project's .code-arena/settings.json, then its .vscode/settings.json. A
  `"[python]": {…}` block in any of them wins for that language.
- **Settings files are followed live.** When a settings file is saved in the window or changed on
  disk, the window has the new values within half a second, also while the agent is answering. A
  changed colour theme recolours the window at once.
- **Open your settings as JSON.** Preferences: Open User Settings (JSON) and Preferences: Open
  Workspace Settings (JSON) open them as tabs, edited and saved like any file; your comments stay.
- **The chat's box takes several lines.** Shift+Enter starts a new line and the box grows with
  them; Up and Down bring back what you sent before (kept per folder, with the terminal's
  history); what you typed stays when you look elsewhere. With no thread yet, the box stands in the
  middle of the view under "What should we work on?" with first messages to start from.
- **One quick pick for every picker.** Pickers and input boxes share VS Code's look: a title,
  group headings, descriptions and details, live preview of the active row, a free-text row such as
  'Go to <path>', validation messages and a password mode with dots. Escape cancels.
- **Dialogs over the window.** Questions such as the Explorer's delete confirmation ask with
  VS Code's buttons: the default one, a destructive one, Escape to cancel and an optional checkbox.
- **Notifications.** Info, warning and error toasts appear in the bottom-right corner, three at
  most, with action buttons. They go away on their own unless they need an answer, and they stay
  in a list. The status bar's bell opens that list, shows a dot when there is something new, and
  has Clear All.
- **Menus.** Right-click menus have separators, checks, radios, submenus, key hints and keyboard
  navigation. The Explorer's context menu is one of them now.
- **Trees for long lists.** A virtual tree for views with many rows: arrow keys, Home and End,
  selecting several rows, folding, and actions on the row under the pointer.
- **Tool call cards.** Each call in an answer is a card with its tool's icon, its name and main
  argument, and where it stands: Waiting for you, Running with its seconds, a spinner and a bar, N
  of M done, Not allowed, Failed, Not run, or its count and time. Click a card to see what it was
  asked with (short values listed, long text as highlighted code, the rest as JSON) and its result:
  a file as code, search hits as links that open, records as a table you can sort, command output,
  or plain text, with '…' when the result was cut. A call that waits for you opens by itself and
  asks 'Allow TOOL to run with these arguments?'.
- **Tool runs fold into one line.** Two or more finished calls in a row fold into 'Used 5 tools ·
  read_file ×3, grep, run_shell', which opens to show their cards.
- **Open-file chip.** A read_file, edit_file or write_file call has a chip under it, such as
  'src/a.ts:12-14', that opens the file at those lines.
- **Tool cards in history.** A thread read back from the session shows each call's arguments and
  result, and a call that never ended shows as Not run.
- **Editor decorations.** Other parts of the window can now mark a file's text the way VS Code does:
  a glyph in the glyph margin that you can click; line backgrounds; colours, squiggles by severity,
  underlines and lines through on ranges; inline hints, end-of-line messages and ghost text; and
  marks in the overview ruler at the right edge.
- **Hovers on marks.** Resting the pointer on a marked range shows its message under the line after
  a short delay, and resting it on a glyph shows the glyph's title. The hover closes when the
  pointer moves off it, the editor scrolls or the caret moves.
- **Tables in answers.** A Markdown table in an answer is drawn in bordered columns, each as wide as
  its widest text. Click a header to sort by that column: ascending, then descending, then back to
  the order written. Numbers sort as numbers and empty cells go last. A table of more than 8 rows
  has a 'Filter rows' box.
- **Code blocks with a bar.** Each fenced block in an answer has a bar naming its file
  (```` ```ts:src/app.ts ````) or its language. The bar has Copy (says 'Copied'), Wrap Lines (on;
  turned off, the block scrolls sideways with line numbers) and Save as a File (asks for a path, and
  asks again before replacing a project file). When the block names a project file, it also has
  'Open NAME in the editor'.
- **Clickable links and file citations.** In an answer, links to the web open in the system's
  browser. Project files cited as `src/app.ts:12`, `:12-30`, `#L12`, `Program.cs(44,13)`, in
  relative links, in inline code or in plain text open in the editor at that line.
- **Selectable answer text.** Drag over an answer's text to select it, Ctrl+C copies it, and Ctrl+A
  selects the whole paragraph.
- **Replace in the editor.** Ctrl+H, or the chevron at the find widget's left, opens the replace
  row. Enter or Ctrl+Shift+1 replaces the selected match and goes to the next one. Ctrl+Alt+Enter
  replaces every match as one undo step. Preserve Case is Alt+P. With Use Regular Expression on, the
  replace text understands $1..$99, $& and $0, $$, \n and \t.
- **Whole word and Find in Selection.** In the find widget, Alt+W matches whole words only, and
  Alt+L limits the search to the selection (or to the caret's line). Alt+C and Alt+R toggle Match
  Case and Use Regular Expression. Each toggle has a tooltip that shows its key.
- **Find history and seeding.** Up and Down in the Find and Replace boxes bring back earlier
  entries. Ctrl+F with nothing selected starts from the word at the caret. Tab and Shift+Tab move
  between the two boxes, and workbench keys such as Ctrl+S still work while a box has focus.
- **Request bubbles show what came with them.** A request shows the words you typed, without the
  text attached with @. It also shows the part of an answer it replies to: two lines, with Show
  all/Show less and Go to the quoted answer, which scrolls to that answer and flashes it. Under the
  bubble, 'Sent with it' has a chip for each file and its lines, and clicking one opens the file at
  those lines. A skill run with /name gets a 'Used the skill name' card. A message the running
  answer read at its next step says 'Sent while it worked: read at its next step'.
- **Code Arena's own notes show in the thread.** A goal's next turn or a loop's report is now a
  centred 'Code Arena: …' line, where before it was dropped.
- **Request details are kept.** A session stores each request's details (as typed, quote, files,
  skill) when it is sent and sends them again with its messages, so a window that reconnects or
  opens the session later draws the same bubble.
- **The model's thinking, as VS Code shows it.** While the model thinks, its answer shows an open
  'Thinking… 12 s' block that ticks and keeps its newest words in view. When the answer begins it
  folds to 'Thought for 12 s' (', cut short' if it was cut), and a chat you open again shows
  'Thought process'.
- **Skip thinking.** A 'Skip thinking' pill on the thinking block tells the model to answer now; the
  block then says 'Answering now…'.
- **Answer footer.** Under a finished answer: Copy answer, the model, '· 12 s', and '· 8.6K in (73%
  cached) · 1.2K out' (exact counts in its tooltip). The session keeps it, so the footer is still
  there after a reload.
- **Pictures open in a tab of their own.** PNG, JPEG, GIF (first frame), WebP, BMP and ICO files
  show on a checkerboard, fitted to the editor. Click the picture (or the zoom item in the status
  bar) to see it at its own size and scroll it. The status bar shows its size in pixels (W × H) and
  in bytes, and the picture is drawn again when the file changes on disk.
- **Sounds and videos open in a tab with their name and size.** MP3, WAV, OGG, OPUS, FLAC, M4A, AAC,
  WEBA, MP4, WebM, OGV and MOV files have an 'Open in the system player' button.
- **An SVG has a preview.** It still opens as text, and the Open Preview button in the tab bar shows
  the drawing in its own colours, next to the text.
- **A file that cannot open says why in its tab.** The tab reads '<name> is not text (or not
  UTF-8)', '<name> is larger than 5 MB', or the read error, where before only the status bar
  mentioned it. Such tabs are kept with the layout like the others.
- **The chat follows only while you read its end.** Scrolled up more than 80 px, the thread stays
  put while the answer streams, and a 'Jump to the latest' button takes you back down.
- **Show earlier.** A long session opens on its last 30 turns. 'Show earlier (N more turns)' at the
  top brings 30 more each time and keeps your place.
- **How the turn stands, under the thread.** 'Working…' until the answer begins, a blinking block
  while it is written, and 'Waiting for the model…' between steps. Notices during a turn (retries,
  waits) are warning lines above the answer.
- **A page reloaded mid-turn sees the turn from its start.** Instead of a silent 'busy', it gets the
  thread before the turn and then everything the running turn has said so far.
- **An edit shows what it changed.** Under each edit in the chat, 'Changes to PATH' lists the changed
  lines with 3 lines around them. Each line has its old and new number, removed lines are red and added
  lines green, '⋮' marks skipped lines and '… N more changed lines' ends a long list. The edit also shows
  'new file' and its +N −M. It stays open while the turn runs. Clicking the path opens the file at its
  first change, and clicking a new line number opens that line. An answer whose only call is a
  successful edit shows just its diff.
- **N files changed.** Under a finished turn's last answer: the files its edits changed, each listed once
  with its +/− counts and the totals. Clicking a file opens the agent's compare of it.
- **/ menu of skills and commands in the chat.** Type / at the start of the chat box (or after a
  /name already chosen) to see "Skills and commands": the project's and your commands with their
  argument hints, and the skills. Names starting with what you typed come first, then names that
  contain it. A command hides a skill of the same name. ↑/↓ choose, Enter or Tab puts '/name ' in
  the box, and Enter on a name typed in full sends it. Esc closes the list; with the list closed,
  Esc drops the chosen /name and keeps your words.
- **Several skills in one message.** '/review /tests cart.rs' sends each named command or skill for
  the same words, one after the other.
- **@ mentions in the chat box.** Typing `@` lists the project's files and the folders they are in,
  ranked as Ctrl+P ranks them. You get 50 at a time ('Show N more', or Down past the last row), 'N of M
  matches', and a note when the project has more files than were listed. Up/Down choose, Enter or Tab
  puts `@path ` in the box (`@"a b.txt" ` for names with spaces), and Esc closes the list. A line range
  you type (`@cart.rs:12-30`) is kept and sends only those lines. Only typing opens the list.
- **Browse any folder with @.** A whole path after `@` (`/`, `~/`, `C:/`) lists that folder on this
  machine. Choosing a folder goes into it; choosing a file puts its whole path in the box.
- **Chips for what mentions sent.** Under a question, a chip names each file, folder or line range its
  mentions sent. Clicking a file's chip opens it at the first of its lines.


### :bug: Bugs fixed

- **Answers are drawn in the native window again.** An answer's text lived in signals that could go when the next
  message came in, so it streamed into nothing; a fast answer could even stop the window. The thread is no longer
  built again at each message, and a group's view no longer at each change of the layout.
- **What you change while the agent answers is no longer lost.** A model, mode, thinking level or
  setting (a colour theme too) chosen in the IDE while an answer is being written is applied when
  the answer ends, in the order you chose them.
- **A message sent as the answer ends is answered.** What you send in the IDE too late for the
  running turn to read now runs as a turn of its own right after it (after Stop too), under its own
  question, instead of waiting for your next message.
- **A command sent while the agent answers runs when the answer ends.** `/name` typed in the IDE
  during an answer is no longer read by the running turn as plain words: it waits, and runs as its
  own turn once the answer has ended, as in the terminal.
- **The IDE says why it cannot do something yet.** Going back to an earlier question, asking again
  or compacting while an answer is written now says "Wait for the answer, or stop it, to ..."
  instead of doing nothing.
- **Ctrl+K Ctrl+T opens the theme picker however you press it.** In the native window the chord no
  longer breaks when Ctrl is let go between its two keys.
- **A message that waited for the answer is shown once (native window).** What you send too late for
  the running answer, or a `/command` sent meanwhile, shows as the question of its own turn in place
  of the bubble that waited, not twice. (A command Code Arena does not know keeps its waiting
  bubble.)
- **A single change to the layout is remembered.** In the native window the first change after
  opening a folder (the panel hidden, a side bar resized, a tab opened) was forgotten at the next
  start unless a second change followed; it is now kept.
- **Tool cards read cleanly.** The wrapped 'Allow TOOL to run with these arguments?' line and a
  call's console output no longer draw their lines over each other.
- **An answer's words around a call stay apart.** What the agent says after a call starts a
  paragraph of its own instead of running on from what it said before it ('first.Done.').
- **Saving a code block into a new folder works.** Save as a File from the chat makes the folders
  of the path first, where it used to fail.

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
