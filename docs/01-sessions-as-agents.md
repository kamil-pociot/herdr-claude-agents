# 01 · Background sessions as agents

Claude Code background sessions (`claude --bg`, or dispatched from `claude agents`) run inside Claude's daemon, not in a terminal. Herdr only knows agents that run in its panes, so it never sees them. This plugin puts each one in a Herdr pane — a tab running `claude attach <id>` — in the space (workspace) of the project it works in, and keeps those tabs in step with the sessions that exist.

## Where a session goes

Every session runs in a folder. Its **project** is the top of that folder's git repository (a worktree's own top for a worktree), or the folder itself outside git. The session's tab goes into:

1. the space with a pane in the session's own folder, else
2. the first space with a pane anywhere in its project, else
3. a **new space** for the project, created like any other (Herdr names it after the folder, and it starts with its usual shell tab).

Spaces are matched by their panes' folders because Herdr's spaces have none of their own. A project that is your home folder or `/` only matches a pane exactly there, so a shell sitting in `~` doesn't collect every session. The tab is named after the session and opens in the session's folder ([03 · Session tabs](03-session-tabs.md)).

In the Agents sidebar each session is a `claude` agent like any other, under its space ([02 · State and notifications](02-state-and-notifications.md)).

Interactive sessions (`claude` in some terminal) are left out: they already have a terminal, and Herdr sees them there when it is a Herdr pane.

## Following changes

A watcher looks at both sides — `claude agents --json` and `herdr pane list` — and does what the difference calls for:

| Seen | Done |
|---|---|
| a session without a tab | opens its tab (above) |
| a session's name, state or activity changed | reports it; a new name also renames the tab |
| a session ended (stopped, removed, finished and cleaned up) | closes its tab, so the agent leaves the sidebar (a space created for it keeps its shell tab) |
| a session's tab is gone but the session isn't | you closed it: it stays closed (below) |

It looks when Claude's files change — the session registry (`~/.claude/sessions/*.json`) and each job's `~/.claude/jobs/<id>/state.json` — after the burst of writes settles (150 ms), and every `poll_seconds` (2 s) anyway, which catches a session whose process died without a file changing. A look costs about 0.1 s of CPU (`claude agents --json`) plus one `herdr pane list`.

## The watcher

There is one watcher per Herdr server, with no pane of its own. Herdr's startup hook runs it when the server starts; **Claude: show background sessions** (the `open` action) runs it when none runs, for example right after `herdr plugin link`. It holds a lock in the plugin's state dir, so a second one exits at once.

Herdr doesn't stop plugin commands with the server, so the watcher stops itself: when the server is gone (stopped, or restarting) and when the plugin is disabled or unlinked (checked every 15 looks). A watcher starting with a new server waits up to 15 s for the old one to notice.

Its errors (no `claude`, a bad `config.toml`) go to the plugin's log (`herdr plugin log list --plugin kamilpociot.claude-agents`) and stay in its state file (`"error"`) until a look succeeds.

## Closing a tab

A tab you close (or leave with `q`, [03](03-session-tabs.md)) stays closed while that session runs. It comes back when you run **Claude: show background sessions**, which reopens every closed tab. Once the session ends it is forgotten, so a later session is opened as usual.

## Herdr restarts

The background sessions keep running while Herdr is down. On a restart Herdr restores the plugin's tabs as plain shells (same pane id, new terminal); the new watcher replaces each with a fresh `claude attach`, or closes it if that session ended meanwhile, and reports every session's state again. It tells a restored pane from its own by the terminal id it recorded when it opened the pane.

## Known issue: Herdr's own Claude hook

Herdr's Claude integration (`herdr integration install claude`) reports a session's id at `SessionStart` to the pane in `HERDR_PANE_ID`. A background session inherits that variable from wherever Claude's daemon was started, so the hook may point a stale pane at it. The plugin doesn't rely on that hook; its reports win on its own panes.

## Code and tests

- `src/claude.rs`: running and parsing `claude agents --json` (background sessions only), each job's `detail`.
- `src/spaces.rs`: a session's project (`git rev-parse --show-toplevel`) and the space for it.
- `src/sync.rs`: `plan` — the table above as pure code (`Op::Open`, `Report`, `Close`, `Dismiss`, `Forget`, `Replace`) — and the saved state (`<state dir>/server-<hash>.json`: each session's pane and name, the closed tabs, the last error).
- `src/watch.rs`: the loop, file watching (`notify`), carrying out the plan, creating spaces, stopping with the server or the plugin.
- `src/main.rs`: `startup` / `open`.
- Unit: every row of the table, restored panes, closed tabs, saved state (also v0.1's), choosing a space (own folder first, project, home and `/` only exactly), the git top level, which file changes wake the watcher.
- E2e (`tests/e2e.rs`): a session in a repository's subfolder lands in the space open on the repository, one elsewhere gets a new space named after its folder, no `Claude` space; one finishing, one ending, one starting; a rename; a closed tab staying closed until `open`; a Herdr restart with one session ending meanwhile — no leftovers, no extra space — and the watcher stopping with the server; disabling the plugin stopping the watcher.
