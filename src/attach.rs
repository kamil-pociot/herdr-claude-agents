//! A session tab: `claude attach <id>`, and what happens when you leave it.
//! The tab has no shell behind it, so leaving the session (`←` to the agent
//! view and out, or `Ctrl-Z`) asks whether to attach again or close the tab.

use std::io::{BufRead, Write};
use std::process::Command;

use anyhow::{Context as _, Result, bail};

use crate::claude;
use crate::plugin::{self, Context};

pub fn run(ctx: &Context) -> Result<()> {
    let Some(id) = std::env::var(plugin::SESSION_ENV).ok().filter(|v| !v.is_empty()) else {
        bail!(
            "{} is not set: this pane is opened by the Claude sessions tab",
            plugin::SESSION_ENV
        );
    };
    let name = std::env::var(plugin::NAME_ENV).unwrap_or_else(|_| id.clone());
    let config = ctx.config().map_err(anyhow::Error::msg)?;
    loop {
        attach(&config.claude, &id)?;
        // Ended (stopped or removed): the tab goes; the watcher forgets it.
        let running = claude::list(&config.claude).map(|all| all.iter().any(|s| s.id == id));
        if !running.unwrap_or(true) {
            return Ok(());
        }
        print!(
            "\r\n\x1b[1m{name}\x1b[0m is still running in the background.\r\n\
             Enter: attach again · q: close this tab \x1b[2m(it stays closed until you run \
             \"Claude: show background sessions\")\x1b[0m\r\n"
        );
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin().lock().read_line(&mut line).unwrap_or(0) == 0 || line.trim() == "q" {
            return Ok(());
        }
    }
}

/// Runs `claude attach <id>` to the end. `Ctrl-Z` would stop it with no shell
/// to bring it back, so a stop ends the attach instead (the session itself
/// keeps running).
fn attach(claude: &str, id: &str) -> Result<()> {
    let saved = Termios::get();
    let child = Command::new(claude)
        .args(["attach", id])
        .spawn()
        .with_context(|| format!("cannot run `{claude} attach {id}`"))?;
    let pid = child.id() as libc::pid_t;
    loop {
        let mut status = 0;
        // SAFETY: waiting on our own child.
        let rc = unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) };
        if rc == -1 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if libc::WIFSTOPPED(status) {
            // SAFETY: signalling our own child.
            unsafe {
                libc::kill(pid, libc::SIGTERM);
                libc::kill(pid, libc::SIGCONT);
            }
            continue;
        }
        break;
    }
    // Whatever state it left the terminal in: cooked mode, main screen, cursor.
    if let Some(t) = saved {
        t.set();
    }
    print!("\x1b[?1049l\x1b[?25h\x1b[0m");
    let _ = std::io::stdout().flush();
    Ok(())
}

struct Termios(libc::termios);

impl Termios {
    fn get() -> Option<Termios> {
        // SAFETY: tcgetattr fills the struct on success.
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            (libc::tcgetattr(0, &mut t) == 0).then_some(Termios(t))
        }
    }

    fn set(&self) {
        // SAFETY: restoring attributes read from the same terminal.
        unsafe {
            libc::tcsetattr(0, libc::TCSANOW, &self.0);
        }
    }
}
