# AGENTS.md

Guidance for coding agents (and humans) working on **herdr-claude-agents**, a [Herdr](https://herdr.dev) plugin that makes Claude Code background sessions (`claude --bg`, `claude agents`) first-class Herdr agents: one tab per session running `claude attach <id>`, its state in Herdr's Agents sidebar, Herdr's notifications and waits, and a list that follows sessions as they start, change and end.

- User-facing overview and setup: [`README.md`](README.md)
- Feature-by-feature documentation (the intent of each part): [`docs/`](docs/README.md)
- Decisions, findings about Herdr and Claude Code, next steps: [`ROADMAP.md`](ROADMAP.md)

## Build, run, test

Rust (edition 2024), one binary crate. On this machine cargo comes from Homebrew's rustup, which is keg-only:

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"   # if `cargo` isn't found

cargo build --release                               # target/release/herdr-claude-agents
herdr plugin link "$PWD"                            # register this checkout with Herdr
herdr plugin action invoke kamilpociot.claude-agents.open   # start it in a running Herdr

cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test                                          # unit + end-to-end (needs `herdr` on PATH)
# on Linux too, in Docker: see docs/05-testing.md#linux
```

A change is done when `fmt --check`, `clippy -D warnings` and the **whole** suite pass (on Linux too when it touches process or file handling) — run the suite twice; the end-to-end tests drive real Herdr servers and timing bugs show up as rare failures.

Requirements at run time: Herdr 0.9.2+ and Claude Code with background sessions (`claude agents --json`).

## Where to work

Work in the main checkout, on `main` or on a normal branch (`git switch -c <name>`), **not in git worktrees** (no `.claude/worktrees/…`, no `git worktree add`). Herdr runs the plugin from this checkout (`herdr plugin link`), so changes made elsewhere don't show up.

- Small changes can go straight to `main`; larger ones on a branch, merged with `git merge --ff-only` once the checks pass.
- Leave the owner's IDE files (`.idea/`) alone.
- The remote is `origin` over **SSH** (`git@github.com:kamil-pociot/herdr-claude-agents.git`; HTTPS has no credentials on this machine, and the repo config rewrites `https://github.com/` to SSH); push only when asked.
- Never touch the owner's real Herdr server while testing (no `herdr server stop`, no closing their panes). Use an isolated server: `XDG_CONFIG_HOME=/tmp/x/config XDG_STATE_HOME=/tmp/x/state herdr server` — that is what `tests/e2e.rs` does.
- Never attach to, prompt or stop the owner's real Claude sessions from tests. The e2e tests use a fake `claude`.

## How it fits together

Herdr plugins are a manifest (`herdr-plugin.toml`) plus commands; the Herdr CLI (`$HERDR_BIN_PATH`) is the whole plugin API. One binary, three subcommands, all started through `bin/herdr-claude-agents` (which picks the release build, or `HERDR_CLAUDE_AGENTS_BIN`):

| Subcommand | Started by | Does |
|---|---|---|
| `startup` | `[[startup]]` hook, once per server start | the watcher: headless, runs until the server stops or the plugin is disabled; keeps one tab per session in its project's space |
| `open` | the `open` action | asks the watcher to reopen tabs the user closed; becomes the watcher if none runs |
| `attach` | `[[panes]] session` | one session's tab: `claude attach <id>`, re-attach prompt when left |

| Module | Role |
|---|---|
| `claude.rs` | `claude agents --json` (background sessions only), state mapping, `jobs/<id>/state.json` detail |
| `herdr.rs` | the Herdr CLI: panes, workspaces, plugin panes, `report-agent`, `report-metadata` tokens, "server gone", plugin enabled |
| `spaces.rs` | which space a session belongs in: its git project, matched by the folders of the spaces' panes |
| `sync.rs` | pure planning: saved state + sessions + panes → `Op`s; the saved state itself |
| `watch.rs` | the loop: file watching + polling, carrying out `Op`s, creating spaces, reporting, stopping |
| `attach.rs` | the session tab wrapper |
| `plugin.rs` | plugin env, `config.toml`, per-server state paths, the watcher lock |

## Conventions

- **Talk to Herdr through its CLI** (`HERDR_BIN_PATH`) and parse its JSON (`{"result": …}` / `{"error": …}`; reports print nothing on success). Never scrape human output.
- **Plan, then act.** Decisions about tabs live in `sync::plan` (pure, unit-tested); `watch.rs` only carries them out.
- **Herdr sees our state, not the screen.** Sessions are reported with source `claude-agents`, which makes Herdr skip screen detection for those panes. Nothing else is displayed (no name, title, labels; the pane label is cleared) so the agent looks like any Claude; extras are tokens (source `claude-agents.tokens`).
- **The watcher stops itself.** Herdr doesn't stop plugin commands with the server, so the watcher exits on `server_not_running` and when the plugin is no longer enabled.
- **Panes are recognised by pane id + terminal id.** After a Herdr restart, panes come back as shells with the same pane id and a new terminal id.
- **Manifest commands must not depend on the working directory** (session tabs open in the session's folder): use `$HERDR_PLUGIN_ROOT`.
- Comments explain *why*; match the surrounding density. `rustfmt.toml` sets `max_width = 120`.

## Tests

Every feature gets unit tests where the logic lives **and** an end-to-end test in `tests/e2e.rs`, which starts a real, isolated `herdr server` with this plugin linked and a fake `claude` whose session list the test rewrites:

```rust
let Some(mut h) = Herdr::new() else { return };          // skipped without herdr
h.sessions(&[session("aaaa", "fix auth", "working", "busy")]);
h.start();
h.wait_for("the session", || h.status_of("fix auth").as_deref() == Some("working"));
```

- Agents are found by their `$claude_session` token (`h.agent("aaaa")`); they carry no other name.
- `wait_for` polls for 25 s; a timeout prints the agents, panes, plugin log and the watcher's saved state (its last error).
- Keep the server's socket path short (`/tmp/hca-…`): Unix sockets stop at ~104 bytes.
- To check a test really guards a fix, temporarily undo the fix and watch it fail.

## Docs

Every feature has its own document in `docs/`, numbered in order (`01-sessions-as-agents.md` …):

- **A new feature gets a new file** with the next free number, never a section squeezed into an existing one. Follow the shape of the others: what it does, how to use it, settings, code and tests. Add a row to the index in `docs/README.md` and link related docs both ways.
- **A change to an existing feature** updates that feature's file (and `04-configuration.md` if settings changed).
- Numbers are never reused or reshuffled, so links stay stable.

Also keep `README.md` and the decisions in `ROADMAP.md` up to date where relevant.
