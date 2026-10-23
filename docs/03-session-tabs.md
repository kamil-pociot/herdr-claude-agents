# 03 · Session tabs

Each background session's tab runs `claude attach <id>`: the full Claude UI for that session, with your keys going to it. Select the agent in Herdr's sidebar (or the tab) and work with the session as in any terminal; the session keeps running in the background whether or not anything is attached.

The tab is a plugin pane, not a shell, which matters when you leave the session:

| Do | Result |
|---|---|
| `←` (Claude's agent view), then quit it | "*name* is still running in the background. Enter: attach again · q: close this tab" |
| `Ctrl-Z` | the same: with no shell to return to, a stopped attach would freeze the tab, so it ends the attach instead (the session itself is not touched) |
| `Enter` at that prompt | attaches again |
| `q` at that prompt | closes the tab; it stays closed until the `open` action ([01](01-sessions-as-agents.md#closing-a-tab)) |
| the session ends | the tab closes by itself |

From Claude's agent view you can attach to a different session in the same tab; the tab keeps its name, and Herdr keeps showing the state of the tab's own session.

The tab sits in the space of the session's project ([01](01-sessions-as-agents.md#where-a-session-goes)), is named after the session, and opens in the session's folder (when it still exists). Its terminal is reset after each attach (cooked mode, main screen, visible cursor) so the prompt is readable whatever Claude left behind.

If the tab cannot start (no `claude`, a bad `config.toml`), it shows the error and waits for Enter instead of closing at once — a tab that vanished would look like one you closed.

## Code and tests

- `src/attach.rs`: the attach loop, stop handling (`waitpid` with `WUNTRACED`), terminal restore, the prompt.
- `herdr-plugin.toml`: the `session` pane; commands go through `$HERDR_PLUGIN_ROOT` because the tab's working directory is the session's folder.
- E2e: the tab runs `claude attach <id>` (a fake `claude` echoes it); the tab opens in the session's folder.

Related: [01 · Background sessions as agents](01-sessions-as-agents.md), [02 · State and notifications](02-state-and-notifications.md).
