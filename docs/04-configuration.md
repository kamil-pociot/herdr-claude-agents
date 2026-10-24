# 04 · Install and configuration

## Install

Needs Herdr 0.9.2+ (agent reports and plugin panes), Claude Code with background sessions (`claude agents --json`), git (to find a session's project), and Rust to build.

```sh
cargo build --release
herdr plugin link "$PWD"                                     # this checkout, for every Herdr session
herdr plugin action invoke kamilpociot.claude-agents.open    # start now, or restart the Herdr server
```

Linking registers the checkout; Herdr runs `bin/herdr-claude-agents`, which runs `target/release/herdr-claude-agents`, so rebuild after a change. A running watcher keeps the old binary until it stops: disable and enable the plugin (`herdr plugin disable` / `enable`, then the action) or restart Herdr.

Without a checkout, `herdr plugin install kamil-pociot/herdr-claude-agents` clones the repository into a Herdr-managed folder and runs the manifest's build step (`cargo build --release --locked`); `herdr plugin update` fetches and rebuilds it. Installing over a linked checkout is refused: unlink first.

`herdr plugin unlink kamilpociot.claude-agents` removes it; the watcher stops within about 30 s, and the session tabs stay until you close them.

## The actions

- **Claude: show background sessions** (`kamilpociot.claude-agents.open`) reopens session tabs you closed, and starts the watcher when none runs.
- **Claude: new background session** (`kamilpociot.claude-agents.new`) opens a dialog that starts one in a folder of the current space, with `claude --bg` or your own command ([06 · New background session](06-new-session.md)).

Bind them in Herdr's `config.toml`. Free after the prefix in Herdr's defaults are `a`, `d`, `f`, `i`, `m`, `t`, `u` and `y` (other plugins may take some):

```toml
[[keys.command]]
key = "prefix+shift+a"
type = "plugin_action"
command = "kamilpociot.claude-agents.open"
description = "Claude background sessions"

[[keys.command]]
key = "prefix+a"
type = "plugin_action"
command = "kamilpociot.claude-agents.new"
description = "new Claude background session"
```

## Settings

`config.toml` in the plugin's config dir (`herdr plugin config-dir kamilpociot.claude-agents`). Unknown keys are an error, logged by the watcher (v0.1's `workspace` is one: remove it).

```toml
claude = "claude"      # Claude Code's command: a name on the Herdr server's PATH, or a full path
                       # (also the new-session dialog's default: `<claude> --bg`)
poll_seconds = 2       # look this often even when no file changes (at least 1)
```

The watcher rereads the file on every look, so changes apply within a poll.

## Files and environment

| What | Where |
|---|---|
| recent commands of the new-session dialog | `$HERDR_PLUGIN_STATE_DIR/commands.json` |
| saved state per Herdr server | `$HERDR_PLUGIN_STATE_DIR/server-<hash>.json` (+ `.lock` while a watcher runs, `.reopen` for the action) |
| Claude's sessions and jobs | `$CLAUDE_CONFIG_DIR` or `~/.claude`: `sessions/`, `jobs/<id>/state.json` |
| test hook | `HERDR_CLAUDE_AGENTS_BIN`: the binary `bin/herdr-claude-agents` runs instead of the release build |

## Code and tests

- `src/plugin.rs`: `Config`, the plugin `Context` from Herdr's environment, state paths, the lock.
- `herdr-plugin.toml`, `bin/herdr-claude-agents`.
- Unit: config defaults, the `poll_seconds` floor, typos and the old `workspace` rejected; one state per server; the lock and waiting for it.

Related: [01 · Background sessions as agents](01-sessions-as-agents.md), [05 · Testing](05-testing.md).
