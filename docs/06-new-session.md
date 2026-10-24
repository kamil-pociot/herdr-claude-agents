# 06 · New background session

**Claude: new background session** (`kamilpociot.claude-agents.new`) opens a dialog over the current space that starts a Claude Code background session in a folder of it — with `claude --bg`, or a command of your own such as an alias that starts a particular agent. The new session then gets its tab like any other ([01](01-sessions-as-agents.md)), and the dialog shows it.

## The dialog

```
 New Claude background session
 in space lms-context · ~/Repositories/lms-context

 › Folder   apps/web/
            api/  web/
   Command  claude --bg
   Prompt   (optional) the first task

 Tab complete / next · ↑↓ field, recent commands · Enter start · Esc cancel
```

| Field | What it takes |
|---|---|
| Folder | relative to the space's folder (empty is the space's folder itself); `~/…` and absolute paths work too. Matching subfolders are listed under it as you type (hidden ones once you type a `.`) |
| Command | what to run there; `claude --bg` (with the `claude` from [04 · Configuration](04-configuration.md)) until you use another, then the last one you used. It runs in your interactive shell (`$SHELL -ic`), so aliases and functions from your shell's rc file work: `po` |
| Prompt | optional; added to the command as one quoted argument (`claude --bg 'fix the login'`) |

| Key | Does |
|---|---|
| typing, `Backspace` | edit the field |
| `Tab` | in Folder: complete the name as far as the matches agree, a single match with its `/`; when there is nothing to complete, and in the other fields: next field |
| `Shift-Tab` | previous field |
| `↑` / `↓` | in Command: older / newer recent commands (the last ten are kept); elsewhere: previous / next field |
| `Ctrl-U` / `Ctrl-W` | clear the field / delete its last word or folder name |
| `Enter` | start (a folder that doesn't exist, or an empty command, is said so instead) |
| `Esc`, `Ctrl-C` | close without starting |

## Starting

The command runs in the chosen folder with its output on the dialog's screen, so a question it asks (Claude's folder-trust prompt, say) can be answered there. `HERDR_*` variables are left out of its environment, so a session — or a Claude daemon started by it — doesn't carry the dialog's Herdr identity around ([01 · Known issue](01-sessions-as-agents.md#known-issue-herdrs-own-claude-hook)).

Then the dialog waits up to 20 s for a session `claude agents --json` didn't list before (preferring one in the chosen folder), and up to 10 s more for the watcher to give it its tab — in the space of its project, created when there is none ([01](01-sessions-as-agents.md#where-a-session-goes)) — and shows that tab. A command that fails, or no session appearing, leaves a message until Enter.

## Setup

Bind the action to a key in Herdr's `config.toml` ([04](04-configuration.md#the-actions)):

```toml
[[keys.command]]
key = "prefix+a"
type = "plugin_action"
command = "kamilpociot.claude-agents.new"
description = "new Claude background session"
```

The action passes the space's folder and name (from Herdr's plugin context, `workspace_cwd` and `workspace_label`) to the dialog, a `popup` pane (72% wide, 18 rows) over the current layout. Recent commands are kept in `$HERDR_PLUGIN_STATE_DIR/commands.json`.

## Code and tests

- `src/new.rs`: `open` (the action: the space from the context, opening the popup), `Form` (fields, keys, completion, the folder, the command line, drawing), `run` / `ask` / `start` (the terminal, running the command, waiting for the session and its tab).
- `src/herdr.rs`: `open_popup` (the CLI's `--placement` has no `popup`, so the manifest's is used).
- `herdr-plugin.toml`: the `new` action and the `new-session` popup pane.
- Unit: completion (common prefix, single match with `/`, case, hidden folders), the folder relative to the space (and `~`, absolute, missing, a file), the prompt as one quoted argument, moving between fields and editing, recent commands (default, ↑↓, remembering), the space from Herdr's context, drawing and the cursor.
- E2e: the dialog in a pane of an isolated Herdr (a popup is outside Herdr's API): a folder completed with Tab, a command of your own and a prompt; the command runs in that folder through the shell with the prompt as one argument; the new session's tab opens in the space and is shown; the command is remembered. The action itself opening its popup is checked by hand.

Related: [01 · Background sessions as agents](01-sessions-as-agents.md), [04 · Install and configuration](04-configuration.md).
