# 05 · Testing

```sh
cargo test                     # unit + end-to-end
cargo test --bins              # unit only
cargo test --test e2e          # end-to-end only
```

Run the whole suite twice before calling a change done.

## Unit tests

Next to the code they test (`#[cfg(test)]` in each module): parsing `claude agents --json`, the state table, the plan for every combination of session and pane, saved state, config, the lock, Herdr replies and agent names, which file changes wake the watcher, the sessions screen.

## End-to-end tests

`tests/e2e.rs` drives a real `herdr server` with the plugin linked through its real manifest, so the startup hook, the sessions tab, session tabs and the `open` action all run as they do for a user. Each test gets its own server:

- **Isolated Herdr.** `XDG_CONFIG_HOME`, `XDG_STATE_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME` point into `/tmp/hca-<pid>-<n>/`, so the plugin registry, socket and state are the test's own; inherited `HERDR_*` variables are removed. The path stays short because Unix socket paths stop at ~104 bytes.
- **Fake Claude.** `config.toml` points `claude` at a script: `agents --json` prints `list.json`, which the test rewrites (`h.sessions(&[…])`); `attach <id>` prints `fake attach <id>` and waits. `CLAUDE_CONFIG_DIR` points at the test's own `claude/` folder, for `jobs/<id>/state.json` and file watching.
- **Debug build.** `HERDR_CLAUDE_AGENTS_BIN` makes the manifest's launcher run the test's binary.
- Tests are skipped when `herdr` is not on `PATH`.

Assertions go through the Herdr CLI (`agent list`, `pane list`, `tab get`, `pane read`). `wait_for` polls for up to 20 s; on a timeout it prints the agents, the panes, the plugin's command log and the sessions tab's screen, which shows the watcher's last error.

Never point the tests at the real Herdr server or the real `~/.claude`: they open and close panes, and a real `claude attach` would attach to live sessions.

Related: [04 · Install and configuration](04-configuration.md).
