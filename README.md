# herdr-claude-agents

A [Herdr](https://herdr.dev) plugin that shows your **Claude Code background sessions** (`claude --bg`, the ones `claude agents` lists) as Herdr agents.

Background sessions run in Claude's daemon, not in any terminal, so Herdr never sees them. This plugin gives each one a tab running `claude attach <id>` in the space of the project it works in, and tells Herdr what the session is doing. You get:

- every background session in Herdr's **Agents sidebar**, shown like any other Claude, under the **space of its project** (a space is created when the project has none);
- its real state — **working**, **needs input**, **done** — from Claude itself, so Herdr's **notifications**, attention rollups and `herdr agent wait` work as for any agent;
- the full Claude UI in its tab, named after the session: read, answer, approve;
- a **dialog to start a session** in a folder of the current space, with `claude --bg` or your own command;
- a list that **follows changes**: new sessions get a tab, ended or removed ones lose theirs, renames and state changes show up within a second or two.

## Install

Needs Herdr 0.9.2+, Claude Code with background sessions, git, and Rust to build.

```sh
herdr plugin install kamil-pociot/herdr-claude-agents       # clones and builds it
herdr plugin action invoke kamilpociot.claude-agents.open   # or restart Herdr
```

To work on it, link a checkout instead:

```sh
git clone https://github.com/kamil-pociot/herdr-claude-agents && cd herdr-claude-agents
cargo build --release
herdr plugin link "$PWD"
```

From then on it runs whenever the Herdr server does.

## Use

| Do | Result |
|---|---|
| start a session (`claude --bg …`, or from `claude agents`) | a tab named after it in its project's space, and a `claude` agent in the sidebar |
| select it in the Agents sidebar | its tab, attached; type to it as usual |
| `←` in a session tab, then open another session | Claude's agent list; the session you open is shown in its own tab, and this tab stays on its session |
| quit the agent list, or `Ctrl-Z`, in a session tab | "Enter: attach again · q: close this tab" |
| close a session's tab | it stays closed until the session ends or you run the action below |
| `herdr plugin action invoke kamilpociot.claude-agents.open` | reopens closed tabs (and starts the plugin if it isn't running) |
| `herdr plugin action invoke kamilpociot.claude-agents.new` | a dialog: start a background session in a folder of the current space (relative path, Tab completes), with `claude --bg` or your own command (an alias like `po` works), optionally with a first prompt |

Bind the actions to keys in Herdr's `config.toml`:

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

Settings (`$(herdr plugin config-dir kamilpociot.claude-agents)/config.toml`):

```toml
claude = "claude"      # Claude Code's command, if not on Herdr's PATH
poll_seconds = 2       # fallback poll; file changes wake it sooner
```

## Docs

- [`docs/`](docs/README.md) — one document per feature
- [`ROADMAP.md`](ROADMAP.md) — decisions and next steps
- [`AGENTS.md`](AGENTS.md) — working on the code

## License

[MIT](LICENSE)
