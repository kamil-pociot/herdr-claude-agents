# 04 · Install and configuration

## Install

Needs Herdr 0.9.2+ (agent reports and plugin panes), Claude Code with background sessions (`claude agents --json`), and Rust to build.

```sh
cargo build --release
herdr plugin link "$PWD"                                     # this checkout, for every Herdr session
herdr plugin action invoke kamilpociot.claude-agents.open    # start now, or restart the Herdr server
```

Linking registers the checkout; Herdr runs `bin/herdr-claude-agents`, which runs `target/release/herdr-claude-agents`, so rebuild after a change. A running watcher keeps the old binary until its tab is closed and reopened (or Herdr restarts).

Without a checkout, `herdr plugin install kamil-pociot/herdr-claude-agents` clones the repository into a Herdr-managed folder and runs the manifest's build step (`cargo build --release --locked`); `herdr plugin update` fetches and rebuilds it. Installing over a linked checkout is refused: unlink first.

`herdr plugin unlink kamilpociot.claude-agents` removes it; close the `Claude` workspace yourself.

## The action

**Claude: show background sessions** (`kamilpociot.claude-agents.open`) opens the sessions tab if it isn't running, or shows it if it is, and reopens session tabs you closed. Bind it in Herdr's `config.toml`:

```toml
[[keys.command]]
key = "prefix+a"
type = "plugin_action"
command = "kamilpociot.claude-agents.open"
description = "Claude sessions"
```

## Settings

`config.toml` in the plugin's config dir (`herdr plugin config-dir kamilpociot.claude-agents`). Unknown keys are an error, shown on the sessions tab.

```toml
workspace = "Claude"   # the workspace for the sessions tab and session tabs
claude = "claude"      # Claude Code's command: a name on the Herdr server's PATH, or a full path
poll_seconds = 2       # look this often even when no file changes (at least 1)
```

The watcher rereads the file on every look, so changes apply within a poll; `workspace` applies to the next `startup` / `open`.

## Files and environment

| What | Where |
|---|---|
| saved state per Herdr server | `$HERDR_PLUGIN_STATE_DIR/server-<hash>.json` (+ `.lock`, `.reopen`) |
| Claude's sessions and jobs | `$CLAUDE_CONFIG_DIR` or `~/.claude`: `sessions/`, `jobs/<id>/state.json` |
| test hook | `HERDR_CLAUDE_AGENTS_BIN`: the binary `bin/herdr-claude-agents` runs instead of the release build |

## Code and tests

- `src/plugin.rs`: `Config`, the plugin `Context` from Herdr's environment, state paths, the lock.
- `herdr-plugin.toml`, `bin/herdr-claude-agents`.
- Unit: config defaults, the `poll_seconds` floor, typos rejected; one state per server; the lock.

Related: [01 · Background sessions as agents](01-sessions-as-agents.md), [05 · Testing](05-testing.md).
