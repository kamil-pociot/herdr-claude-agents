# 02 · State and notifications

A session's agent looks like any Claude in Herdr's Agents sidebar: agent `claude`, in its space and tab, with Herdr's own marks and labels. What the plugin adds is the **state**: Herdr's notifications, attention rollups (a blocked agent makes its tab and space look blocked), Done badges and `herdr agent wait` all come from an agent's lifecycle state, and for a session tab the plugin reports it from Claude itself rather than letting Herdr read it off the screen. So it is right even while the tab shows Claude's agent view, and a permission prompt shows as blocked.

## State

The session's `state` and its process `status` from `claude agents --json` decide (the status wins over a stale state: a session that ended its turn is `idle` while its state may still read `working`):

| Claude | Herdr state |
|---|---|
| `blocked`, or status `waiting` | `blocked` |
| `done` / `completed`, or status `idle` | `idle` (`done` until you look at it) |
| `failed`, `stopped` | `idle` |
| anything else (`working`, status `busy` / `shell`) | `working` |

A `working` → `idle` change is a finished turn, which Herdr notifies about and marks Done until seen; `→ blocked` asks for attention. These are Herdr's own rules, so its notification settings (`[ui.toast]`, sounds) apply as for any agent.

Reports use source `claude-agents` and agent `claude`, numbered by time so a late report never overwrites a newer one. Holding the pane this way makes Herdr skip screen detection there (`screen_detection_skipped`). Only changes are reported; a new watcher reports everything once.

## Nothing else on display

No display name, title, state labels or agent name are set, and the plugin pane's label ("Claude session") is cleared, so the default sidebar rows show the agent exactly as they show a Claude you started yourself. The session's name is on its **tab**.

For custom Agent sidebar rows, each session's pane carries tokens (source `claude-agents.tokens`), which the default rows don't show:

| Token | Value |
|---|---|
| `$summary` | what it is doing now, from `jobs/<id>/state.json` (`detail`: "Reading src/main.rs") |
| `$project` | the session's folder name |
| `$claude_state` | `working`, `needs input`, `done`, `failed` or `stopped` |
| `$claude_session` | the short id (`claude attach <id>`) |

## Code and tests

- `src/claude.rs`: `State::parse` (state + status), `State::herdr`, `State::label`, `detail`.
- `src/watch.rs`: `report` (state, tokens, tab rename), `tokens`, `Seq`; clearing the pane label in `open`.
- `src/herdr.rs`: `report_agent`, `report_tokens`, `clear_pane_label`.
- Unit: every state/status pair, the Herdr state of each, the tokens, ever-growing report numbers.
- E2e: a working and a waiting session show as `working` and `blocked`; the agents have no display name, name, title, labels or pane label; the tokens; an `idle` status turning the agent `done`.

Related: [01 · Background sessions as agents](01-sessions-as-agents.md).
