# herdr-claude-agents — Decisions & Roadmap

Goal: Herdr detects Claude Code background sessions and shows them as agents — visible in the Agents sidebar, with Herdr's notifications and other agent features working for them — and the list follows sessions as they start, change and end.

> Status: **v0.1 implemented** (2026-10-23). Herdr 0.9.5, Claude Code 2.1.309.

---

## 0. Decisions (2026-10-23)

| # | Topic | Decision |
|---|---|---|
| 1 | Language | **Rust**, one binary with subcommands; Herdr runs it through `bin/herdr-claude-agents`. |
| 2 | Making sessions agents | Herdr agents only exist in panes, so **each session gets a pane** (a tab) running `claude attach <id>`. That also gives the full Claude UI for answering and approving. |
| 3 | Where | A dedicated **`Claude` workspace** (configurable), one tab per session plus the sessions tab. Not mixed into project workspaces: the plugin must be free to open and close these tabs. |
| 4 | State | **Reported from Claude** (`claude agents --json` state + status) with `pane report-agent`, not read off the screen: right while the tab shows Claude's agent view, and permission prompts count as blocked. Notifications, rollups and waits follow from it. |
| 5 | Long-running process | The watcher is a **plugin pane** (the "Claude sessions" tab), opened by the startup hook. Herdr's docs say startup hooks are one-shot and plugins must not leave detached daemons; a pane is supervised, visible, and dies with the server. |
| 6 | Noticing changes | **File events** on `~/.claude/sessions` and `~/.claude/jobs/*/state.json`, debounced, plus a **2 s poll** (`claude agents --json` takes ~0.1 s). |
| 7 | Closed tabs | A tab the user closes **stays closed** until the session ends or the `open` action runs; reopening it every 2 s would make it impossible to close. |
| 8 | Restarts | Herdr restores plugin panes as **shells**; the new watcher replaces them, telling them apart by **terminal id**. No reliance on Herdr's resume commands: the sessions outlive Herdr anyway. |
| 9 | Interactive sessions | **Left out**: they already run in a terminal; when that's a Herdr pane, Herdr sees them there. |
| 10 | Talking to Herdr | The **CLI** (`HERDR_BIN_PATH`), as Herdr recommends for plugins, not the raw socket. |

## 1. Findings

**Herdr (0.9.5)**

- Plugins: manifest `herdr-plugin.toml` with `[[startup]]`, `[[actions]]`, `[[events]]`, `[[panes]]`; the whole CLI is the plugin API. `plugin pane open --placement tab --workspace W --cwd D --env K=V` opens a pane entrypoint as a tab; there is no "list plugin panes", so the plugin remembers its panes.
- Plugin panes get the entrypoint's `title` as their pane label. The working directory of their command is the `--cwd` given, so manifest commands must not use relative paths (`$HERDR_PLUGIN_ROOT` instead).
- `pane report-agent --source S --agent claude --state …` takes the pane's lifecycle state from screen detection; it prints nothing on success. `--agent-session-id` from a custom source is accepted but not exposed as `agent_session`.
- `pane report-metadata`: display name, title, state labels, tokens; `--clear-state-labels` cannot be combined with `--state-label` (labels sent replace the old ones).
- A pane running `claude attach` is detected as a `claude` agent by Herdr's screen manifest even without reports.
- On restart, plugin panes come back as shells: same pane id and label, new terminal id.
- `XDG_CONFIG_HOME` + `XDG_STATE_HOME` fully isolate a Herdr server (socket, plugin registry, plugin state) — used by the tests.

**Claude Code (2.1.309)**

- `claude agents --json`: active sessions (`--all` adds finished ones) with `id`, `pid`, `cwd`, `kind` (`background` / `interactive`), `sessionId`, `name`, `status` (`busy`, `idle`, `waiting`, `shell`), `state` (`working`, `done`, `blocked`, …).
- `claude attach <id|name>`: `←` goes to the agent view, `Ctrl-Z` drops to the shell; the session keeps running either way.
- `~/.claude/sessions/<pid>.json`: the registry, updated on status changes. `~/.claude/jobs/<id>/state.json`: a background job's state with a one-line `detail`.
- Background sessions inherit `HERDR_*` from wherever the daemon started, so Herdr's own `SessionStart` hook reports them to a stale pane ([docs/01](docs/01-sessions-as-agents.md#known-issue-herdrs-own-claude-hook)).

## 2. Next

- Optionally place a session's tab in the workspace of its project (matching `cwd`) instead of the `Claude` workspace.
- Keys in the sessions tab: Enter to jump to a session's tab, `a` to reopen closed tabs.
- Publish on GitHub with the `herdr-plugin` topic so `herdr plugin install` and the marketplace find it.
- Watch for Herdr adding runtime pane listing or plugin storage APIs, which would simplify the saved state.
