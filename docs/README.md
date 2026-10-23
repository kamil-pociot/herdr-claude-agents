# herdr-claude-agents documentation

herdr-claude-agents is a Herdr plugin that shows Claude Code background sessions as Herdr agents. Each document below describes one feature: what it is for, how it behaves, the settings that affect it, and where it lives in the code and tests.

| # | Document | In short |
|---|---|---|
| 01 | [Background sessions as agents](01-sessions-as-agents.md) | Detecting sessions, a tab per session in the space of its project (created when missing), the watcher, following starts, ends and renames, closed tabs, Herdr restarts |
| 02 | [State and notifications](02-state-and-notifications.md) | Claude's state reported to Herdr: sidebar marks, notifications, waits; shown like any Claude; `$tokens` for custom sidebar rows |
| 03 | [Session tabs](03-session-tabs.md) | `claude attach <id>` in a tab: leaving it, attaching again, closing it, errors |
| 04 | [Install and configuration](04-configuration.md) | Building, linking, the `open` action and a key for it, `config.toml` |
| 05 | [Testing](05-testing.md) | Unit tests and end-to-end tests against an isolated Herdr server |

See also: [`../README.md`](../README.md) (overview and quick start), [`../ROADMAP.md`](../ROADMAP.md) (decisions, findings, next steps), [`../AGENTS.md`](../AGENTS.md) (working on the code).
