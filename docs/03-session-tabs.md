# 03 · Session tabs

Each background session's tab runs `claude attach <id>`: the full Claude UI for that session, with your keys going to it. Select the agent in Herdr's sidebar (or the tab) and work with the session as in any terminal; the session keeps running in the background whether or not anything is attached.

The tab is a plugin pane, not a shell, which matters when you leave the session:

| Do | Result |
|---|---|
| `←` | Claude's agent list, as in any terminal |
| open another session from that list | **that session's own tab** is shown (opened first if it has none), and this tab goes back to its session |
| open this tab's session from that list | it stays, as usual |
| `Esc` in the list (quits it), or `Ctrl-Z` | "*name* is still running in the background. Enter: attach again · q: close this tab". With no shell to return to, a stopped attach would freeze the tab, so `Ctrl-Z` ends the attach instead (the session itself is not touched) |
| `Enter` at that prompt | attaches again |
| `q` at that prompt | closes the tab; it stays closed until the `open` action ([01](01-sessions-as-agents.md#closing-a-tab)) |
| the session ends | the tab closes by itself |

**One session per tab.** Left alone, the agent list would open another session inside this tab, which would then show one session while Herdr's sidebar reports another. So every session opened from a tab's agent list goes to its own tab, and both stay in the sidebar, each selectable.

How it works: on `←`, Claude relaunches the attach process in place as `claude agents`, and an opened session titles the terminal with its name (the list itself is titled `claude agents`). The tab checks its process's arguments every 100 ms (`KERN_PROCARGS2` on macOS, `/proc/<pid>/cmdline` on Linux); while they no longer say `attach`, it reads the pane's title from Herdr every 100 ms — or, with no title at all, the name in the rule under Claude's prompt on screen (`──── fix auth ─`). A name that isn't this tab's session is looked up in the watcher's state, which keeps every tabbed session's name (only a session too new for it costs a `claude agents --json`). Then Herdr focuses that session's tab at once (`herdr agent focus`) — asking the watcher for it and waiting up to 6 s when it has none, which also reopens other tabs you closed — and only then is the list ended (SIGTERM, SIGKILL after 0.5 s) and this tab's own session attached again. From Enter in the list to the other tab in focus takes about 0.1 s (0.8 s when the focus waited for Claude to quit). `←` while typing a prompt still moves the cursor: only the relaunch counts.

The tab sits in the space of the session's project ([01](01-sessions-as-agents.md#where-a-session-goes)), is named after the session, and opens in the session's folder (when it still exists). Its terminal is reset after each attach (cooked mode, main screen without moving the cursor, visible cursor) so the prompt is readable whatever Claude left behind.

If the tab cannot start (no `claude`, a bad `config.toml`), it shows the error and waits for Enter instead of closing at once — a tab that vanished would look like one you closed.

## Code and tests

- `src/attach.rs`: the attach loop (`waitpid` with `WNOHANG | WUNTRACED` every 100 ms), stops, spotting the agent list (`args_of`, `in_agent_list`), which session it shows (`shown`, `opened_elsewhere`), showing the other session's tab (`show`), ending the process (SIGTERM, SIGKILL after 0.5 s), terminal restore, the prompt.
- `src/herdr.rs`: `pane_title`, `pane_screen`, `focus_agent`.
- `herdr-plugin.toml`: the `session` pane; commands go through `$HERDR_PLUGIN_ROOT` because the tab's working directory is the session's folder.
- Unit: what counts as the agent list (also for a `claude` that is a script), the open session from a title or a screen (the list itself shows none), reading a live process's arguments, parsing `KERN_PROCARGS2`.
- E2e: the tab runs `claude attach <id>` (a fake `claude` echoes it); the tab opens in the session's folder; `←` (the fake relaunching itself as `claude agents`) shows the list, opening the tab's own session there changes nothing, opening another focuses that session's tab and puts this one back on its session, both agents staying.

Related: [01 · Background sessions as agents](01-sessions-as-agents.md), [02 · State and notifications](02-state-and-notifications.md).
