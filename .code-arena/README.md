# .code-arena

Code Arena's folder for this project. Code Arena made it the first time the project was
opened, and never writes over a file you have changed.

| path | what it is | commit it? |
|---|---|---|
| `settings.json` | the project's settings: rules that deny, hooks, tool settings | yes |
| `mcp.json` | the project's MCP servers; each person enables them before they run | yes |
| `commands/NAME.md` | `/NAME` commands; `$ARGUMENTS` is what was typed after the name | yes |
| `agents/NAME.md` | sub-agents the agent can hand work to | yes |
| `skills/NAME/SKILL.md` | skills, in the agentskills.io format | yes |
| `checklists/*.md` | the repository's checklists: a `# ` heading and `- ` items | yes |
| `tools/*.json` | the project's own command tools | yes |
| `hooks/` | scripts that hooks in `settings.json` run | yes |
| `local/`, `media/`, `plots/`, `vm/` | each person's own state and outputs | no (`.gitignore` keeps them out) |

The project's instructions for the agent go in `ARENA.md` at the repository's root
(`AGENTS.md` is read too, and other agents' own files once `compat.agents` names them).

A project's settings can only make Code Arena more careful: they can deny tools and make
them ask, never let them run unasked. Commands, tools, MCP servers and hooks that the
project brings run only after you enable them on your machine.
