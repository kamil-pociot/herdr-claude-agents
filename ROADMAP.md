# herdr-claude-agents — Decisions & Roadmap

Goal: Herdr detects Claude Code background sessions and shows them as agents — visible in the Agents sidebar, with Herdr's notifications and other agent features working for them — and the list follows sessions as they start, change and end.

> Status: **v0.2 implemented** (2026-10-23): sessions in their projects' spaces, shown like any Claude, a headless watcher. Herdr 0.9.5, Claude Code 2.1.309.

---

## 0. Decisions (2026-10-23)

| # | Topic | Decision |
|---|---|---|
| 1 | Language | **Rust**, one binary with subcommands; Herdr runs it through `bin/herdr-claude-agents`. |
| 2 | Making sessions agents | Herdr agents only exist in panes, so **each session gets a pane** (a tab) running `claude attach <id>`. That also gives the full Claude UI for answering and approving. |
| 3 | Where | **The space of the session's project** (v0.2; v0.1 used a dedicated `Claude` workspace): the space with a pane in the session's git repository — preferring its exact folder — or a new space for that folder. The sessions run in projects the user already has spaces for, and should show up there. |
| 4 | State | **Reported from Claude** (`claude agents --json` state + status) with `pane report-agent`, not read off the screen: right while the tab shows Claude's agent view, and permission prompts count as blocked. Notifications, rollups and waits follow from it. |
| 5 | Long-running process | The watcher **is the startup hook** (or the `open` action when none runs), headless, and **stops itself** when the server goes or the plugin is disabled. v0.1 ran it in a "Claude sessions" tab, which needed a home space; without a `Claude` space it had none. It is not a detached daemon: the hook process itself runs on, which Herdr allows; Herdr doesn't stop it with the server, hence the self-stop. |
| 6 | Noticing changes | **File events** on `~/.claude/sessions` and `~/.claude/jobs/*/state.json`, debounced, plus a **2 s poll** (`claude agents --json` takes ~0.1 s). |
| 7 | Closed tabs | A tab the user closes **stays closed** until the session ends or the `open` action runs; reopening it every 2 s would make it impossible to close. |
| 8 | Restarts | Herdr restores plugin panes as **shells**; the new watcher replaces them, telling them apart by **terminal id**. No reliance on Herdr's resume commands: the sessions outlive Herdr anyway. |
| 9 | Interactive sessions | **Left out**: they already run in a terminal; when that's a Herdr pane, Herdr sees them there. |
| 10 | Talking to Herdr | The **CLI** (`HERDR_BIN_PATH`), as Herdr recommends for plugins, not the raw socket. |
| 11 | Display | **Like any Claude** in the Agents sidebar (v0.2): no display name, title, state labels or agent name; the plugin pane label cleared. The session's name is on its tab; extras are `$tokens` for custom rows. |
| 12 | One session per tab | `←` keeps opening Claude's agent list in a session tab, but **a session opened there is shown in its own tab** (focused through Herdr) and the tab goes back to its session, so each tab shows what Herdr reports about it and both stay in the sidebar. The tab notices the relaunch as `claude agents` and then the opened session's name in the terminal title (or on screen). A first version simply ended the agent list; there is no Claude option to turn off only the agent view. |

## 1. Findings

**Herdr (0.9.5)**

- Plugins: manifest `herdr-plugin.toml` with `[[startup]]`, `[[actions]]`, `[[events]]`, `[[panes]]`; the whole CLI is the plugin API. `plugin pane open --placement tab --workspace W --cwd D --env K=V` opens a pane entrypoint as a tab; there is no "list plugin panes", so the plugin remembers its panes.
- Plugin panes get the entrypoint's `title` as their pane label. The working directory of their command is the `--cwd` given, so manifest commands must not use relative paths (`$HERDR_PLUGIN_ROOT` instead).
- `pane report-agent --source S --agent claude --state …` takes the pane's lifecycle state from screen detection; it prints nothing on success. `--agent-session-id` from a custom source is accepted but not exposed as `agent_session`.
- `pane report-metadata`: display name, title, state labels, tokens; `--clear-state-labels` cannot be combined with `--state-label` (labels sent replace the old ones).
- A pane running `claude attach` is detected as a `claude` agent by Herdr's screen manifest even without reports.
- On restart, plugin panes come back as shells: same pane id and label, new terminal id.
- Workspaces have no folder (`workspace list` / `session.snapshot` carry none); panes do (`cwd`). A workspace closes when its last pane closes.
- Startup hooks and actions are not stopped with the server: a long-running one outlives it unless it stops itself. `plugin list --json --plugin <id>` lists nothing for an unlinked plugin; the CLI's error for a stopped server is `server_not_running`.
- `pane rename <pane> --clear` removes a plugin pane's label.
- `XDG_CONFIG_HOME` + `XDG_STATE_HOME` fully isolate a Herdr server (socket, plugin registry, plugin state) — used by the tests.

**Claude Code (2.1.309)**

- `claude agents --json`: active sessions (`--all` adds finished ones) with `id`, `pid`, `cwd`, `kind` (`background` / `interactive`), `sessionId`, `name`, `status` (`busy`, `idle`, `waiting`, `shell`), `state` (`working`, `done`, `blocked`, …).
- `claude attach <id|name>`: `←` goes to the agent view by **relaunching the same process as `claude agents`** (same pid; `CLAUDE_CODE_AGENT_VIEW_RELAUNCH`); opening a session there attaches it inside that process (no new relaunch) and titles the terminal with its name (the list is titled `claude agents`); `Ctrl-Z` drops to the shell; the session keeps running either way. No option turns off just the agent view (`CLAUDE_CODE_DISABLE_AGENT_VIEW` / `disableAgentView` turn off the whole background feature).
- `~/.claude/sessions/<pid>.json`: the registry, updated on status changes. `~/.claude/jobs/<id>/state.json`: a background job's state with a one-line `detail`.
- Background sessions inherit `HERDR_*` from wherever the daemon started, so Herdr's own `SessionStart` hook reports them to a stale pane ([docs/01](docs/01-sessions-as-agents.md#known-issue-herdrs-own-claude-hook)).

## 2. Next

- Optionally close a space the plugin created once its last session ends and nothing else was opened there.
- Add the `herdr-plugin` topic on GitHub so the marketplace lists it (the repository is public at github.com/kamil-pociot/herdr-claude-agents; `herdr plugin install` already works).
- Watch for Herdr adding runtime pane listing or plugin storage APIs, which would simplify the saved state.
