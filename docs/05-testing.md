# 05 · Testing

```sh
cargo test                     # unit + end-to-end
cargo test --bins              # unit only
cargo test --test e2e          # end-to-end only
```

Run the whole suite twice before calling a change done.

## Unit tests

Next to the code they test (`#[cfg(test)]` in each module): parsing `claude agents --json`, the state table, the plan for every combination of session and pane, saved state, config, the lock, Herdr replies, choosing a space and a session's git project, which file changes wake the watcher, the tokens.

## End-to-end tests

`tests/e2e.rs` drives a real `herdr server` with the plugin linked through its real manifest, so the startup hook (the watcher), session tabs and the `open` action all run as they do for a user. Each test has two projects: `repos/alpha`, a git repository, and `repos/beta`, a plain folder; `h.open_space("alpha")` opens a space on one as a user would. Each test gets its own server:

- **Isolated Herdr.** `XDG_CONFIG_HOME`, `XDG_STATE_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME` point into `/tmp/hca-<pid>-<n>/`, so the plugin registry, socket and state are the test's own; inherited `HERDR_*` variables are removed. The path stays short because Unix socket paths stop at ~104 bytes.
- **Fake Claude.** `config.toml` points `claude` at a script: `agents --json` prints `list.json`, which the test rewrites (`h.sessions(&[…])`); `attach <id>` prints `fake attach <id>` and waits. `CLAUDE_CONFIG_DIR` points at the test's own `claude/` folder, for `jobs/<id>/state.json` and file watching.
- **Debug build.** `HERDR_CLAUDE_AGENTS_BIN` makes the manifest's launcher run the test's binary.
- Tests are skipped when `herdr` is not on `PATH`.

Assertions go through the Herdr CLI (`agent list`, `pane list`, `tab get`, `pane read`). Agents are found by their `$claude_session` token. `h.watcher_running()` tells whether the watcher holds its lock. `wait_for` polls for up to 25 s; on a timeout it prints the agents, the panes, the plugin's command log and the watcher's saved state, which holds its last error.

## Linux

The suite runs on Linux too (process arguments come from `/proc/<pid>/cmdline` there, file events from inotify). On a Mac, with Docker, run it in a container with Herdr's Linux binary on `PATH` (`herdr-linux-aarch64` or `-x86_64` from Herdr's GitHub releases, in a folder of its own); the checkout is mounted read-only and the build goes to a volume:

```sh
mkdir -p /tmp/herdr-linux && curl -fsSL -o /tmp/herdr-linux/herdr \
  https://github.com/herdrdev/herdr/releases/download/v0.9.5/herdr-linux-aarch64 && chmod +x /tmp/herdr-linux/herdr
docker run --rm -v "$PWD":/src:ro -v /tmp/herdr-linux:/opt/herdr:ro -v hca-target:/target \
  -e CARGO_TARGET_DIR=/target -w /src rust:1 sh -c \
  'export PATH=/opt/herdr:$PATH; rustup component add clippy rustfmt; cargo fmt --all -- --check &&
   cargo clippy --all-targets -- -D warnings && cargo test && cargo test'
```

A test that reads a process's arguments right after starting it retries: under load, Linux can briefly show none, or still the parent's.

Never point the tests at the real Herdr server or the real `~/.claude`: they open and close panes, and a real `claude attach` would attach to live sessions.

Related: [04 · Install and configuration](04-configuration.md).
