# 02 · State and notifications

Herdr's notifications, attention rollups (a blocked agent makes its tab and workspace look blocked), Done badges and `herdr agent wait` all come from an agent's lifecycle state. For a session tab, the plugin reports that state from Claude itself rather than letting Herdr read it off the screen, so it is right even while the tab shows Claude's agent view, and a permission prompt shows as blocked.

## State

The session's `state` and its process `status` from `claude agents --json` decide (the status wins over a stale state: a session that ended its turn is `idle` while its state may still read `working`):

| Claude | Plugin | Herdr state | Herdr shows |
|---|---|---|---|
| `blocked`, or status `waiting` | needs input | `blocked` | attention; label "needs input" |
| `failed` | failed | `idle` | label "failed" |
| `stopped` | stopped | `idle` | label "stopped" |
| `done` / `completed`, or status `idle` | done | `idle` | `done` until you look at it |
| anything else (`working`, status `busy` / `shell`) | working | `working` | working |

A `working` → `idle` change is a finished turn, which Herdr notifies about and marks Done until seen; `→ blocked` asks for attention. These are Herdr's own rules, so its notification settings (`[ui.toast]`, sounds) apply as for any agent.

Reports use source `claude-agents` and agent `claude`, numbered by time so a late report never overwrites a newer one. Holding the pane this way makes Herdr skip screen detection there (`screen_detection_skipped`). Only changes are reported; a new watcher reports everything once.

## What the sidebar shows

Display-only extras go with source `claude-agents.display`:

| Field | Value |
|---|---|
| displayed name | the session's name ("fix auth") |
| title | the session's folder name |
| state labels | `blocked` = "needs input"; `idle` / `done` = "failed" or "stopped" for those |
| `$summary` | what it is doing now, from `jobs/<id>/state.json` (`detail`: "Reading src/main.rs") |
| `$project` | the folder name |
| `$claude_state` | the plugin's state word (`working`, `needs input`, `done`, …) |
| `$claude_session` | the short id (`claude attach <id>`) |

The tokens can be used in custom Agent sidebar rows (see Herdr's configuration docs).

Each agent also gets a **name** — the session name made Herdr-safe (`fix auth (2)` → `fix-auth-2`; a clash gets the id appended) — so scripts and other agents can address it: `herdr agent prompt fix-auth "…" --wait`, `herdr agent wait fix-auth --until blocked`.

## Code and tests

- `src/claude.rs`: `State::parse` (state + status), `State::herdr`, `State::label`, `detail`.
- `src/watch.rs`: `report` (state, display, tab rename), `display`, `Seq`; agent names in `open`.
- `src/herdr.rs`: `report_agent`, `report_display`, `rename_agent`, `agent_slug`.
- Unit: every state/status pair, the Herdr state of each, labels and tokens, agent names, ever-growing report numbers.
- E2e: a working and a waiting session show as `working` and `blocked`; name, title, `$summary`, `$claude_session`, agent name; an `idle` status turning the agent `done`.

Related: [01 · Background sessions as agents](01-sessions-as-agents.md).
