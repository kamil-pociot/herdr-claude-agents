//! herdr-claude-agents: a Herdr plugin that shows Claude Code background
//! sessions as Herdr agents. See docs/README.md.

mod attach;
mod claude;
mod herdr;
mod plugin;
mod sync;
mod watch;

use std::process::ExitCode;

use anyhow::Result;

use crate::herdr::Herdr;
use crate::plugin::{Context, Lock};
use crate::sync::Saved;

const USAGE: &str = "usage: herdr-claude-agents <startup|open|watch|attach>

  startup  open the Claude sessions tab (Herdr's startup hook)
  open     open or show it, and reopen session tabs you closed
  watch    the Claude sessions tab itself
  attach   one session's tab (reads CLAUDE_AGENTS_SESSION)";

fn main() -> ExitCode {
    let ctx = Context::from_env();
    let result = match std::env::args().nth(1).as_deref() {
        Some("startup") => ensure(&ctx, false),
        Some("open") => {
            // The watcher picks this up and reopens dismissed tabs.
            let _ = std::fs::create_dir_all(&ctx.state_dir);
            let _ = std::fs::write(ctx.reopen_path(), "");
            ensure(&ctx, true)
        }
        Some("watch") => watch::run(&ctx),
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

/// Opens the Claude sessions tab unless a watcher already runs for this
/// server, in its workspace (created when missing). With `focus`, shows it.
fn ensure(ctx: &Context, focus: bool) -> Result<()> {
    let herdr = Herdr::from_env();
    if Lock::is_held(&ctx.lock_path()) {
        if focus && let Some(w) = Saved::load(&ctx.state_path()).watcher {
            herdr.focus_tab(&w.tab_id)?;
        }
        return Ok(());
    }
    let config = ctx.config().map_err(anyhow::Error::msg)?;
    let existing = herdr.workspaces()?.into_iter().find(|w| w.label == config.workspace);
    let (workspace, root) = match existing {
        Some(w) => (w.workspace_id, None),
        None => {
            let (id, root) = herdr.create_workspace(&config.workspace)?;
            (id, Some(root))
        }
    };
    herdr.open_tab(&ctx.plugin_id, plugin::WATCHER, &workspace, None, &[], focus)?;
    // A new workspace comes with a shell tab; the sessions tab replaces it.
    if let Some(root) = root {
        herdr.close_pane(&root)?;
    }
    Ok(())
}
