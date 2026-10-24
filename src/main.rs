//! herdr-claude-agents: a Herdr plugin that shows Claude Code background
//! sessions as Herdr agents. See docs/README.md.

mod attach;
mod claude;
mod herdr;
mod new;
mod plugin;
mod spaces;
mod sync;
mod watch;

use std::process::ExitCode;
use std::time::Duration;

use crate::plugin::Context;

const USAGE: &str = "usage: herdr-claude-agents <startup|open|new|attach|dialog>

  startup  watch the sessions for this Herdr server (Herdr's startup hook)
  open     reopen session tabs you closed; watch if nothing does yet
  new      open the new-session dialog over the current space
  attach   one session's tab (reads CLAUDE_AGENTS_SESSION)
  dialog   the new-session dialog itself";

/// How long a new watcher waits for one from before a Herdr restart, which
/// notices the old server is gone within a poll.
const TAKE_OVER: Duration = Duration::from_secs(15);

fn main() -> ExitCode {
    let ctx = Context::from_env();
    let result = match std::env::args().nth(1).as_deref() {
        Some("startup") => watch::run(&ctx, TAKE_OVER),
        Some("open") => {
            // A running watcher picks this up and reopens dismissed tabs; a
            // new one starts with it.
            let _ = std::fs::create_dir_all(&ctx.state_dir);
            let _ = std::fs::write(ctx.reopen_path(), "");
            watch::run(&ctx, Duration::ZERO)
        }
        Some("new") => new::open(&ctx),
        Some("dialog") => new::run(&ctx),
        Some("attach") => attach::run(&ctx).inspect_err(|e| {
            // A tab that vanished at once would pass for one the user closed;
            // this one stays until the error has been read.
            eprintln!("herdr-claude-agents: {e:#}\r\nPress Enter to close this tab.\r");
            let _ = std::io::stdin().read_line(&mut String::new());
        }),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("herdr-claude-agents: {e:#}");
            ExitCode::FAILURE
        }
    }
}
