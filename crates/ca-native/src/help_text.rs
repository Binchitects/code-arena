//! The Help overlay's own words, as the old Code Arena's help has them: what the workbench is for, what each of its
//! 23 parts does, the common tasks step by step, and the keys. **Words** in bold are the window's own labels. The
//! overlay (`help.rs`) draws them as Markdown, which [`markdown`] writes: the part asked about first, every part,
//! the recipes, then the keys as the keymap has them now. Keep it plain and short.

use serde_json::Value;

use crate::layout::View;
use crate::state::{Send, Ui};

/// The company's product and its code graph, as the old help names them.
const PRODUCT: &str = "Farzan Arena";
const CODE_INTEL: &str = "Farzan Graph";

/// What Code Arena is for, under its name at the overlay's top.
pub const ABOUT: &str = "An editor around the coding agent, for this folder: its files, a terminal, and the agent's \
chat, which can read, change and run things here as the mode allows.";

/// A part of the workbench: its name, what it does, and the heading of the manual's Code Arena page that tells the
/// rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Part {
    pub id: &'static str,
    pub name: &'static str,
    pub text: &'static str,
    pub manual: &'static str,
}

const fn part(id: &'static str, name: &'static str, manual: &'static str, text: &'static str) -> Part {
    Part { id, name, text, manual }
}

/// Every part, in the order the help lists them.
pub const PARTS: [Part; 23] = [
    part(
        "automation",
        "Automation",
        "automation",
        "What runs on its own. **Schedules here**: a prompt Code Arena runs in a folder every so often (every 30 \
minutes to every week, a day's at a time of day), as a session of its own in the mode chosen (**reads only** unless \
you let it edit), its sessions synced to Arena as any. They run while this IDE is open, and with it closed once \
`code-arena schedule install` has set up an OS timer (systemd, launchd or Task Scheduler); ▶ runs one now, the switch \
turns one off. **Agents in Arena**: your Agent Studio agents and those shared with you, each run here with a text, \
how it goes shown step by step. **Scheduled tasks in Arena**: yours, run now with ▶; their answers come to you in \
Arena. In the terminal: /schedule and /agents.",
    ),
    part(
        "projects",
        "Projects",
        "projects",
        "Arena's projects, as its Projects page has them: **Mine** (those you are in), **Global** (everyone's to add \
to) and **All I see**, found by their words. The one this session counts to is in bold, and the one this folder's \
repository is in says so. A click opens a project in a tab of its own, the same page as Arena's: **Overview** (its \
cost, tokens, requests, people's and the models' time and models over the time window, with its presets, and \
**Export** as PDF, Excel or CSV), **Goals and tasks** (add, change and tick off, as a contributor or owner), **In \
it** (its repositories, chats and sessions: a chat or session of yours continues in the chat; **This session** puts \
the session open now in it or takes it out), **People** (owners change their roles) and **Settings** for owners (its \
dates, budget, global, archive). **+** makes a project when Arena lets you (a global one too, when you may). Arena \
decides what you may do in each, as on its page; what a session uses shows there within seconds. The project in the \
status bar opens the session's; /project opens it from the chat's box, /projects shows this view and /project new \
NAME makes one.",
    ),
    part(
        "tasks",
        "Tasks",
        "sub-agents-and-tasks",
        "The sub-agents of this session, beside the chat (the chat shows one line of each, **Tasks** in its header \
opens this). The agent hands work to a sub-agent: a context of its own, the reading tools unless it named others, as \
one of your Arena assistants if it chose. Those that only read work side by side; those that may change files one at \
a time, never asking (what the mode would ask is refused to them). Each row says where it stands, what it does now, \
its time and tokens; click one for what it was asked, its thinking, each tool call with its result, and its report. \
**Stop** (the square) stops one; the agent carries on without it. **+** starts a task of yours in the background, as \
an assistant if you choose, reading only unless you let it change files: you go on with the chat meanwhile, and its \
report reaches the agent with your next message. /tasks in the terminal lists them too.",
    ),
    part(
        "extensions",
        "Extensions",
        "extensions",
        "VS Code's extensions, for what they declare (Ctrl+Shift+X): colour and file icon themes, snippets, languages \
(comments, brackets, folding), keybindings and setting defaults. **Install from VSIX…** takes a .vsix you \
downloaded; **VS Code on this computer** imports one VS Code has; **Recommended** says where each of the project's \
recommendations stands here. A switch turns one off; the bin uninstalls it. No extension's code runs: ESLint, \
GitLens, debuggers and language servers are not extensions here.",
    ),
    part(
        "debug",
        "Run and Debug",
        "run-and-debug",
        "Choose a run configuration (.vscode/launch.json's, or one the project's files give) and press **Start \
Debugging** (F5) or **Run** (Ctrl+F5: in a terminal, without the debugger). **Build** is Debug or Release. While a \
session runs: **Variables** of the frame chosen (double-click one to change it), **Watch** expressions, the **Call \
Stack** (a frame opens its line), and the **Breakpoints** (click beside a line number, or F9; right-click for a \
condition or a log message). The toolbar over the editor continues (F5), pauses (F6), steps over (F10), into (F11), \
out (Shift+F11), restarts and stops (Shift+F5).",
    ),
    part(
        "console",
        "Debug Console",
        "run-and-debug",
        "What the program and the debugger print while debugging; the box below evaluates an expression in the frame \
chosen while the program is stopped (↑ and ↓ bring back what you typed).",
    ),
    part(
        "output",
        "Output",
        "language-servers",
        "What each language server said, one channel each (choose it above): when it started, its messages, why it \
ended, and the last lines it wrote to stderr. **Restart** starts it again. The status bar's language server item \
opens it too (**Show Language Server Log**).",
    ),
    part(
        "ports",
        "Ports",
        "ports",
        "In a Code Studio workspace: what its programs listen on (a dev server you started in a terminal), each at an \
address of its own that only you open, signed in to Arena. A new one is announced with **Open in Browser**. On a \
row: open it, copy its address, set its label, **Share** it with everyone signed in (or make it private again), \
**Stop Forwarding**. **Forward a Port** adds one before its server starts.",
    ),
    part(
        "settings",
        "Settings",
        "settings-and-keyboard-shortcuts",
        "The settings Code Arena reads, as VS Code's (Ctrl+,). Search by words, or **@modified**, **@lang:python** \
(a language's own), **@id:editor.tabSize**. **User** is your settings.json, for every project; **Workspace** the \
project's .vscode/settings.json. A change is written at once, the file's comments kept; the menu beside a setting \
resets it. **Accessibility** has the zoom (Ctrl+= and Ctrl+-), High Contrast as the system asks, less motion, more \
contrast, a larger text, strong focus rings and colour-blind safe colours.",
    ),
    part(
        "keybindings",
        "Keyboard Shortcuts",
        "settings-and-keyboard-shortcuts",
        "Every command with its keys (Ctrl+K Ctrl+S). Search by words, or press **Record Keys** and the keys. \
Double-click a key to change it; the menu adds, removes or resets one, or changes when it applies. What you change is \
written into your keybindings.json.",
    ),
    part(
        "activity",
        "Activity bar",
        "the-ide",
        "The icons down the side bar's edge. **Explorer**, **Search**, **Source control**, **Run and Debug**, \
**Extensions**, **Agent changes** (with the number of files), **Problems**, **Skills** and **Chat** switch the side \
bar; pressing the one shown hides it. Drag an icon to put it elsewhere in the bar, or onto the panel's tabs to move \
its view there (drag the tab back to return it); its right-click menu has the same moves, hides the icon (**Hide**), \
puts the side bar on the other side (the chat goes opposite) and resets them. A right-click on the bar lists every \
view with a check: uncheck one to hide its icon (its command still opens it), and **Activity Bar Position** puts the \
bar beside the side bar, above it as a row, or hides it. At its foot: **Terminal**, the theme (light, dark or the \
system's, and the colour theme), **Accounts**, **Manage** (the gear: Settings, Keyboard Shortcuts, Color Theme, \
Extensions, the palette, and how the model gets its tools), **Help** and **About**.",
    ),
    part(
        "explorer",
        "Explorer",
        "the-ide",
        "The folder's files, following the file the editor shows. Click opens one; a right-click has New file, New \
folder, Cut, Copy, Paste, Copy Path, Copy Relative Path, Find in Folder, Open in Integrated Terminal, Rename (F2) and \
Delete; Ctrl+click and Shift+click select several, a drag moves rows into a folder, and files dropped from the \
computer are written there. The files the agent changed are marked, and git's changes (M, U, A), what git ignores \
(dimmed) and the files with problems (red, yellow) too, with their folders. **Outline** lists the symbols of the file \
shown (the one the cursor is in marked); a click goes to one. A folder added to the workspace, or each folder of a \
.code-workspace file opened (**Open Workspace from File…**), is a root of its own below, with its name.",
    ),
    part(
        "search",
        "Search",
        "the-ide",
        "Words across the folder's files (those git sees), with match case, whole word, a regular expression, and \
files to include or leave out. A result opens the file at the match. Ctrl+P opens a file by a few letters of its \
path.",
    ),
    part(
        "changes",
        "Agent changes",
        "the-ide",
        "The files the agent changed in this run that you have not kept or put back yet, and below them **Your \
changes**: those changed by hand (git's that are not the agent's). Each opens as it was and as it is now. **Accept** \
keeps a change, **Revert** puts the file back, **Accept all** keeps them all; a file's right-click menu adds it to the \
chat, or asks the agent to explain or fix the change. Each group folds; **View as Tree** shows the files under their \
folders, and **Collapse All** folds everything.",
    ),
    part(
        "problems",
        "Problems",
        "problems",
        "The errors and warnings of the project's own check (its build, type check or linter), the language servers' \
and the editor's own: **Run check** runs the check. Each opens the file at its line, where the editor marks it too; \
**Fix** asks the agent to fix it, and its **⋯** menu copies it or offers **Quick Fix…**. **Filter problems** takes \
words, globs of files (`**/*.ts`) and `!` to leave out; the buttons beside it show each severity, or the active file \
only.",
    ),
    part(
        "review",
        "Review",
        "review",
        "A review of the changes not yet committed, or of the branch over its base, by a reviewer that reads the code \
and changes nothing. Its findings, worst first, open the file at their line and are marked there; **Apply fix** makes \
a fix that fits the file in the editor (undo takes it back), **Ask the agent** hands one over, **Dismiss** leaves it.",
    ),
    part(
        "skills",
        "Skills",
        "your-own-commands-sub-agents-and-skills",
        "Every skill the agent reads, by whose it is: **This project**'s (.arena/skills, in the repository), \
**Yours** (every project: kept in Arena when signed in to it, else on this computer), **From Arena** (your groups' \
and the company's, or a plugin's) and **Built in**. The model loads one when a task needs it; **/name** in the chat \
uses one (/ lists them). A click opens one of the project's or yours to edit (any other goes into the chat's box); \
**New Skill** makes one, for this project or for you. A skill saved here is used from the next message; one made or \
changed on Arena's Skills page comes within half a minute, or at once with **Refresh**. Skills in .code-arena/skills \
(the project's) and ~/.code-arena/skills (yours) are read too: each row says the folder. **Export** on a row, or \
**…** → **Export all**, saves them as a .zip of their folders; **…** → **Import Skills…** brings in a .zip, a \
SKILL.md or a folder, for this project or for you, after a preview (a name taken: **Replace**, **Rename** or \
**Skip**; a secret never comes in).",
    ),
    part(
        "git",
        "Source control",
        "source-control",
        "The folder's git: what changed since the last commit, in conflict (**Merge Changes**: a file opens with \
**Accept Current Change**, **Accept Incoming Change** and **Accept Both Changes** above each conflict; the merge icon \
opens it in the merge editor, incoming and current above the result, and **Complete Merge** stages it), **staged** \
for the next commit or not. A file opens as the last commit has it and as it is now; **+** stages it, **−** unstages \
it, **↺** puts it back; a group's or a folder's buttons, all of its files. Each group folds with its chevron (← and → \
too); **View as Tree** shows the files under their folders, **Collapse All** folds everything, and the folder keeps \
it so. Write the message and **Commit** (all of it when nothing is staged; its arrow has Amend, Commit & Push and \
Commit & Sync); **Sync Changes** pulls and pushes, **Publish Branch** gives a branch its upstream, and **⋯** has \
Pull, Push, Fetch, Checkout to…, the branches and Undo Last Commit. Switch the branch below, and see the last \
commits. With several repositories (one in a folder of this one, or an added folder's) each has a section with its \
branch, its own commit box and changes, folding on its row; a git command asks which repository.",
    ),
    part(
        "sessions",
        "Sessions",
        "sessions-and-the-models-window",
        "This folder's conversations with the agent, newest first. Click one to carry it on; **New session** starts \
another. They are the same as in the terminal: code-arena chat --resume opens them too.",
    ),
    part(
        "editor",
        "Editor",
        "the-ide",
        "Your open files, in tabs. A dot marks changes not saved yet, and Ctrl+S saves; the middle mouse button \
closes a tab. Split Right in a tab's menu (or Ctrl+\\\\) opens it in a second group beside it; drag a tab onto \
another group to move it there. Move into New Window in its menu opens it in a window of its own. A file the agent \
edits reloads in its tab when nothing in it is unsaved; saving a file that changed on disk since you opened it asks \
first. A Jupyter notebook (.ipynb) opens as cells: **Run** (Ctrl+Enter, Shift+Enter for the next) runs one in the \
notebook's Python, its outputs below it; **Open as Text** shows its JSON.",
    ),
    part(
        "terminal",
        "Terminal",
        "terminals",
        "Shells in this folder, under the editor. Ctrl+` shows or hides them, **+** opens another (the middle mouse \
button on its tab closes one), and dragging the panel's top edge resizes it. A terminal can do whatever your own \
shell can: the mode guards the agent, not you.",
    ),
    part(
        "chat",
        "Chat with the agent",
        "modes",
        "Ask for an answer or a change. **Assistant**, at the top left, works as one of your assistants in Arena (its \
instructions, model, tools and skills); the model and its thinking are beside it, with **Tools** and **Configure \
Tools** (the gear: each tool on or off, how it asks you, what the model is told of it, its arguments fixed or given a \
default, for every project or this one; the project's own tools and MCP servers to enable here). **Skip thinking** \
beside a thinking still going makes the model answer at once (the rest of the turn does not think). A sub-agent the \
agent starts is one line in the thread (what it is for, where it stands, its time and tokens): it opens in \
**Tasks**, the network icon at the top, with each sub-agent's own steps. **Goal**, the target, sets a goal the agent \
works toward turn after turn until its check passes or its budget is spent, or with **Until it is met** with no limit \
at all; another goal set meanwhile waits its turn, and each starts when the one before is met, so a long session \
carries on until every goal is met (its bar above the box pauses, resumes or cancels it, lists the goals waiting and \
drops one; /goal, /goal next and /goal drop N in the box too). **Loops** (/loop, or the command palette) run a prompt \
as a background agent every so often while the session is open, each run a task in **Tasks**. Each tool the agent \
uses is a card, with the diff under each edit (its chevron folds it; a finished turn shows its captions alone), and \
under a finished answer the files it changed (**N files changed**), each opening as the agent's change. A file it \
cites (`src/app.ts:12`), the path and line numbers of a diff, and the chip under the card of a tool that read or \
edited a file open it in the editor at those lines. A command it runs with no time limit shows above the box with its \
output and **Stop**; the turn waits for it. **Stop** under the box ends a turn. The mode under the box says what runs \
without asking: **Ask**, **Auto-edit**, **Plan** or **Yolo**. The folder mark at the top is the project in Arena the \
session counts to (what it uses goes to the project's accounting): choose one there or with **/project NAME**, and \
the folder's next sessions start in it. **/** at the start of the box lists /compact, /context, /clear, /project, \
your commands and the skills: **/name** and your words uses one (Esc lets it go, your words kept), and Enter on a \
name typed whole (**/compact**) sends it. A turn that fails says why under the thread (one Arena's safeguards refused \
names the rule, what it found and in which message). **Rewind to here** beside a question goes back to before it, as \
/rewind: the conversation and the code, only the conversation, or only the code, with the files that would change \
shown first. Select words in an answer to **Reply** quoting them (Ctrl+Shift+'), **Ask about this** or **Copy**; \
**Quote part of this answer…** under an answer does it by keyboard. The ring under the box shows how full the model's \
window is and what fills it (your instructions, files, messages, answers and tool results among it, what is kept for \
the answer and what is free), as **Context** in the status bar does; its **What fills it** (or /context) opens the \
**Context** panel: each tool's definition, skill, file and message in the model's own tokens, and what could be freed \
(**Turn off** a server's tools never called, **Compact now**); when the conversation is compacted, every message stays \
and a mark shows where (**Show summary**: what the model reads instead).",
    ),
    part(
        "status",
        "Status bar",
        "the-ide",
        "Along the bottom: the git branch, the agent's changes, the terminal and the commands running with no time \
limit, and the project in Arena the session counts to (a click opens it), on the left; the cursor's line and column, \
the file's formatter, its language server (starting, ready, failed, or off until the folder is trusted: click it to \
restart it or see its log; one that is not running, and a formatter that cannot run, in italics) and its language, \
the MCP servers (**Try again** for one not connected), how full the model's window is (**Context**: what fills it, as \
the chat's ring shows it, and when the session compacts), the mode, the model and the version (About: the licence, \
the source and where its log is, code-arena.log beside the program) on the right.",
    ),
];

/// A part by its id.
pub fn part_of(id: &str) -> Option<&'static Part> {
    PARTS.iter().find(|p| p.id == id)
}

/// The part a view is, for the '?' in its title: none for the views the help does not tell of yet (Plan, Memory,
/// Harness); the Outline is the Explorer's, the Context the chat's.
pub fn part_of_view(v: View) -> Option<&'static Part> {
    use View::*;
    let id = match v {
        Files | Outline => "explorer",
        Search => "search",
        Scm => "git",
        Run => "debug",
        Extensions => "extensions",
        Changes => "changes",
        Problems => "problems",
        Review => "review",
        Skills => "skills",
        Projects => "projects",
        Automation => "automation",
        Sessions => "sessions",
        Agent | Context => "chat",
        Tasks => "tasks",
        Terminal => "terminal",
        DebugConsole => "console",
        Output => "output",
        Ports => "ports",
        Plan | Memory | Harness => return None,
    };
    part_of(id)
}

/// A common task, step by step.
pub struct Recipe {
    pub title: String,
    pub steps: Vec<&'static str>,
}

fn recipe(title: &str, steps: &[&'static str]) -> Recipe {
    Recipe { title: title.to_string(), steps: steps.to_vec() }
}

/// The common tasks, in the order the help lists them.
pub fn recipes() -> Vec<Recipe> {
    vec![
        recipe(
            "Ask the agent for a change",
            &[
                "Type what you want in the chat's box, on the right, and press Enter.",
                "When it asks before an edit or a command, read it, then press **Allow**, **Always for this \
session** or **Deny** (stopping a command it runs with no time limit asks every time: there is no **Always**).",
                "What it changed shows under **Agent changes** and in the open tabs.",
            ],
        ),
        recipe(
            "Keep or undo what the agent changed",
            &[
                "Press **Agent changes** in the activity bar.",
                "Click a file: it opens as it was before and as it is now.",
                "Press **Accept** to keep it or **Revert** to put it back; **Accept all** keeps every change.",
            ],
        ),
        recipe(
            "Ask about code in the editor",
            &[
                "Choose the lines (or put the cursor on one) and press Ctrl+L: they go with your next message.",
                "Or right-click them: **Explain this**, **Fix this** or **Complete this** asks at once.",
                "On a line with a problem, the light bulb offers **Fix with Code Arena**.",
            ],
        ),
        recipe("Open a file fast", &["Press Ctrl+P.", "Type a few letters of its path.", "Press Enter."]),
        recipe(
            "Run a command yourself",
            &["Press Ctrl+` or **Terminal** in the activity bar.", "Type in the shell; **+** opens another one."],
        ),
        recipe(
            "Let the agent do more, or less, without asking",
            &[
                "Open the mode under the chat's box.",
                "**Ask** asks before edits and commands, **Auto-edit** only before commands, **Plan** changes \
nothing, **Yolo** asks nothing: use it only in a folder you can throw away.",
            ],
        ),
        recipe(
            "Go back to an earlier session",
            &["Press **Sessions** in the activity bar.", "Click the session; **New session** starts another."],
        ),
        recipe(
            "Change when the conversation is compacted",
            &[
                "Click how full the window is, in the status bar (tokens used / the model's).",
                "Choose **At a share of the window** (and set **Compact at**), **Only when a request would not fit**, \
or **As my Arena account says** (Your account → Answers in Arena), and what the recent part keeps, in percent of the \
window. It always compacts before a request would not fit, and one step early when the next step's answer and \
results would pass the threshold. Before any of it, a tool result the model has answered after twice goes as a \
summary (its key lines; a file read as its outline and the lines cited) and stays whole for **read_result**: \
`\"condenseResults\": false` in config.json turns that off.",
                "Press **Save**: the next sessions use it too.",
            ],
        ),
        recipe(
            "Add to what the agent is doing, without stopping it",
            &[
                "While it works, type in the chat's box and press Enter: the message waits at the end of the thread.",
                "The agent reads it at its next step (after its tool calls) and goes on with it; until then **Edit** \
changes it and **Cancel** takes it out.",
            ],
        ),
        recipe(
            "Stop a long command the agent runs",
            &[
                "While it runs, its output shows above the chat's box.",
                "Press **Stop** beside it: the agent is told it ended.",
            ],
        ),
        recipe(
            "Find what Code Arena said (its log)",
            &[
                "Press **About** at the activity bar's foot, or the version at the status bar's right.",
                "**Log** is where it is: code-arena.log beside the program (in the data folder's logs when that \
folder cannot be written). `code-arena --help` ends with it too.",
                "Each line has its time; the IDE's key is left out. It keeps three older files as it grows.",
            ],
        ),
        recipe(
            "Change a setting, as in VS Code",
            &[
                "Press Ctrl+, (or run **Preferences: Open User Settings (JSON)**): your settings.json, for every \
project.",
                "Type a setting's name: the editor suggests them, with what each does. `\"[python]\": { … }` sets them \
for one language.",
                "Save: it applies at once. The project's own are in .vscode/settings.json (**Preferences: Open \
Workspace Settings (JSON)**).",
            ],
        ),
        recipe(
            "Change a keyboard shortcut",
            &[
                "Press Ctrl+K Ctrl+S: the Keyboard Shortcuts editor. Search for the command, or press **Record \
Keys** and the keys.",
                "Double-click its row, press the new keys (one, or a chord of two) and Enter. The row's menu adds, \
removes or resets keys, or changes when they apply.",
                "It is written into your keybindings.json and applies at once. **Preferences: Open Keyboard \
Shortcuts (JSON)** opens the file: `{ \"key\": \"ctrl+shift+e\", \"command\": \"workbench.action.togglePanel\" }`; \
`\"-command\"` takes a key away.",
            ],
        ),
        recipe(
            "Use a VS Code theme, icons or snippets",
            &[
                "Press Ctrl+Shift+X (**Extensions**).",
                "Press **Install from VSIX…** and choose the extension's .vsix (its page on Open VSX or the \
Marketplace has a download), or import it from **VS Code on this computer**.",
                "Choose its theme with Ctrl+K Ctrl+T (the editor shows each as you go down the list), or its icons \
with **Preferences: File Icon Theme**; its snippets and languages apply at once.",
            ],
        ),
        recipe(
            "Work on several folders at once (a .code-workspace)",
            &[
                "Run **File: Open Workspace from File…** and choose the .code-workspace file (or press **Open \
Workspace** above it in the editor).",
                "Its folders are roots in the Explorer, each with its repository in Source control; Search goes \
through them all. **Add Folder to Workspace…** and the × beside a folder change the file.",
                "Settings has **User**, **Workspace** (the file's settings) and **Folder** tabs. **Save Workspace \
As…** makes a file of the folders open now; **Close Workspace** leaves the folder alone.",
            ],
        ),
        recipe(
            "Run a Jupyter notebook",
            &[
                "Open the .ipynb file (or **Create: New Jupyter Notebook**); trust the folder when it asks (**Manage \
Workspace Trust**).",
                "Press **Run** beside a cell, or Ctrl+Enter in it; Shift+Enter runs it and goes to the next. **Run \
All** runs them all.",
                "The kernel is the project's Python (its .venv when there is one). **Interrupt** stops a cell, \
**Restart** starts it again; Ctrl+S saves the notebook with its outputs.",
            ],
        ),
        recipe(
            "Resolve a merge conflict",
            &[
                "Open **Source control** and the file under **Merge Changes**.",
                "Click **Accept Current Change**, **Accept Incoming Change** or **Accept Both Changes** above each \
conflict, or **Resolve in Merge Editor** to take each side from the panes above the result.",
                "Save, then stage the file (+), or press **Complete Merge** in the merge editor, which does both. \
Commit to finish the merge.",
            ],
        ),
        recipe(
            "Make the project's own snippets, or your own",
            &[
                "Run **Snippets: Configure Snippets** from the palette.",
                "Choose **New Snippets file for this folder…** (in .vscode/, for the team), **New Global Snippets \
file…** (every project), or a language for its own file.",
                "Write the snippet (the example shows how) and save: its prefix is suggested as you type. \
**Snippets: Insert Snippet** lists them all.",
            ],
        ),
        recipe(
            "Get completions, definitions and errors from a language server",
            &[
                "Install the language server for the language (typescript-language-server, pyright, gopls, \
rust-analyzer, clangd…): the status bar says which, and how, when it is missing.",
                "Trust the folder (**Manage Workspace Trust**): a language server runs the folder's code.",
                "Open a file of the language: the server starts, and F12 goes to a definition, Shift+F12 lists the \
references, F2 renames.",
            ],
        ),
        recipe(
            &format!("Use Arena's and {CODE_INTEL}'s tools"),
            &[
                "They connect by themselves once you are signed in: the plug in the status bar counts those \
connected.",
                "Click it to see why one is not, and press **Try again**.",
            ],
        ),
        recipe(
            "Test an app in a virtual machine",
            &[
                "Have VirtualBox, VMware, libvirt or Hyper-V here, the guest tools in the VM (Guest Additions, VMware \
Tools, qemu-guest-agent), and a snapshot of it clean.",
                "Ask the agent, as \"test the build in test-vm from the snapshot clean\": it restores the snapshot, \
copies the build in, runs it, brings back the logs and the screen (into .code-arena/vm), and puts the VM back.",
                "When it needs the guest's user and password, a dialog asks; **Keep it in the keyring** saves them \
for next time (`code-arena vm login VM` does too). The password never shows in the chat.",
            ],
        ),
    ]
}

/// The keys the help lists: the commands each row is about (their keys read from the keymap when the overlay
/// opens), the keys said when none of them has any, and what they do.
pub const KEYS: &[(&[&str], &str, &str)] = &[
    (
        &["workbench.action.files.save", "workbench.action.files.saveAll"],
        "Ctrl+S, Ctrl+K S",
        "Save the file, save them all",
    ),
    (&["workbench.action.quickOpen"], "Ctrl+P", "Open a file by its name (:12 goes to a line)"),
    (&["workbench.action.showCommands"], "Ctrl+Shift+P or F1", "Run a command by its name"),
    (&["workbench.view.search"], "Ctrl+Shift+F", "Search the files"),
    (&["workbench.view.explorer"], "Ctrl+Shift+E", "The Explorer"),
    (&["workbench.view.scm"], "Ctrl+Shift+G", "Source control"),
    (&["workbench.action.terminal.toggleTerminal"], "Ctrl+`", "Show or hide the terminal"),
    (
        &["workbench.action.toggleSidebarVisibility", "workbench.action.togglePanel"],
        "Ctrl+B, Ctrl+J",
        "Show or hide the side bar, the panel",
    ),
    (&[crate::manage::OPEN_SETTINGS], "Ctrl+,", "Settings (settings.json, as VS Code's)"),
    (
        &["workbench.action.openGlobalKeybindings"],
        "Ctrl+K Ctrl+S",
        "Keyboard shortcuts (keybindings.json, as VS Code's)",
    ),
    (&["workbench.action.selectTheme"], "Ctrl+K Ctrl+T", "The colour theme"),
    (&[crate::manage::SHOW_EXTENSIONS], "Ctrl+Shift+X", "Extensions (VS Code's themes, icons, snippets, languages)"),
    (&["workbench.action.splitEditor"], "Ctrl+\\, Ctrl+1…8", "Split the editor, focus an editor group"),
    (&[], "Ctrl+L", "In the editor: the lines chosen go with the next message"),
    (
        &[],
        "↑ / ↓",
        "In the chat's box: what you sent before in this folder (the terminal's too); Esc goes back to what you were typing",
    ),
];

/// A key row's keys: its commands' keys as the keymap has them (`keys_of`), joined; the help's own when none has.
pub fn keys_shown(commands: &[&str], said: &str, keys_of: impl Fn(&str) -> String) -> String {
    let found: Vec<String> = commands.iter().map(|c| keys_of(c)).filter(|k| !k.is_empty()).collect();
    if found.is_empty() { said.to_string() } else { found.join(", ") }
}

/// The help as Markdown, below its title: the part asked about first (with the manual's heading for it, when the
/// manual is on the web), every part, the recipes, and the keys (each row as `keys` gives it).
pub fn markdown(part: Option<&Part>, manual: Option<&str>, keys: impl Fn(&[&str], &str) -> String) -> String {
    let mut md = String::new();
    let book = manual.filter(|m| m.starts_with("http://") || m.starts_with("https://"));
    if let Some(p) = part {
        md += &format!("> **This part: {}**\n>\n> {}\n", p.name, p.text);
        if let Some(b) = book {
            md += &format!(">\n> [More on this in the manual]({b}#{})\n", p.manual);
        }
        md += "\n";
    }
    md += "#### Its parts\n\n";
    for p in &PARTS {
        md += &format!("**{}**\\\n{}\n\n", p.name, p.text);
    }
    md += "#### How to\n\n";
    for r in recipes() {
        md += &format!("**{}**\n\n", r.title);
        for (n, s) in r.steps.iter().enumerate() {
            md += &format!("{}. {s}\n", n + 1);
        }
        md += "\n";
    }
    md += "#### Keys\n\n| Keys | What they do |\n| --- | --- |\n";
    for (commands, said, what) in KEYS {
        // A bar in a key (none today) would end the cell.
        md += &format!("| `{}` | {what} |\n", keys(commands, said).replace('|', "\\|"));
    }
    md
}

/// The words at the overlay's foot when the manual is not on the web.
pub fn no_manual() -> String {
    format!("The whole story is in your {PRODUCT}'s manual: Manual, then Code Arena.")
}

/// Nothing to add: the overlay is `help.rs`'s.
pub fn register(_ui: Ui, _send: &Send) {}

/// Nothing to take.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_part_has_its_words_and_a_heading_of_the_manual() {
        let ids: std::collections::HashSet<_> = PARTS.iter().map(|p| p.id).collect();
        assert_eq!(ids.len(), 23);
        assert!(PARTS.iter().all(|p| !p.name.is_empty() && p.text.len() > 40 && !p.manual.is_empty()));
        assert_eq!(recipes().len(), 21);
    }

    #[test]
    fn each_view_but_three_is_a_part_of_the_help() {
        let told: Vec<View> = crate::layout::VIEWS.into_iter().filter(|v| part_of_view(*v).is_none()).collect();
        assert_eq!(told, [View::Plan, View::Memory, View::Harness]);
        assert_eq!(part_of_view(View::Scm).map(|p| p.name), Some("Source control"));
        assert_eq!(part_of_view(View::Outline).map(|p| p.id), Some("explorer"));
    }

    #[test]
    fn the_keys_are_the_keymaps_and_the_helps_own_where_it_has_none() {
        let keys_of = |c: &str| if c == "workbench.action.quickOpen" { "Ctrl+E".to_string() } else { String::new() };
        assert_eq!(keys_shown(&["workbench.action.quickOpen"], "Ctrl+P", keys_of), "Ctrl+E");
        assert_eq!(keys_shown(&["a", "workbench.action.quickOpen"], "x", keys_of), "Ctrl+E");
        assert_eq!(keys_shown(&["a"], "Ctrl+L", keys_of), "Ctrl+L");
    }

    #[test]
    fn the_help_puts_the_part_asked_about_first_with_the_manual_on_the_web_only() {
        let git = part_of("git");
        let md = markdown(git, Some("https://arena.example/help/code-arena"), |_, said| said.to_string());
        assert!(md.starts_with("> **This part: Source control**"));
        assert!(md.contains("[More on this in the manual](https://arena.example/help/code-arena#source-control)"));
        assert!(md.contains("| `Ctrl+P` | Open a file by its name"));
        let md = markdown(git, Some("ARENA/help/code-arena"), |_, said| said.to_string());
        assert!(!md.contains("More on this in the manual"));
        let md = markdown(None, None, |_, said| said.to_string());
        assert!(md.starts_with("#### Its parts"));
        // Every block is one the answers' renderer draws: the quote, the headings, the steps and the table.
        let blocks = crate::markdown::parse(&markdown(git, None, |_, s| s.to_string()));
        let crate::markdown::Block::Quote(quote) = &blocks[0] else { panic!("{:?}", blocks[0]) };
        assert!(quote.text.starts_with("This part: Source control\n\nThe folder's git"), "{}", quote.text);
        let crate::markdown::Block::Paragraph(first) = &blocks[2] else { panic!("{:?}", blocks[2]) };
        assert!(first.text.starts_with("Automation\nWhat runs on its own."), "{}", first.text);
        assert!(
            blocks.iter().any(|b| matches!(b, crate::markdown::Block::Table { rows, .. } if rows.len() == KEYS.len()))
        );
    }
}
