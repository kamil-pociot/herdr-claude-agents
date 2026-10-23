# 01 · Background sessions as agents

Claude Code background sessions (`claude --bg`, or dispatched from `claude agents`) run inside Claude's daemon, not in a terminal. Herdr only knows agents that run in its panes, so it never sees them. This plugin puts each one in a Herdr pane — a tab running `claude attach <id>` — and keeps those tabs in step with the sessions that exist.

## What you get

- A workspace named **Claude** (created the first time; [04 · Configuration](04-configuration.md) renames it).
- Its first tab, **Claude sessions**, is the watcher: it lists every background session with its state, folder and current activity, and does the work below.
- One tab per background session, named after the session, opened in the session's folder, running `claude attach <id>` ([03 · Session tabs](03-session-tabs.md)).
- In Herdr's Agents sidebar, one `claude` agent per session with the session's name and state ([02 · State and notifications](02-state-and-notifications.md)).

Interactive sessions (`claude` in some terminal) are left out: they already have a terminal, and Herdr sees them there when it is a Herdr pane.

## Following changes

The watcher looks at both sides — `claude agents --json` and `herdr pane list` — and does what the difference calls for:

| Seen | Done |
|---|---|
| a session without a tab | opens its tab |
| a session's name, state or activity changed | reports it; a new name also renames the tab and the sidebar entry |
| a session ended (stopped, removed, finished and cleaned up) | closes its tab, so the agent leaves the sidebar |
| a session's tab is gone but the session isn't | you closed it: it stays closed (below) |

It looks when Claude's files change — the session registry (`~/.claude/sessions/*.json`) and each job's `~/.claude/jobs/<id>/state.json` — after the burst of writes settles (150 ms), and every `poll_seconds` (2 s) anyway, which catches a session whose process died without a file changing. A look costs about 0.1 s of CPU (`claude agents --json`) plus one `herdr pane list`.

## Closing a tab

A tab you close (or leave with `q`, [03](03-session-tabs.md)) stays closed while that session runs, and the sessions tab marks it `(tab closed)`. It comes back when you run **Claude: show background sessions** (the `open` action), which reopens every closed tab and shows the sessions tab. Once the session ends it is forgotten, so a later session is opened as usual.

## Herdr restarts

The background sessions keep running while Herdr is down. On a restart Herdr restores the plugin's tabs as plain shells (same pane id, new terminal), then runs the startup hook, which opens a new sessions tab. The new watcher:

- closes the old sessions tab (a shell now),
- replaces each restored session tab with a fresh `claude attach`, or closes it if that session ended meanwhile,
- reports every session's state again.

It tells a restored pane from its own by the terminal id it recorded when it opened the pane. There is never more than one watcher per Herdr server: it holds a lock, and `startup` / `open` only open a sessions tab when nobody holds it.

## Known issue: Herdr's own Claude hook

Herdr's Claude integration (`herdr integration install claude`) reports a session's id at `SessionStart` to the pane in `HERDR_PANE_ID`. A background session inherits that variable from wherever Claude's daemon was started, so the hook may point a stale pane at it. The plugin doesn't rely on that hook; its reports win on its own panes.

## Code and tests

- `src/claude.rs`: running and parsing `claude agents --json` (background sessions only), each job's `detail`.
- `src/sync.rs`: `plan` — the table above as pure code (`Op::Open`, `Report`, `Close`, `Dismiss`, `Forget`, `Replace`) — and the saved state (`<state dir>/server-<hash>.json`: the watcher pane, each session's pane, the closed tabs).
- `src/watch.rs`: the loop, file watching (`notify`), carrying out the plan, the sessions screen.
- `src/main.rs`: `startup` / `open` — find or create the workspace, open the sessions tab, drop the new workspace's shell tab.
- Unit: every row of the table, restored panes, closed tabs, saved state round trip, which file changes wake the watcher, the sessions screen.
- E2e (`tests/e2e.rs`): sessions become agents in the `Claude` workspace with no stray shell tab; one finishing, one ending, one starting; a rename; a closed tab staying closed until `open`; a Herdr restart with one session ending meanwhile — no leftovers, no second workspace or sessions tab.
