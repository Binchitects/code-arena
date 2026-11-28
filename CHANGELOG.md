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
  ranked as Ctrl+P ranks them. You get 50 at a time ('Show N more', or Down past the last row), 'N
  of M matches', and a note when the project has more files than were listed. Up/Down choose, Enter
  or Tab puts `@path ` in the box (`@"a b.txt" ` for names with spaces), and Esc closes the list. A
  line range you type (`@cart.rs:12-30`) is kept and sends only those lines. Only typing opens the
  list.
- **Browse any folder with @.** A whole path after `@` (`/`, `~/`, `C:/`) lists that folder on this
  machine. Choosing a folder goes into it; choosing a file puts its whole path in the box.
- **Chips for what mentions sent.** Under a question, a chip names each file, folder or line range
  its mentions sent. Clicking a file's chip opens it at the first of its lines.
- **Model picker that describes each model.** The chat's model menu shows each model's window,
  longest answer and whether it uses tools, thinks or sees images (for example '128K context ·
  answers up to 16K · Tools · Thinks'). Models that cannot be loaded are dimmed. The thinking picker
  shows only while the model thinks.
- **Pickers work during a turn.** You can change the mode, the model or the thinking while an answer
  is being written. The pill shows your choice with a watch icon, the menu says 'Applies to the next
  turn', and the choice takes effect when the turn ends. Mode menu rows say what each mode lets run;
  Yolo is drawn in red and warns when chosen, Plan in the accent colour.
- **Agent header and More menu.** A '…' button over the chat opens its More menu (Open in a New
  Window, Hide the Chat, and what other parts add). Goal, Tasks (with a count of running jobs) and
  History buttons appear once their commands and views exist. The palette has Chat: Compact the
  Conversation, Chat: Hide the Chat, Chat: Show Tasks and Sub-agents, and Chat: Show This Folder's
  Sessions.
- **Change or take back what you sent while it works.** Each message sent while an answer is written
  waits in its own bubble at the thread's end ('Read at the next step', or 'Waits for the answer to
  end' for a command). Hover it to Edit it in place (Enter keeps the words, Escape leaves them) or
  Cancel it. Clear all takes them all back. One the answer already read says 'The message could not
  be changed.'
- **A message the answer read becomes its question.** Read at the next step, the bubble turns into a
  question marked 'Sent while it worked'. Read after the answer, its question takes the bubble's
  place. Matching is by number, not words, so two messages with the same words, or one changed
  since it was sent, are never mixed up. A window that comes in mid-answer sees the queue as it is
  now.
- **Editor groups.** Split the editor into up to eight groups, side by side or one under the other
  (Split Editor Ctrl+\, Split Editor Right/Left/Down/Up, Split Editor in Orthogonal Direction
  Ctrl+K Ctrl+\, Toggle Vertical/Horizontal Editor Layout Alt+Shift+0). Each group has its own tabs
  and its own tab shown, and a file open in two groups is one document. Drag the divider, move it
  5% with the arrow keys, or double-click it to share the room evenly; each group keeps at least
  10%. A group whose last tab closes goes.
- **Move between groups.** Move Editor into Next/Previous Group (Ctrl+Alt+Right/Left; past the last
  group it makes a new one), Focus First..Eighth Editor Group (Ctrl+1..8; one past the last splits),
  Focus Next/Previous/Left/Right/Above/Below Editor Group, Join All Editor Groups and Close Editor
  Group. Save, Close Editor and Find act on the group you are in.
- **Open to the Side and Show All Editors.** Open a file from the Explorer's menu in the group
  beside the one you are in. Show All Editors By Appearance (Ctrl+K Ctrl+P) lists every open editor
  under Group 1, Group 2 and so on.
- **Groups come back after a restart.** The groups, their tabs, the tab each shows, their sizes, the
  layout, the group you were in, and where each tab's caret and scroll were come back after a
  reload.
- **Copy, Edit, Answer again and Rewind to here on a question.** With the pointer over one of your
  questions in the Agent view you can copy its words. Edit asks it again in new words from a box that
  keeps its quote. Answer again gives the same answer, or a shorter or longer one, or one from another
  model or with other thinking (for that answer only), and can put the files back as they were. Rewind
  to here says how many messages go and which files would be put back or removed, and lets you take back
  the conversation, the code or both. Afterwards the question's words are back in the box.
- **Bracket colours and matching.** Brackets are coloured by how deeply they are nested, using the
  theme's editorBracketHighlight colours (an unexpected bracket shows in red), and the bracket at
  the caret is highlighted with its pair, or the pair the caret is inside (editor.matchBrackets,
  editor.bracketPairColorization.enabled).
- **The word and the selection found again.** Every place of the word at the caret is highlighted
  (editor.occurrencesHighlight), and so is every place of the selected text
  (editor.selectionHighlight).
- **Find matches marked.** All of the find widget's matches are highlighted, the current one in its
  own colour, and they show in the overview ruler.
- **Odd Unicode marked.** Invisible characters, and characters that could be taken for ASCII ones,
  are underlined outside comments, with VS Code's hover saying which character it is
  (editor.unicodeHighlight.ambiguousCharacters).
- **Links in the editor.** Hold Ctrl (Cmd on macOS) over an address or a project file path to
  underline it. Ctrl+click opens it: an address in the system browser, a file in the editor at its
  line.
- **Suggestions at the caret.** Typing a word's letters (editor.quickSuggestions) or pressing
  Ctrl+Space opens VS Code's suggest widget. It lists the words of the open documents
  (editor.wordBasedSuggestions), each row with its kind's icon and the typed letters lit. Up, Down,
  PageUp and PageDown move; Enter (editor.acceptSuggestionOnEnter: on, smart, off), Tab or a click
  takes one; Escape closes it. Ctrl+Space while it is open shows the focused suggestion's details
  beside it. With the widget closed, Tab completes the word as editor.tabCompletion says.
- **Ghost text in the editor.** An inline suggestion can be shown at the caret in grey
  (editor.inlineSuggest.enabled). Tab takes it and Escape drops it, as does moving the caret away.
- **Code folding.** Fold and unfold regions as in VS Code. A region is a block of deeper indentation
  or a `#region`/`#endregion` pair. Click the gutter chevrons (shown on hover, and always on a
  folded line), or use Fold (Ctrl+Shift+[), Unfold (Ctrl+Shift+]), Toggle Fold (Ctrl+K Ctrl+L), Fold
  All (Ctrl+K Ctrl+0), Unfold All (Ctrl+K Ctrl+J), Fold/Unfold Recursively (Ctrl+K Ctrl+[ / ]) or
  Fold Level 1-7 (Ctrl+K Ctrl+1...7). A folded line shows ⋯ on the fold background. Folds stay with
  each file while the window is open and move as you edit. `editor.folding` turns folding off.
- **Tabs work like VS Code's.** A file clicked in the Explorer opens as a preview that the next
  click replaces. A double click, an edit, a drag or Keep Open (Ctrl+K Enter) keeps it. Pin a tab
  (Ctrl+K Shift+Enter) to put it first and keep it from closing with the others. A middle click
  closes a tab. Drag a tab to reorder it, move it to another group, or drop it on a group's edge to
  split.
- **Tab menu and close commands.** The tab's menu has Close, Close Others, Close to the Right, Close
  Saved, Close All, Copy Path, Copy Relative Path, Reveal in Explorer View, Keep Open, Pin/Unpin,
  Split and Move to Group. Closing several tabs asks once about every unsaved file (Save, Don't
  Save, Cancel).
- **Move between editors.** Go Back and Go Forward (Alt+Left/Right), Reopen Closed Editor
  (Ctrl+Shift+T), Next and Previous Editor (Ctrl+PageDown/Up, Ctrl+Alt+PageDown/Up), Alt+1..9, Move
  Editor Left/Right (Ctrl+Shift+PageUp/Down). Copy the active file's path with Ctrl+K P, or its
  relative path with Ctrl+K Ctrl+Shift+C.
- **Editor settings applied live.** The editor follows `editor.fontFamily`, `fontSize`, `fontWeight`,
  `lineHeight`, `wordWrap` (off, on, wordWrapColumn, bounded), `rulers`, `lineNumbers` on/off,
  `renderWhitespace`, `renderLineHighlight`, `cursorBlinking` solid, `guides.indentation`,
  `scrollBeyondLastLine` and `emptySelectionClipboard` as soon as a settings file changes, a
  language's `[lang]` block first, in file tabs and in the person's settings files alike.
- **Toggle Word Wrap and font zoom.** Alt+Z wraps or unwraps the file on screen; View: Editor Font
  Zoom In, Out and Reset (and Ctrl with the mouse wheel when `editor.mouseWheelZoom` is on) zoom
  the editor's font in 10% steps.
- **View toggles write the setting.** Toggle Minimap, Toggle Sticky Scroll, Toggle Breadcrumbs and
  Toggle Render Whitespace write the person's settings.json, and the breadcrumbs follow
  `breadcrumbs.enabled`.
- **VS Code's editing commands in the editor.** Toggle Line Comment (Ctrl+/; Add and Remove with
  Ctrl+K Ctrl+C / Ctrl+K Ctrl+U) and Toggle Block Comment (Shift+Alt+A) use each language's own
  tokens. You can also use Move Line Up/Down (Alt+Up/Down), Copy Line Up/Down (Shift+Alt+Up/Down),
  Delete Line (Ctrl+Shift+K), Insert Line Below/Above (Ctrl+Enter, Ctrl+Shift+Enter), Indent/Outdent
  Line (Ctrl+], Ctrl+[, or Tab and Shift+Tab on selected lines) and Go to Bracket (Ctrl+Shift+\).
- **Multiple cursors and smart select.** Add Cursor Above/Below (Ctrl+Alt+Up/Down), Add Next
  Occurrence (Ctrl+D), Select All Occurrences (Ctrl+Shift+L), Add Cursors to Line Ends (Shift+Alt+I)
  and Expand/Shrink Selection (Shift+Alt+Right/Left).
- **Brackets and quotes close themselves.** As `editor.autoClosingBrackets` and
  `editor.autoClosingQuotes` say, a closer can be typed over, a selection is wrapped in the pair,
  and Backspace between a pair deletes both. Enter keeps the line's indent and indents after an
  opened bracket, as `editor.autoIndent` says.
- **Edit, Selection and right-click menus that work.** The Edit and Selection menus now run their
  commands, and right-clicking in the editor offers Cut, Copy, Paste and Command Palette....
- **Several rows selected in the Explorer.** Ctrl+click (Cmd on macOS) adds a row, Shift+click and
  Shift+arrows select a range, Ctrl+A selects all; Delete asks once for all of them.
- **Cut, Copy and Paste in the Explorer.** Ctrl+X, Ctrl+C and Ctrl+V (also in the row's menu) move
  or copy files and folders into the folder selected; a copy whose name is taken is named 'name
  copy.ext', 'name copy 2.ext'; open tabs follow a move; a folder is never pasted into itself.
- **Drag and drop in the Explorer.** Drag rows onto a folder, or below the rows for the top, to move
  them after VS Code's question ('Do not ask me again' turns off explorer.confirmDragAndDrop); hold
  Ctrl (Alt on macOS) to copy. Files dropped from the system are copied in, asking before one is
  replaced.
- **Auto reveal.** The file the editor shows is selected in the Explorer with its folders opened
  (explorer.autoReveal), and File: Reveal Active File in Explorer View does it on demand.
- **Open in Integrated Terminal** from a row's menu starts a shell in that folder.
- **Restore a deleted file.** The notification after deleting one row has a Restore button that puts
  it back.
- **Explorer rows say more.** File kind icons, a '…' row while a folder loads, the reason a folder
  could not be listed, and ↪ after a link.
- **Search options.** The Search view has Match Whole Word next to Match Case and Use Regular
  Expression, and its "…" button shows files to include and to exclude (comma-separated globs, as
  VS Code reads them: `*.rs`, `src`, `./src`). It searches 300 ms after you stop typing, says
  "Searching…", then "N results in M files" and a note when the results were cut. It lists every
  match on its own row with its line number and cuts a long line's start so the match shows. You
  can fold each file, and Collapse All, Refresh and Clear Search Results are in the view's title.
- **Replace in files.** Ctrl+Shift+H (or the chevron) shows the replace box. Replace All asks
  "Replace N results in M files?" and leaves files with unsaved changes as they are. You can also
  replace one file's matches or one match from its row. With a regex, `$1` puts a group back. Line
  endings stay as they were, and open tabs without changes take in the new text.
- **A search result opens as a preview with its match selected.** Find in Folder... in the
  Explorer's menu fills in files to include, and Edit > Find in Files and Replace in Files are in
  the menu. The query and options are kept with the folder and come back at the next start.
- **Source Control view.** Ctrl+Shift+G shows the repository's changes in Merge Changes, Staged
  Changes and Changes, each with its count. A row shows the file's name, its folder and git's
  coloured letter, and a deleted file is struck through. Hover a file or a group to Open File,
  Stage, Unstage or Discard. Discard asks first, and a file git does not track is kept aside rather
  than lost.
- **Commit box.** Type a message and press Ctrl+Enter, or click 'Commit N staged' ('Commit all' when
  nothing is staged, which stages everything first). Commit (Amend) is in the button's menu. A toast
  says 'Committed <hash>' or what git refused.
- **Changes tabs.** A click on a changed file opens 'name (Working Tree)', the working tree against
  the index. A staged file opens 'name (Index)', the index against HEAD. A file in conflict, or one
  git does not track, opens as it is.
- **Branch and badge.** The status bar shows the branch and the Source Control icon shows the count
  of changes. Both follow commits, checkouts and staging done in a terminal, since the driver
  watches git's own files. A folder without a repository offers Initialize Repository.
- **Git commands.** Git: Commit, Commit (Amend), Refresh, Stage All Changes, Unstage All Changes,
  Discard All Changes and Initialize Repository are in the palette.
- **Accept or revert the agent's changes.** In the Changes view, pointing at a file shows Open File,
  Revert and Accept. Revert asks first and says what it will do: a file the agent made is deleted,
  any other goes back to how it was. The view's title has Accept All Changes
  (codeArena.acceptAllChanges, also in the palette), Refresh, View as Tree or View as List, and
  Collapse All.
- **A menu on each changed file.** Right-click a file in the Changes view for Open Changes, Open
  File, Add to Chat, Explain These Changes, Fix These Changes, Accept, Revert, Copy Path and Copy
  Relative Path.
- **The agent's changes counted.** The status bar shows 'Agent changes: N files' and a click opens
  the view. The view's activity bar icon has the count as its badge. The list is asked for again
  when the view is shown and after each write by the agent, and an open diff tab closes once its
  file is accepted or reverted.
- **The Explorer's file menu matches VS Code's.** Right-clicking a file puts Open to the Side first.
  New File and New Folder now appear only on folders and on the empty part of the tree.
- **Find in Files starts from your selection.** Ctrl+Shift+F and Ctrl+Shift+H put the text selected
  on one line into the search box. Replace in Files puts the cursor in the search box unless that
  box was just filled from a selection.
- **Long lines scroll sideways.** A line wider than the editor now scrolls into view with the caret,
  with a horizontal scrollbar, instead of being cut off at the editor's edge.
- **Ctrl+PageUp and Ctrl+PageDown switch tabs in the editor.** They used to scroll a page there.
- **Sessions view.** The side bar's Sessions view lists this folder's sessions newest first under
  Today, Yesterday, Last 7 Days, Last 30 Days and Older. The one you are in has a dot that beats
  while an answer is written, a cloud marks a session kept with Arena, and 'not sent' shows until
  Arena has all of it. Rest the pointer on a row to see its title, message count and id, click it to
  take the session up, or use Fork on hover. The search box finds earlier sessions by their words.
  The foot shows the folder, the branch and the version.
- **New and forked sessions.** New Session (the + in the Sessions view, the chat's More menu, or
  `/clear` and `/new` in the input) starts a fresh session and keeps the last one. It stays on the
  same model, mode and thinking. Fork Session copies all of the session, and Fork from a Question...
  copies what came before the question you pick. While an answer is written these wait or say why.
- **Terminal links.** Ctrl+click a project path in terminal output (`src/main.rs:12:5`,
  `./src/a.rs`, or the full path under the project folder) to open it at that line and column.
  Ctrl+click a web address to open it in the browser. The link under the pointer is underlined.
- **Terminal settings apply live.** `terminal.integrated.fontFamily`, `fontSize`, `lineHeight`,
  `cursorStyle`, `cursorBlinking`, `scrollback`, `minimumContrastRatio` and `sendKeybindingsToShell`
  take effect as soon as they change, and the screen is fitted again to the new font.
- **Terminal tabs and panel.** Tabs read 'bash 1', 'bash 2' and gain '(ended)' when the shell exits.
  The panel then says 'The shell ended with exit code N.' A middle-click on a tab ends that
  terminal. The screen has a draggable scrollbar, the panel has a 'Hide the panel' chevron, and the
  status bar has a terminal toggle.
- **The session's own terminals show at once.** A terminal the session starts for a task, a run or a
  debuggee appears in the panel straight away. If the window falls behind the session's messages, it
  redraws every terminal from what it last showed.
- **Terminal shells.** A terminal runs the shell it is asked for, else `$SHELL`, else bash, else sh.
  It sets `TERM_PROGRAM=code-arena`, does not pass `ARENA_API_KEY` on, and at most 10 of your own
  terminals run at once.
- **Arena's colour themes and VS Code's most used.** Code Arena, Sakura, Midnight, Copper, Forest,
  Ocean and Lavender, each dark and light, beside One Dark Pro, One Light, Dracula, Alucard, GitHub
  Dark and Light, Nord, Nord Light, Monokai Light and Dark/Light (Visual Studio). A theme name
  copied from VS Code or an extension (Default Dark Modern, GitHub Dark Default, Atom One Light)
  reads as the theme here it stands for, and an unknown name gets the nearest one. In every theme
  the line numbers, suggestions and links are 4.5:1 or more on their background.
- **The theme follows the system.** With window.autoDetectColorScheme on (the default), the window
  wears workbench.preferredDarkColorTheme or workbench.preferredLightColorTheme, so a new window is
  Code Arena Dark. A workbench.colorTheme kept before now only counts once you pick a theme, which
  turns following off.
- **Color Theme picker as VS Code's.** Ctrl+K Ctrl+T (or Ctrl+K Ctrl+Alt+T) lists Follow the System,
  then the light, dark and high contrast themes, Code Arena's before yours, with the current one
  marked. The arrow keys show each theme at once, Escape goes back, and choosing one keeps it.
  Preferences: Toggle between Light/Dark Themes switches to the same theme's other mode.
- **Theme button in the activity bar.** It offers Light, Dark and System, plus a Color Theme
  submenu of the theme families. Choosing a family makes its two modes the preferred ones.
- **A refused setting is said.** When a setting can't be kept, a toast says why.
- **Activity bar menus.** Right-click an activity bar icon to hide it, move it to the panel, move it
  up or down, or dock it (Dock, Dock With every group, Float). Right-click the empty part of the bar
  to show or hide each view's icon, set the bar's position, or reset view locations.
- **Activity bar position.** Put the activity bar at the side, as a row at the top of the side bar,
  or hide it. Use the bar's menu or View: Move Activity Bar to Side/Top, Hide Activity Bar, or
  Toggle Activity Bar Visibility. The position and hidden icons are kept with the folder's layout.
- **Reorder by dragging.** Drag an icon within the activity bar to reorder it; a line shows where it
  goes. Drop a view from anywhere onto the bar to put it in the side bar at that place.
- **Panel tab menu and position.** Right-click a panel tab to move the view to the side bar or move
  the whole panel to the bottom, left or right (also View: Move Panel Left/Right/To Bottom). The
  panel keeps its size. Tabs that do not fit go under a '…' menu.
- **Activity bar foot buttons.** Terminal, Color Theme, Accounts, Manage, Help and About. Manage
  opens Command Palette, Color Theme and About until a part adds the Manage command.
- **Zen mode.** Ctrl+K Z (View: Toggle Zen Mode, View > Zen Mode) hides every part but the editor
  and goes full screen; pressing it again shows exactly what was there before, and a restart while
  in Zen shows the layout from before it.
- **Full screen.** F11 (View: Toggle Full Screen) fills the monitor and hides the title bar; F11
  again puts the window back where it was, maximized if it was. Where the window can't be placed
  (Wayland), it is maximized instead.
- **Panel position.** View: Move Panel Left, Move Panel Right, Move Panel To Bottom (also in the
  View menu, dimmed where the panel already is) put the panel beside or under the editor, keeping
  its tabs.
- **Title bar menus by keyboard.** Alt+F/E/S/V/G/R/T/H and F10 open the menus; the arrows, Home/End,
  Enter and Escape work in them, Left/Right go to the next menu, and the keys go back to the editor
  when the menu closes. Menus open under their own titles. Go > Go to File, Terminal > New/Kill
  Terminal and Help > Show All Commands now run their commands.
- **Window title.** The window is called '<folder> · Code Arena'.
- **Ctrl+0** (View: Focus into Primary Side Bar) shows the primary side bar.
- **A cheat code.** Type the Konami code where no box has the keys.
- **Quick Open modes.** Type `:12` or `:12:5` to go to a line and column of the file shown (Go to
  Line/Column, Ctrl+G, or click Ln/Col in the status bar). Type `path:12[:5]` to open a file at a
  line, `@` to list the file's symbols (Go to Symbol in Editor, Ctrl+Shift+O; `@:` groups them by
  kind), and `?` to list the modes. Ctrl+Enter opens a file to the side.
- **Recently opened and recently used.** With nothing typed, Quick Open lists the files you opened
  last first. The Command Palette lists the last 20 commands you ran under 'recently used', and both
  lists are kept with the folder. The palette also finds commands by id or by words in any order,
  and opens with F1 too, from the editor and the terminal.
- **Go menu.** Go to File, Go to Symbol in Editor and Go to Line/Column are now in the Go menu.
- **Breadcrumb pickers.** Click a folder in the breadcrumbs to list its files and open one ("No
  files here, only folders." when there are none). The file's part lists the files beside it, and
  each symbol part lists the symbols beside it with their kinds. Type to filter a list; Up, Down and
  Enter choose a row; Escape closes it.
- **Focus Breadcrumbs keys.** Focus and Select Breadcrumbs (Ctrl+Shift+.) and Focus Breadcrumbs
  (Ctrl+Shift+;) give the bar the keys. Left and Right move between parts, Enter, Space or Down open
  a part's list, and Escape goes back to the editor. `breadcrumbs.symbolPath` hides the symbols.
- **Outline filter, Collapse All and signatures.** The Outline has a Filter box that keeps the
  symbols matching it and the symbols they are in, a Collapse All button in its title, and a
  tooltip on each row with the symbol's definition line.
- **Plan view.** The Plan view shows the agent's working plan: its title, 'N/M done', goal and steps
  (Done, Doing, Blocked, To do) with 'Done when: `check` · passed|failed' (output on hover), 'after
  1, 2', file chips, the symbols it changes, impact and notes; Clear Plan in its title.
- **Plan review in plan mode.** A proposed plan shows what a second reading said, then Approve,
  Approve, asking before each change, Another Way... and Reject... (each asks why), and Change step
  N on each step; the review is said to the agent as the next turn.
- **Undo a step, carry on an earlier plan.** 'Undo step N' puts a begun step's files back (after a
  confirmation, with a toast); with no plan, the view offers to carry on the plan an earlier session
  of the folder left unfinished.
- **Plan bar.** The plan's bar over the input shows a progress bar and 'waiting for your review',
  and opens to the steps with a click.
- **Compare editor.** Compare Active File with Saved (Ctrl+K D) or with Clipboard (Ctrl+K C), or pick
  two files with Select for Compare and Compare with Selected (from the tab or Explorer menu). The
  compare opens side by side as in VS Code: matching lines line up, hatched rows fill the gaps,
  changed words are marked, and both sides scroll together. When the editor is narrow it switches to
  the inline view. The file's own side can be edited and saved with Ctrl+S, and the arrow by each
  change reverts it. The toolbar shows the lines added and removed, and has Previous/Next Change
  (Shift+Alt+F5, Alt+F5), the inline toggle, Show Whitespace Differences, Swap Sides and Open File.
- **Agent changes open in the compare editor.** Opening a change from the Changes view shows the file
  before the agent beside the file itself, with Revert and Accept in the toolbar.
- **Context ring.** A ring with the share used sits before Send in the chat's box, amber near the
  compaction threshold and red at it. Click it to see what fills the window: a bar by kind with the
  threshold, each kind with the room kept for the answer and what is free, the biggest messages, who
  set the threshold, What fills it, and Compact now.
- **Context view.** The Context tab lists what fills the next request part by part. Each part opens
  to its pieces: each tool's definition, each message. It also says how the tokens were counted and
  what could be freed, with Compact now. Agent: Show the Context and Agent: Compact Now are in the
  palette.
- **Compaction settings.** The status bar's Context item (USED / LIMIT, amber at the threshold)
  opens the settings. Compact at a share of the window (20-95%, default 80), only when a request
  would not fit, or as your Arena account says. Keep the recent part within 5% to ten points under
  the threshold (default 25). The settings are checked, have Defaults, apply live and are kept in
  settings.json (also: Agent: Compaction Settings...).
- **Corner bubbles while you look elsewhere.** When the window is not in front or the Agent view is
  hidden, Code Arena tells you in the bottom-right corner (up to four bubbles). 'Waiting for you:
  TOOL' can be answered there with Allow, Always or Deny. 'Ready: the agent finished' has a Reply
  box and Send. 'The turn failed' shows the error. Ready and failed bubbles close after 20 s, and a
  click on a title opens the Agent view. A call answered anywhere is not asked again.
- **Desktop Notifications.** A new checked entry in the Manage menu (setting
  `codeArena.desktopNotifications`, off by default). While the window is minimized, notifications go
  to the desktop through notify-send, osascript or PowerShell. The window asks for attention when a
  bubble comes while it is not in front.
- **Add to chat from the editor.** Code Arena: Add to chat (Ctrl+L, or the editor's right-click
  menu) puts the selection, or the caret's line, in a chip over the chat's box ('src/cart.rs:12-30',
  '(unsaved)' when the file has changes not saved). X leaves it out, and adding the same lines again
  replaces their chip; the box keeps 8 at most. A dashed '+ path:line' chip offers the editor's
  current lines. Lines sent with no words go as 'Look at these lines.', and they come back to the
  box if the message is refused.
- **Explain this, Fix this, Complete this.** From the editor's right-click menu, each asks about the
  selected lines at once (queued while an answer is written) and leaves the chips in the box alone.
- **The model reads the lines it is sent.** Each piece goes with three lines around it, numbered as
  read_file numbers them; an unsaved piece goes as the person sees it. The pieces appear under the
  question as 'Sent with it' and are kept with it.
- **Add to Chat in the Explorer.** A row's menu puts its @path before what is typed in the chat's
  box.
- **Reply quoting part of an answer.** Select words in a finished answer and a toolbar offers Reply,
  Ask about this and Copy. Reply puts the words over the box as 'Quoted in your reply' (cut to 4,000
  characters; X or Backspace in an empty box takes it away; it comes back if the message is
  refused), and the model reads the quote before your words. Ask about this sends a question about
  the words at once ('Explain this', 'Say more', 'Give an example', or your own), leaving the box as
  it was. Chat: Reply Quoting the Selection is Ctrl+Shift+'.
- **Pictures, sounds and videos a tool made show under its card.** When a tool saves a file under
  `.code-arena/media/`, its card shows a small copy of each picture, and a card for each sound or
  video that opens it in the system's player. While a tool that draws or films runs, its card says
  'Drawing the picture…' or 'Making the video, a few minutes…'.
- **A picture viewer over the whole window.** Click a tool's picture to see it fitted to the window.
  Zoom with + / − / 0, Ctrl+wheel or the bar's buttons, go through the call's other pictures with
  the arrows or the arrow keys, open it in the system's viewer, or Save As to a path in the project.
  Escape closes the viewer.
- **Attach files to a message.** The chat's paperclip (the system's file dialog), a file dropped on
  the box, or files copied in a file manager and pasted with Ctrl+V go up to Arena at once. Each is
  a chip over the box: a picture as itself, another file by its name with 'Uploading N%' and a bar,
  then its size (', cut to fit'), or what went wrong ('Larger than 20 MB.'). Its X takes it away.
- **The model reads what is attached.** A document's text goes after your words between `<uploaded>`
  markers, and a picture goes as an image part to a model that sees. Otherwise the box says 'MODEL
  cannot see images: it gets their names only.' The message waits for every upload, files alone are
  sent as 'Look at these files.', at most ten go with one message, and there is no paperclip without
  Arena.
- **A question shows its files.** Under the bubble: pictures as small pictures, other files as chips
  with their size, kept with the session.
- **Diagrams in answers.** A ```mermaid (or ```mmd) block in an answer is drawn as a diagram where
  it is written, in the window's theme, once its fence is closed. It is drawn by mermaid-cli (the
  project's own or on PATH) or else by a headless Chrome/Chromium with a local mermaid.js. Show Code
  and Show Diagram switch between the drawing and its source, and Open in Browser opens a page that
  draws it. A diagram that does not parse shows Mermaid's error over its code. Where nothing can
  draw diagrams, the block says what to install and Open in Browser still shows it.
- **The approval card says what you are allowing.** Before a call runs, the card's title says what
  it does ('Run this command?', 'Allow edit_file to change this file?'). It says when a command has
  no time limit or runs in the background, and the sentence under the buttons says what 'Always for
  this session' spares you. A call that asks every time (stopping a command, a hook's ask) offers no
  Always.
- **Laya reads commands before they run.** When you are signed in to an Arena that has Laya, the
  card shows Laya's line ('Laya: destructive 96%, outside the workspace 93%, network 3%'). A command
  at 60% or more asks even in yolo mode and past an allow rule, and a -p run refuses it. If the
  Arena has no Laya, or gives no answer within 3 seconds, commands are asked about exactly as
  before.
- **Markdown preview.** Markdown: Open Preview (Ctrl+Shift+V), Open Preview to the Side (Ctrl+K V)
  and Show Source, also from the breadcrumb's Preview button: a 'Preview' tab draws the file with
  the chat's Markdown renderer (tables too), follows its text 150 ms after typing, draws the
  project's pictures, opens linked files (a linked Markdown file in the preview's group), scrolls
  to #headings by GitHub's names and opens web links in the browser.
- **Tasks view.** A Tasks view sits beside the Agent and lists the session's sub-agents, those at
  work first, then those that ended. Each row shows its status, its title, a pencil when it may
  change files, 'as ASSISTANT · now · 12 s · 3.4k tokens · 5 tool calls' and Stop. The header says
  'N at work' and has a help button.
- **A sub-agent's detail.** Open a sub-agent to see what it was asked, its tools, each step's
  thinking, words and calls, what it is doing now, its report and its errors. Stop it while it
  works, or go Back to the list.
- **Sub-agent lines in the thread.** A task call is now one line: its status, its description,
  'Working…' or its report's first line, its time, tokens and model. Clicking it opens the sub-agent
  in Tasks. Calls in a row are grouped as '3 sub-agents · 2 working'.
- **Quick diff in the gutter.** While you type, bars beside the line numbers show the lines changed
  since git's index: green for added, blue for modified and a red triangle where lines were deleted.
  The same marks show on the overview ruler. They are read again after a save, a stage or a commit.
  `scm.diffDecorations` (all/gutter/overview/none) picks where they are drawn. Alt+F3 / Shift+Alt+F3
  (and Alt+F5 / Shift+Alt+F5 outside a compare) go to the next or previous change.
- **Problems view.** See every source's problems (project check, tasks, language servers, security
  review, code review), each in its own section with per-file folds. Rows show the severity icon,
  message, source(code) and [Ln, Col]; a click opens the file at that place. Filter by text, **/*.ts
  or !exclude, and show or hide errors, warnings, infos or everything outside the active file. The
  view counts what the filter hides and has Clear Filters and Collapse All. A row's menu has Copy,
  Copy Message, Fix with Code Arena and Quick Fix....
- **Problems in the editor.** Problems show as squiggles in open files and in files opened later,
  with the message as the hover and a mark in the overview ruler. A light bulb on each marked line
  (or Ctrl+.) offers Fix with Code Arena for that line or for the whole file.
- **Problem counts.** The status bar shows real error, warning and info counts instead of 0, and a
  click shows Problems. The Problems icon in the activity bar has a badge with the total.
- **Fix this sends the problems.** The editor's 'Fix this' now sends the problems marked in the
  selected lines along with them.
- **Extensions from a .vsix.** You can install a VS Code extension from a .vsix file. Code Arena
  reads it without running any of its code and keeps only the files its themes, icon themes,
  snippets, languages, grammars, keybindings and configuration defaults name. A package with a name
  that leads out of it, or with a link in it, is refused whole. A new version replaces the old one
  and keeps it on or off as it was.
- **Import from VS Code.** You can see the extensions that VS Code, VSCodium and Insiders installed
  on this computer, the newest copy of each, and import one. VS Code's own copy is left untouched.
- **Recommended extensions.** The project's `.vscode/extensions.json` is read, and each
  recommendation is marked Installed, Built in, Not supported (with what to use instead), On this
  computer, or Not here. You can also uninstall an extension, or turn it off so its contributions
  stop applying.
- **Banners under the title bar.** The window now says when its session has stopped (the editor and
  unsaved text stay), when the agent has no model, and shows Arena's broadcasts (info, warning,
  critical; newest first). X dismisses one; critical ones stay.
- **The IDE opens without signing in.** The editor, terminal, git and tasks work with no model;
  a message to the agent gets a notice saying how to sign in instead of a failed turn.
- **Arena's settings applied live.** When Arena's models or skills change, the window's model
  picker and the / menu pick them up within about 30 seconds, without a reload.
- **A skeleton while the session starts.** The window shows its shape until the first state
  arrives, instead of an empty workbench.
- **Code Studio connect.** `code-arena ide --connect` (signed in with `code-arena login`) dials
  Arena's Code Studio over a WebSocket with your API key, as "FOLDER on MACHINE" with its git
  project and branch. A lost or silent link is dialled again after 1 s, doubling up to 30 s, and the
  folder keeps its id between runs. The run's own key never leaves the machine. A refusal or a
  deletion in Code Studio ends it.
- **Ports view.** The panel's Ports view says the folder is on this machine, so what its programs
  listen on is already at localhost.
- **Help overlay.** Help: Show Help, or Help > Code Arena Help, opens a sheet at the window's right.
  It says what Code Arena is for and what each of its 23 parts does, gives 21 step-by-step recipes
  and lists the keys as the keymap has them now, with a link to the manual when the session gives
  one. The '?' in a view's title, or Help: the Chat in the chat's More menu, opens it with that part
  first. Escape, the cross or a click beside it closes it.
- **About dialog.** Help: About and the activity bar's info button show the version, licence,
  folder, log and source, with Copy, View Licence and Open Source. Help: View License and Help:
  Documentation are in the Help menu too.
- **Manage menu.** The gear now has Settings, Extensions and Keyboard Shortcuts beside the Command
  Palette and the colour theme, each with its keys.
- **VS Code's keymap.** Keys resolve as in VS Code: the last rule whose `when` holds wins. `when`
  has `!`, `&&`, `||`, brackets, `==`, `!=`, `<`, `<=`, `>`, `>=`, `=~ /re/` and `in` / `not in`. It
  reads live context keys such as `editorTextFocus`, `terminalFocus`, `sideBarVisible`,
  `activeViewlet`, `resourceLangId` and `config.*`. VS Code's default table fills the keys no
  command takes.
- **keybindings.json.** Your own rules from keybindings.json apply at once when you save the file or
  it changes on disk. They can use `[KeyA]`, `oem_1`, `mac`/`linux`/`win`, and `-command` takes a
  default away. A rule that cannot be read is skipped and named in a notification.
- **Chords as in VS Code.** The status bar says '(Ctrl+K) was pressed. Waiting for the second key of
  the chord…'. A second key that ends no chord is 'not a command' for 4 s and no longer runs on its
  own. A chord now completes in the editor even when its second key is Ctrl+S, Ctrl+W or Enter.
- **Terminal keys.** The terminal leaves to the workbench only the keys of the commands it skips the
  shell for, plus chords. `terminal.integrated.commandsToSkipShell` adds or removes (`-id`)
  commands, and `allowChords` and `sendKeybindingsToShell` work as in VS Code.
- **Keys shown are the ones in force.** The palette, the menus and the tab menu show your own key
  for a command when you set one. On macOS, Ctrl in a default key is Cmd and keys show as macOS
  writes them (⇧⌘P).
- **Save All, Save without Formatting and Revert File.** Ctrl+K S saves every tab with unsaved
  changes, Ctrl+K Ctrl+Shift+S saves without the on-save fixes, and File > Revert File reloads the
  file from disk.
- **A save never silently overwrites someone else's change.** If the file changed on disk after it
  was read, saving asks VS Code's question: Overwrite, or Compare your version with the file on
  disk.
- **Hot exit.** Unsaved text is backed up a second after you stop typing. At the next start it comes
  back into its tab, still unsaved, as one edit over the disk text. Saving or discarding drops the
  backup (files.hotExit).
- **Auto save.** files.autoSave supports afterDelay (files.autoSaveDelay), onFocusChange and
  onWindowChange. It pauses for a file that changed on disk and says so once.
- **Fixes on save.** files.trimTrailingWhitespace, files.insertFinalNewline and
  files.trimFinalNewlines run on Save. Each can be undone by itself and line endings are kept.
- **Open files catch up at once.** When the window regains focus, or after the agent's
  edit_file/write_file, the open files are checked on disk straight away instead of at the next
  watch tick.
- **Exit with unsaved text.** File > Exit and the title bar's close button back up unsaved text
  first. With hot exit off they ask Save / Don't Save.
- **Indentation per file.** Each file's indentation comes from `editor.tabSize`,
  `editor.insertSpaces` and `editor.detectIndentation` (detected from its first 10,000 lines). Tab,
  Enter, Indent and Outdent use it, and a tab is drawn that wide. The status bar's 'Spaces: 4' /
  'Tab Size: 4' opens Indent Using Spaces/Tabs, Change Tab Display Size, Detect Indentation from
  Content and Convert Indentation to Spaces/Tabs. A pick stays until those settings change.
- **Line endings in the status bar.** 'LF' / 'CRLF' shows the ending most of a file's lines have, or
  `files.eol` for a new file (auto = the system's). A click picks another, and the file is saved
  with every line ending that way. The UTF-8 item says only UTF-8 is read and written.
- **Language mode detection.** A file's language is found as in VS Code: by `files.associations` (a
  pattern with a `/` against the path, else against the name), then the file's name, then its
  extension, then its first line (`#!/usr/bin/env python3`, `<?php`). Settings in a `[lang]` block
  apply to it, and the status bar names it as VS Code does (TypeScript, Shell Script, JSON with
  Comments).
- **Change Language Mode.** Click the status bar's language or press Ctrl+K M to pick another
  language for the file, or Auto Detect. The file is coloured again at once, and the choice lasts
  until its tab closes. "Configure '<lang>' language based settings..." adds an empty `[lang]` block
  to your settings.json and opens it.
- **Commands with no time limit above the chat's box.** See every job the agent started with no
  time limit or in the background under '2 commands, 1 running' (a click folds them all): each with
  a spinner or a dot, 'job N', its command, 'running, no time limit', 'running in the background' or
  'exit code 0 after 3m 12s', and 'Stop job N'. Open one to follow the last 200 lines of its output;
  the newest one running opens by itself. The bar takes a quarter of the window at most.
- **The agent carries on when a long command ends.** When a job with no time limit ends while
  nothing runs, the thread says 'Job N ended with exit code 0.' and the model carries on from its
  output in a turn of its own, as in the terminal. A job you stopped only gets the notice.
- **Right-to-left and mixed text in the chat.** Persian, Arabic and Hebrew stand on the right: the
  box and the request bubbles lay out each line by its own words, an answer's paragraphs, headings
  and quotes follow their first strong letters (a quote's bar moves to the right), a list puts its
  bullets on the side of its first item, and code blocks stay left to right. Thinking, queued
  requests and quotes do the same, and plan step and session titles are isolated so they do not
  reorder the words around them.
- **Memory view.** See what the agent remembers, grouped as This project, About you and The
  company's, pinned ones first with their kind. Find a memory by its words, keep one with Remember
  something… (up to 500 characters, for this project only or every project), forget your own, open
  the project's long memory, and see when memories are kept on this machine. The view is gone when
  Arena denies memory.
- **Goal mode in the IDE.** Chat: Work Toward a Goal… (the target in the chat's header, or `/goal
  TEXT`) opens a form for the goal, how it is checked (a command succeeds, a condition holds, or the
  agent writes it down first) and its budget (until it is met, or steps, minutes and tokens). The
  agent then works on it turn after turn until the check passes, the budget is spent or it gets
  stuck.
- **The goal's bar over the chat box.** It shows the goal's status, what it has spent (`turn N · X
  of Y steps · …`) with a budget bar that turns to the warning colour at 90%, the check and how the
  goal ended. Pause, Resume, Cancel and Put away are on the bar, with the goals waiting after it
  (each with a drop) and Start the next. Add a Goal After This One…, Pause, Resume and Cancel the
  Goal are in the palette too.
- **Loops in the IDE.** Chat: Loops… (or `/loop`) lists the loops, each with how often it runs,
  whether it may change files, its runs, the runs it skipped and how its last run went, and a stop.
  It also adds a loop: a prompt run every 15 minutes, 30 minutes, 1 hour, 4 hours or 1 day. A bar
  over the box says how many loops run, and each run's report goes to the agent with your next
  message.
- **Skills view.** Every skill the agent reads, under This project, Yours, From Arena and Built in,
  each group folding and saying its folders. A click opens the project's or your skill's SKILL.md
  (others go into the chat's box as /name); Export (.zip) and Use in the Chat on hover. New Skill
  makes one for this project, in Arena or on this computer, and opens it. Refresh and Collapse All
  are in the title. Its More menu has Import Skills..., which previews what comes in, where it
  goes, and Replace, Rename or Skip for a name already there, with secrets left out. It also has
  Export the Project's and Yours..., Export All... and the Built-in Skills on/off.
- **Workspace trust.** The first time you open a folder, Code Arena asks whether you trust its
  authors, since it runs some of the folder's own programs (formatters, language servers, tasks,
  debuggers, notebook kernels, project tools). You can trust the parent folder too. Until you trust
  it, the folder is in Restricted Mode: a banner and a "Restricted Mode" status bar item say so.
  **Workspaces: Manage Workspace Trust** asks again or takes the trust back. The answer is kept on
  this computer, and a feature refused for an untrusted folder shows a toast with Trust.
- **Merge conflicts in the editor.** A file with <<<<<<< / ||||||| / ======= / >>>>>>> markers shows
  the current change in green, the incoming change in blue and the common ancestor in grey. Each
  marker line says which side it is, with marks in the overview ruler. Click Accept Current Change,
  Accept Incoming Change or Accept Both Changes after the conflict's first line, or use the Merge
  Conflict commands (Accept Current, Incoming or Both, Accept All Current, Incoming or Both, Next
  and Previous Conflict). Each accept is undone with one Ctrl+Z. merge-conflict.decorators.enabled
  and merge-conflict.codeLens.enabled turn the colours and the lens off.
- **Automation view.** The Automation view lists this machine's schedules: each shows its name
  (struck through while off), how often, its mode and next run, its prompt and its last run. Run it
  now, Turn off/on and Remove show while the pointer is on a row. New Schedule asks for a name,
  every 30 minutes to 7 days, a time of day for daily ones, the mode, the folder and what to do. Due
  schedules run while the IDE is open, and with it closed after `code-arena schedule install`.
- **Arena's agents and scheduled tasks in the IDE.** Agents in Arena runs an Agent Studio agent with
  what it is given, and its run is followed here step by step until it ends. Scheduled tasks in
  Arena shows each task's cron, last run and next run, and runs one now in Arena.
- **Schedules can be turned off.** A schedule that is off is kept but not run when due, and
  `code-arena schedule list` says so.
- **Extensions view.** Ctrl+Shift+X lists your installed extensions. Each row shows the icon, name,
  version, description and publisher, what the extension brings, a warning for what is not used
  here, a switch to turn it on or off, and Uninstall (asked first). The view also lists the
  project's recommendations from .vscode/extensions.json, and the extensions that VS Code, VSCodium
  or VS Code - Insiders have on this computer, with Import or Update.
- **Install from VSIX and extension search.** 'Extensions: Install from VSIX...' (palette, or the
  view's ... menu) asks for the path of a .vsix. The search box finds extensions by name, id,
  publisher or description, and @installed, @enabled, @disabled, @recommended and @local narrow it.
  The title also has Refresh and Clear Extensions Search Results, and the palette has Show Installed
  and Show Recommended Extensions.
- **Rate an answer.** Under an answer of a chat kept in Arena you can give a thumb up or down. A
  down vote can carry a reason (Wrong, Incomplete, Too long, Unsafe, Ignored instructions, Other).
  Either vote can carry some words and 'Share this chat with the admins'. Take back removes the
  rating, and the thumb you gave is still shown after a reload.
- **Virtual machines for the agent.** The agent can list your VirtualBox, VMware, libvirt and
  Hyper-V machines, start and stop them, take and restore snapshots, save the screen, run commands
  in the guest, copy files in and out, and run a whole test in one go with vm_test (restore a
  snapshot, copy in, run, bring back logs and the screen into .code-arena/vm, then restore the
  snapshot). The tools appear only when a hypervisor is installed.
- **Sign in to a VM.** When a VM tool needs the guest's user and password, a 'Sign in to VM' dialog
  asks for them. It says so when the guest refused the last sign-in, offers 'Keep it in the keyring'
  when a keyring is available, and closes when another window answers. A corner bubble tells you it
  is waiting. The password never shows in the chat, and it is saved to the keyring only after the
  guest accepts it.
- **Zoom the window.** View: Zoom In (Ctrl+=, Ctrl+Shift+=, Ctrl+Numpad+), Zoom Out (Ctrl+-,
  Ctrl+Numpad-) and Reset Zoom (Ctrl+Numpad0) make the whole native window 20% larger or smaller a
  step, from -5 to 9, kept as window.zoomLevel. Two quick presses are two steps.
- **Accessibility options over any theme.** More contrast gives secondary text the text's colour and
  makes borders stand out. Strong focus rings mark the keyboard's place in the text's colour.
  Reduce motion stops the spinners. Colour-blind safe colours use blue and orange for added and
  removed, passed and failed. The text size makes the workbench larger and leaves the editor's text
  as it was.
- **Accessibility toggles.** Toggle High Contrast Theme, Toggle More Contrast, Toggle Colour-Blind
  Safe Colours and Toggle Reduce Motion are in the Command Palette.
- **Status bar agent and window items.** The status bar shows the agent's mode (red in Yolo) and
  model; click either to change it in a quick pick (Chat: Open Mode Picker / Open Model Picker). It
  also shows a spinner with the count of commands running with no time limit, the last turn's time
  and tokens ('12 s · 8.6K in (73% cached) · 1.2K out'), a dot for the session's connection, and
  'code-arena <version>', which opens About. The Local remote item offers Open Folder and Open
  Recent. Every item has a tooltip.
- **Compact the chat and see where it was compacted.** Type /compact and press Enter, or run Compact
  Now, to have the model summarize the older part of the chat. It is refused while an answer is
  being written and when there is too little to compact. A line over the box says it is
  compacting, and a toast says how it ended. A 'Chat compacted' divider then sits after the last
  turn it covers: Show summary shows what the model goes on with, and after another compaction
  the divider moves and shows the latest summary. Questions the summary covers can no longer be
  edited or answered again.
- **Checklists in the Plan view.** The checklist on in the session shows 'N/M done' and Turn off,
  and its items are boxes you tick, each with how the model did or checked it. With none on, you
  turn one of yours or the repository's on, and make, edit or delete your own (its items one per
  line).
- **The chat's own commands work in the IDE.** /help, /model, /mode, /thinking, /cost, /mcp,
  /commands, /skills, /plan, /checklist, /commit, /hooks, /memory, /remember, /project, /assistant,
  /arena-agents, /arena-tasks, /sync, /tools, /permissions, /compact-at, /sandbox, /jobs, /search,
  /output-style and /web answer in the thread. /compact, /rewind N, /fork [N] and /resume ID do what
  the window's own buttons do. /tasks, /context, /projects, /loop, /goal TEXT and /resume open their
  views, and /review asks the model to review the changes not committed yet. The / menu lists these
  commands first.
- **!command in the chat.** Type !git status to run a command yourself: its output shows in the
  thread and the model reads it with your next message. Sent while the agent works, it runs when the
  turn ends.
- **After each turn, as in the terminal.** git.autoCommit commits what the turn changed, your
  statusLine command's line shows in the status bar, what background work said shows as toasts, and
  after a long turn the agent is reminded to keep what it worked out.
- **Chats in Arena under the sessions.** The Sessions view lists your Arena chats ("ALI'S CHATS"),
  newest first, with a Find a chat box that matches title or folder words. It shows at most 200,
  then "N more: find one by its title." Chats already kept with a session of this folder are left
  out. Code Arena's own chats have the terminal icon. A click continues the chat here, in its
  session or in a new one filled with its history, with a spinner on the row while it opens.
- **Send the other sessions to Arena.** The section's title has Send the Other Sessions Here: each
  session of the folder that Arena lacks goes to its chat, and a toast says what it came to.
- **Sync mark in the chat header.** The chat header says where the session stands with its Arena
  chat: In step, N to send, Web news or Not in step, with the full words on hover. A click shows the
  sessions.
- **Web news taken in while idle.** Between turns, what the web chat adds comes in at once, and the
  thread and the list are drawn again. The session's notices (Arena out of reach and back, messages
  taken in) show once, as toasts. After each turn the chat in Arena is let go in the background, and
  closing the window sends what Arena still lacks.
- **Tools picker.** The wrench in the chat box's toolbar shows how many tool groups are on. Click it
  for 'Tools: N of M on': Code Arena's own tools (always on), the Arena project's tools and each MCP
  server's. Each group has an icon, a shield when it asks you before each call, an About that shows
  what it is for and its functions, and a switch. Turning a group off keeps it off for the session,
  including tools that join later. A change made while an answer runs applies when the answer is
  done. Customize tools… is at the foot of the list. The wrench is hidden when the model calls no
  tools.
- **MCP servers in the status bar.** The plug shows connected/total servers, in the warning colours
  when one is down. Click it, or run MCP: List Servers, for the MCP servers dialog. It lists each
  server's state, its tools or why it is not connected, its address (without user, password or
  query), when it is tried again, and Try again (also Try All Again). A footer says which settings
  turn Arena's tools and Farzan Graph's on and off.
- **Other providers dialog.** The plug beside the chat's pickers, or More → Other Providers…, lists
  the providers of your own keys: each one's API (Messages API or Chat completions), its address,
  where its key comes from (kept here, from its variable, or no key in red), up to four of its
  models, and a button to remove it. You can add Anthropic, OpenAI, DeepSeek, OpenRouter, Mistral or
  another API (its name, address and what it speaks). The key goes in a password field and is
  checked by listing its models. It is kept in credentials.json, which only you can read, and is
  never shown again. The models appear at once in the model picker as provider:model. When Arena's
  admins do not allow other providers, the dialog says so.
- **Work as an Arena assistant from the chat.** The chat's More menu has an Assistant submenu (None:
  Code Arena's own, then yours and those shared with you, each with its description and model, a
  check by the one at work; dimmed while a turn runs), and Chat: Work as an Assistant… is a quick
  pick. /assistant [N|NAME|none] works in the chat's box too; what it says comes as a bubble.
- **Put the session in an Arena project.** The More menu's Project submenu and Chat: Put the Session
  in a Project… offer In no project and the projects you add to, with "its repository's" by the one
  this folder's repository is in; /project NAME|N|none and /project new NAME work in the chat. The
  status bar shows the project the session counts to, and a click changes it.
- **Projects view.** Mine, Global and All I See, a box to find a project, New Project (global when
  you may) and Refresh; each row says global, archived, your role, how much of its goals are done,
  this month's spend of its budget, whether this session counts to it and whether this folder's
  repository is in it, and the pointer on a row puts the session in it.
- **Accounts menu.** The Accounts button in the activity bar opens a menu that says whose key is in
  use and on which Arena. From it you can open Your Account in Arena, open People (Admin) if you may
  manage people, sign in as someone else, send feedback, or sign out (you are asked first). The
  button has a warning dot while you are signed out or Arena refused the key.
- **Sign in from the window.** Sign In with User Name and Password... asks for the Arena's address,
  your user name, your password and then your authenticator code if the account has one. The session
  then picks up the new key in the same window without restarting.
- **Send Feedback.** Send Feedback... is in the Accounts menu, the Manage menu and the palette. You
  say whether it is an idea, a problem, praise or something else, and Arena's admins get it and may
  answer you there.
- **Open Folder.** File: Open Folder... (Ctrl+K Ctrl+O), the welcome's button or the Explorer's
  "..." menu lists a folder as VS Code's simple dialog does: Open <folder>, "..", its folders (Enter
  goes in), the folders opened before, and "Go to <path>" for a whole path typed (/, ~/, C:\). The
  window then works on that folder in a new session (the last one stays in Sessions). Its tabs, tree
  and terminals are made again, and the folder's own layout comes back. Changes not saved are asked
  about first.
- **Open Recent.** Ctrl+R lists the folders opened before (ten, then More...). Clear Recently
  Opened... forgets them.
- **Welcome.** An empty window says "No file is open" and offers Open Folder... above its keys. An
  empty second group says "Drag a tab here, or open a file."
- **Changed files as a tree or a list.** Source Control and Agent changes can show changed files as
  a tree or as a list. Use View as Tree / View as List in the view's title; in a tree, Collapse All
  folds every folder. `scm.defaultViewMode` sets the starting view, and the choice and the folded
  folders are kept for the folder. With `scm.compactFolders`, a folder that holds only one folder
  shares its row with it (`tests/unit`).
- **Folder actions on changed files.** In a tree, a folder row has its group's actions for every
  file under it: Stage, Unstage and Discard in Source Control, and Accept and Revert in Agent
  changes. Revert asks once for the whole folder.
- **Branches in Source Control.** Checkout to... (from the palette or the status bar's branch item)
  lists your branches, remote branches and tags, and can create a new branch or create one from
  another ref. Create, Rename and Delete Branch check the name as you type. Deleting a branch that
  is not merged asks 'Delete anyway?'. Undo Last Commit stages its changes again and puts its
  message back in the box.
- **Pull, Push, Fetch and Sync.** The view's … menu has Pull, Pull (Rebase), Push, Push (Force with
  Lease), Publish Branch, Fetch, Fetch From All Remotes, Sync, Commit & Push and Commit & Sync. Sync
  asks first while git.confirmSync is on ('OK, Don't Ask Again' turns it off) and rebases while
  git.rebaseWhenSync is on. A branch with no upstream is published. Git's errors appear as toasts in
  git's own words, and a fetch, pull, push or sync is stopped after 10 minutes.
- **Sync in the status bar.** The status bar shows the branch (with '*' while there are changes) and
  '0↓ 1↑'. Clicking it syncs, its icon turning while git works; a branch with no upstream shows a
  Publish cloud. When nothing is left to commit, a 'Sync Changes 1↓ 2↑' or 'Publish Branch' button
  appears under Commit. With git.autofetch on, the remotes are fetched every three minutes.
- **Keyboard Shortcuts editor.** Preferences: Open Keyboard Shortcuts (Ctrl+K Ctrl+S) lists every
  command with its keys, its when and its source. Search by words, by keys in quotes, by @source: or
  @command:, or press Record Keys and the keys themselves.
- **Change a shortcut where it is listed.** Double-click a row or press Enter, then press one key or
  a chord of two and Enter. The other commands on those keys are named. The row's menu adds, removes
  or resets keys, changes the when (checked before it is written) and copies the rule or the
  command's id.
- **keybindings.json written as VS Code writes it.** A default is taken away with -command and the
  new rule is added after it, and the person's own rules are changed in place. The other rules and
  comments are kept. Rules that do nothing are listed above the table, and Preferences: Open
  Keyboard Shortcuts (JSON) opens the file.
- **3-way merge editor.** Open a file in conflict with **Resolve in Merge Editor** (the conflict
  lens, the palette, or the merge icon on a Merge Changes row in Source Control; a row click opens
  it too when `git.mergeEditor` is on). The incoming and current sides each show their branch and
  commit. Take a conflict from either side with **Accept Incoming**, **Accept Current** or **Accept
  Combination**, or with the check in a side's margin. You can also edit the result, which is the
  file itself, and save it with Ctrl+S. The header says how many conflicts remain, steps through
  them with Previous and Next (both sides follow), and **Reset** puts back the text the editor
  opened with. **Complete Merge** saves the file, stages it and closes the editor. It asks first
  while conflicts remain. A file that is not text is refused, and a banner says when git has already
  merged the file. Below 700 px the three panes stack into one column.
- **Formatters found for each file.** The IDE finds the formatters for each file, in order: Prettier
  (from the nearest node_modules, then the PATH), Ruff and Black (from the folder's .venv, then the
  PATH), gofmt, rustfmt (with the edition from Cargo.toml), clang-format and dotnet format. Each one
  says whether it can run, why not, and how to install it.
- **Your own formatters.** Add them to settings.json under "formatters" (command, args, languages or
  extensions, rangeArgs), with ${file}, ${relativeFile}, ${workspaceFolder}, ${tabSize} and range
  placeholders. The text goes in on standard input and comes back on standard output.
- **Formatting runs sandboxed.** Formatters run in a read-only sandbox with their own /tmp and no
  network. They get none of Code Arena's keys, NO_COLOR is set, at most four run at once, and each
  has 10 s (30 s for dotnet format). They run only in a trusted folder, and a fault's line:col is
  reported back.
- **Prettier settings.** prettier.printWidth, tabWidth, useTabs, semi, singleQuote, trailingComma,
  bracketSpacing, arrowParens and endOfLine apply when the folder has no Prettier configuration of
  its own.


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
- **Skip thinking answers in the same block.** After Skip thinking, the answer used to start under a
  'Thinking skipped' notice, and the first block was left showing 'Answering now…'. It now goes on
  right under its block, which folds to 'Thought for N s, cut short'.
- **The chat follows a streaming answer.** The thread used to scroll only when a new item came, so a
  long answer, a growing thinking block or the answer's footer ran below the view. It now follows
  them while the end is in view, and stays where you scrolled if you scrolled up to read.
- **A thinking block cut by a message you send no longer ticks on.** When you send a message while
  the model thinks, the thinking part above your message used to keep 'Thinking… N s' and its Skip
  pill forever. It now ends with the turn.
- **Escape closes the find widget after Replace All.** Clicking Replace or Replace All used to take
  the keys out of the widget, so Escape did nothing. The keys now stay in the replace box.
- **The @ list no longer covers the chat box.** It now sits above the box, clear of its border,
  the same way the / list does.
- **Show earlier keeps your place.** Loading the last earlier turns no longer moves the thread
  down by the height of the row that went away.
- **Going back to a question no longer closes the window.** This happened when an answer with a code
  block was on screen; the theme is no longer applied again when the session sends the same one.
- **Ctrl+K chords work in the editor.** Ctrl+K Enter and Ctrl+K Shift+Enter now run their command
  while the editor has focus, instead of typing a new line.
- **No crash on a machine without italic fonts.** A preview tab is drawn upright when no italic font
  is installed, so opening one no longer closes the window.
- **A queued message keeps its width under the pointer.** Its Edit and Cancel used to push the
  bubble wider as the pointer came over it. They now sit in room the bubble always keeps.
- **A chat picker's menu shows its rows in full.** A menu opened from a pill near the view's right
  edge, such as Thinking, cut its descriptions short ('The model's ...'). It now moves left to fit
  inside the view.
- **Split editors show one caret.** Every editor group used to draw its caret. Only the focused
  group draws one now, as in VS Code.
- **Line numbers turned off are hidden.** With editor.lineNumbers set to off, the editor no longer
  draws the numbers in black when the window uses its software renderer.
- **The current line's highlight reaches the editor's edge.** It no longer stops where the longest
  line ends.
- **Split editors fit the window.** Two editor groups with long lines or many tabs no longer push
  the Agent view past the window's right edge. Each group's tabs scroll inside its own strip, so its
  Split and "..." buttons stay in sight.
- **The open tab stays in sight.** When a group has more tabs than fit, the tab shown is scrolled
  into view, also when the window opens. The mouse wheel scrolls the tabs.
- **Menu rows are no longer cut short.** The tab menu's longest row now reads "Copy Relative Path"
  in full.
- **Source Control's message box fits the view.** Its placeholder ends in an ellipsis inside the box
  instead of running past the Commit button. The box stays dark while you type in it.
- **Source Control's and Changes' letters line up.** The M, U and D letters and the group counts sit
  in one column at the right and stay put when a row's actions show.
- **Commit is in Source Control's title after a restart.** Before, the check icon was missing until
  the view was opened again.
- **Text boxes stay readable under the pointer.** The command palette, Quick Open, the Search,
  Sessions and Explorer inputs and the chat's edit boxes no longer turn white with light grey text
  when the pointer is over them.
- **A new terminal is named after the shell that runs in it.** It reads 'bash 2' instead of 'sh 2'
  when `$SHELL` is not set.
- **An ended terminal's message can be read.** 'The shell ended with exit code N. Press any key to
  start a new one.' now wraps at the foot of the panel instead of being cut off below it.
- **The chat box takes the keys after a new session.** Clicking anywhere in the centred box now lets
  you type. Before, a click on its right side did nothing and the typing was lost.
- **Session search finds a word as you type it.** 'greet' now finds 'greeting'; before, only whole
  words matched, so the list said 'No results found.' while you were still typing.
- **Escape twice leaves Zen mode.** Pressing Escape twice leaves Zen mode as in VS Code, from the
  editor too. Before, only Ctrl+K Z left it.
- **Title bar menus grey out what cannot run.** Rows such as Move Panel To Bottom (when the panel is
  already there) or Start Debugging were drawn like live rows, and a click on them did nothing; now
  they are greyed out.
- **Moving between title bar menus.** With a menu open, moving the pointer onto another menu's title
  opens that menu. Before, the first menu just closed.
- **Ctrl+P then Enter goes back to the previous file.** With nothing typed, Quick Open now picks the
  file opened before the current one, as in VS Code. Before, it picked the file already shown.
- **The palette works while the breadcrumbs have the keys.** After Focus Breadcrumbs or closing a
  breadcrumb list, Ctrl+Shift+P and every other binding work again; only the bar's own keys stop
  there.
- **The Outline filter stays readable.** Pointing at the filter box no longer turns it near white
  with light grey text.
- **Wrapped lines no longer overlap in the Context view and the compaction settings.** The usage
  line and the field labels take the lines they need.
- **The plan review's prompts say Press Enter once.** Change Step, Another Way and Reject ask their
  question with the input's own hint after it.
- **A long notice or error in the chat wraps.** "Reading the plan again with fresh eyes before your
  review" and errors no longer run past the chat panel's edge.
- **SVG text in previews.** SVG files opened in a preview tab now draw their text in the system's
  fonts.
- **Asking about picked words no longer hides them.** The 'Ask about this' field and its offered
  questions now stand above the words you selected in an answer, not over them.
- **A finished turn's bubble waits while you reply in it.** The 'Ready: the agent finished' bubble
  no longer disappears after 20 seconds while its Reply field has the keys or holds words.
- **A command of several lines fits on its card's head.** A call whose command runs over several
  lines now shows its first line and '…' in one row, not a tall, half-empty card.
- **Opening a Markdown file no longer stops the window where no italic font exists.** Italic text in
  the editor, chat answers, the Markdown preview and the terminal is drawn upright on such a system.
- **Headings stay in the UI font.** Headings in answers and the Markdown preview, table headers and
  the Context titles came out in the monospace font when the UI font had no semibold face. They are
  now bold.
- **An attached file's X removes it.** Before, the X on a chip above the chat box did nothing when
  clicked.
- **An upload's error can be read whole.** The chip cuts its line short; hover over the line to see
  all of it.
- **The notes under the chat box and on a diagram are readable.** 'No file dialog was found…' and
  'Nothing here draws diagrams…' wrap with normal line spacing instead of lines pressed together.
- **The Problems view's empty message fits the side bar.** 'No problems have been detected in the
  workspace.' wraps in a narrow side bar instead of being cut off.
- **Ctrl+, opens Settings.** Ctrl+, and the gear's Settings row open your settings.json, and the row now
  shows Ctrl+, as its key. Before, Ctrl+, did nothing and the row showed no key.
- **Typing works again after Change Language Mode and the indentation, line-ending and encoding
  pickers.** Closing one of these pickers now puts the cursor back in the editor, as VS Code does,
  so you can type into the file straight away.
- **Skill descriptions stay out from under the row's buttons.** In the Skills view, a long description
  now ends in an ellipsis before the Export and Use in the Chat buttons instead of running on beneath
  them.
- **Left-to-right items in right-to-left lists stand beside their numbers.** In an answer, an English
  item in a Persian numbered list now sits next to its marker instead of at the far side of the
  answer.
- **The goal bar, the goal form and the loops are easier to read.** Wrapped lines there now have room
  between them instead of nearly touching.
- **You can tell apart tabs with the same name.** When two open tabs share a file name (two skills'
  SKILL.md, for example), each now shows the folders that tell it apart, as VS Code does
  ('.../explain-module').
- **The Extensions view's "No extensions found" message wraps.** A search with no result no longer
  runs off the side bar's right edge.
- **A guest that refuses the password asks again.** When a VM's guest refused the user and password
  typed in the sign-in form, the form closed and the agent was told you did not sign in. It now
  opens again and says the guest refused, with the user filled in.
- **The status bar's spinners stand still with reduced motion.** With workbench.reduceMotion on, the
  model item's spinner and the jobs item's spinner no longer turn.
- **Edit and Answer again look disabled when they cannot be used.** On a question a compaction
  covers, and while an answer is being written, the two actions are now drawn greyed out, as VS Code
  draws a disabled action. Before, they looked like the other actions.
- **A command's answer is scrolled into view.** Sending a line from the box (a /command, a !command
  or a question) takes the thread to its end even when it was scrolled up, so the answer is no
  longer hidden below the fold.
- **Multi-line notices are readable.** The output of a !command and other notices of several lines
  in the thread are no longer squeezed together.
- **Send button kept in a narrow chat box.** With the tools and providers buttons in the toolbar,
  the send button no longer runs past the panel's edge and the model picker no longer shrinks to
  nothing; the thinking picker's words give way first.
- **Activity bar follows the view shown.** A view opened from the palette or a key, such as
  Projects, has its icon scrolled into sight instead of lit below the bar's fold.
- **MCP servers dialog names each server once.** A server titled by its own name no longer shows the
  name twice.
- **Keyless local provider not flagged.** A provider at an http:// address or ollama, kept without a
  key, says 'No key needed' instead of a red 'No key'.
- **Sign-in prompt.** The address step no longer says twice to press Enter.
- **A folder opened under the same repository gets its shell there.** After Open Folder or Open
  Recent picks a subfolder of the repository already open, a new terminal starts in that folder.
  Before, it started in the old folder.
- **The Explorer still lists the project when the folder is a repository's subfolder.** Opening
  such a folder, or starting there, showed 'is outside the folders' in the Explorer. It now lists
  the repository from its top.
- **A long side bar title no longer runs under its buttons.** The Source Control title with the
  tree buttons is now cut with '…', as in VS Code.

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
