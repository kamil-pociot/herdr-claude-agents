# 03 · Session tabs

Each background session's tab runs `claude attach <id>`: the full Claude UI for that session, with your keys going to it. Select the agent in Herdr's sidebar (or the tab) and work with the session as in any terminal; the session keeps running in the background whether or not anything is attached.

The tab is a plugin pane, not a shell, which matters when you leave the session:

| Do | Result |
|---|---|
| `←` (Claude's agent list) | "The agent list isn't available here: this tab shows *name* only. Enter: attach again · q: close this tab" |
| `Ctrl-Z` | "*name* is still running in the background. Enter: attach again · q: close this tab": with no shell to return to, a stopped attach would freeze the tab, so it ends the attach instead (the session itself is not touched) |
| `Enter` at that prompt | attaches again |
| `q` at that prompt | closes the tab; it stays closed until the `open` action ([01](01-sessions-as-agents.md#closing-a-tab)) |
| the session ends | the tab closes by itself |

**One session per tab.** In a plain terminal, `←` takes `claude attach` to Claude's agent list, where any session can be opened — in this tab that would show one session while Herdr's sidebar reports another. Claude does it by relaunching the attach process in place as `claude agents`, so the tab checks its process's arguments every 100 ms (`KERN_PROCARGS2` on macOS, `/proc/<pid>/cmdline` on Linux) and ends it the moment `attach` is gone from them, before anything else can be opened. `←` while typing a prompt still moves the cursor: only the relaunch counts. The other sessions are in Herdr's Agents sidebar.

The tab sits in the space of the session's project ([01](01-sessions-as-agents.md#where-a-session-goes)), is named after the session, and opens in the session's folder (when it still exists). Its terminal is reset after each attach (cooked mode, main screen, visible cursor) so the prompt is readable whatever Claude left behind.

If the tab cannot start (no `claude`, a bad `config.toml`), it shows the error and waits for Enter instead of closing at once — a tab that vanished would look like one you closed.

## Code and tests

- `src/attach.rs`: the attach loop (`waitpid` with `WNOHANG | WUNTRACED` every 100 ms), stops, spotting the agent list (`args_of`, `in_agent_list`), ending the process (SIGTERM, SIGKILL after 2 s), terminal restore, the prompt.
- Unit: what counts as the agent list (also for a `claude` that is a script), reading a live process's arguments, parsing `KERN_PROCARGS2`.
- `herdr-plugin.toml`: the `session` pane; commands go through `$HERDR_PLUGIN_ROOT` because the tab's working directory is the session's folder.
- E2e: the tab runs `claude attach <id>` (a fake `claude` echoes it); the tab opens in the session's folder; `←` (the fake relaunching itself as `claude agents`) brings the prompt, the agent stays on its session, Enter attaches the same session again.

Related: [01 · Background sessions as agents](01-sessions-as-agents.md), [02 · State and notifications](02-state-and-notifications.md).
