//! A session tab: `claude attach <id>`, and what happens when you leave it.
//!
//! The tab shows one session. `←` still opens Claude's agent list, but
//! opening another session there jumps to that session's own tab and puts
//! this tab back on its session, so every tab keeps showing what Herdr is
//! told about it. The tab has no shell behind it, so quitting (or `Ctrl-Z`)
//! asks whether to attach again or close the tab.

use std::io::{BufRead, Write};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};

use crate::claude;
use crate::herdr::Herdr;
use crate::plugin::{self, Context};
use crate::sync::Saved;

/// How often the attach is checked for having turned into the agent list.
const CHECK: Duration = Duration::from_millis(100);
/// How often, in the agent list, the tab looks at which session it shows.
const LOOK: Duration = Duration::from_millis(300);
/// How long it gets to quit before it is killed.
const GRACE: Duration = Duration::from_secs(2);
/// How long to wait for the watcher to open a session that has no tab.
const NEW_TAB: Duration = Duration::from_secs(6);
/// The terminal title of Claude's agent list.
const LIST_TITLE: &str = "claude agents";

pub fn run(ctx: &Context) -> Result<()> {
    let Some(id) = std::env::var(plugin::SESSION_ENV).ok().filter(|v| !v.is_empty()) else {
        bail!(
            "{} is not set: this pane is opened by the plugin for a session",
            plugin::SESSION_ENV
        );
    };
    let name = std::env::var(plugin::NAME_ENV).unwrap_or_else(|_| id.clone());
    let config = ctx.config().map_err(anyhow::Error::msg)?;
    let tab = Tab {
        ctx,
        herdr: Herdr::from_env(),
        pane: std::env::var("HERDR_PANE_ID").ok().filter(|p| !p.is_empty()),
        claude: config.claude,
        id,
    };
    loop {
        match tab.attach()? {
            Left::Opened(other) => {
                // Back on this tab's session at once; the other has its own tab.
                tab.show(&other);
                continue;
            }
            Left::Quit | Left::Stopped => {}
        }
        // Ended (stopped or removed): the tab goes; the watcher forgets it.
        let running = claude::list(&tab.claude).map(|all| all.iter().any(|s| s.id == tab.id));
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum Left {
    /// It quit (or the session ended).
    Quit,
    /// `Ctrl-Z`.
    Stopped,
    /// Another session was opened from the agent list (its id).
    Opened(String),
}

struct Tab<'a> {
    ctx: &'a Context,
    herdr: Herdr,
    /// This tab's pane, for its title and screen.
    pane: Option<String>,
    claude: String,
    /// The session this tab shows.
    id: String,
}

impl Tab<'_> {
    /// Runs `claude attach <id>` until it ends. `←` relaunches it in place as
    /// `claude agents`; once that opens another session, it is ended. `Ctrl-Z`
    /// would stop it with no shell to bring it back, so a stop ends it too
    /// (the session itself keeps running).
    fn attach(&self) -> Result<Left> {
        let saved = Termios::get();
        let child = Command::new(&self.claude)
            .args(["attach", &self.id])
            .spawn()
            .with_context(|| format!("cannot run `{} attach {}`", self.claude, self.id))?;
        let pid = child.id() as libc::pid_t;
        let mut left = Left::Quit;
        let mut ending: Option<Instant> = None;
        let mut looked = Instant::now() - LOOK;
        // The last name seen in the agent list, so each is looked up once.
        let mut seen: Option<String> = None;
        loop {
            let mut status = 0;
            // SAFETY: waiting on our own child.
            let rc = unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED | libc::WNOHANG) };
            if rc == pid {
                if !libc::WIFSTOPPED(status) {
                    break;
                }
                left = Left::Stopped;
                end(pid, libc::SIGTERM);
                // SAFETY: signalling our own child.
                unsafe { libc::kill(pid, libc::SIGCONT) };
                ending.get_or_insert_with(Instant::now);
            } else if rc == -1 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                break;
            }
            match ending {
                None if looked.elapsed() >= LOOK && args_of(pid).is_some_and(|a| in_agent_list(&a)) => {
                    looked = Instant::now();
                    if let Some(other) = self.opened_elsewhere(&mut seen) {
                        left = Left::Opened(other);
                        end(pid, libc::SIGTERM);
                        ending = Some(Instant::now());
                    }
                }
                Some(since) if since.elapsed() > GRACE => end(pid, libc::SIGKILL),
                _ => {}
            }
            std::thread::sleep(CHECK);
        }
        // Whatever state it left the terminal in: cooked mode, main screen,
        // visible cursor. (1047, not 1049: that also restores a saved cursor,
        // which sends output back to the top when no alternate screen was on.)
        if let Some(t) = saved {
            t.set();
        }
        print!("\x1b[?1047l\x1b[?25h\x1b[0m");
        let _ = std::io::stdout().flush();
        Ok(left)
    }

    /// The id of a session other than this tab's that the agent list has
    /// opened in this tab, if any.
    fn opened_elsewhere(&self, seen: &mut Option<String>) -> Option<String> {
        let pane = self.pane.as_deref()?;
        let title = self.herdr.pane_title(pane).ok().flatten();
        let screen = match title.as_deref() {
            Some(t) if t != LIST_TITLE => None,
            _ => self.herdr.pane_screen(pane).ok(),
        };
        let name = shown(title.as_deref(), screen.as_deref())?;
        if seen.as_deref() == Some(name.as_str()) {
            return None;
        }
        *seen = Some(name.clone());
        let sessions = claude::list(&self.claude).ok()?;
        let s = sessions.iter().find(|s| s.name == name || s.id == name)?;
        (s.id != self.id).then(|| s.id.clone())
    }

    /// Shows session `other`'s own tab, asking the watcher to open it first
    /// when it has none (new, or its tab was closed).
    fn show(&self, other: &str) {
        let pane_of = || {
            Saved::load(&self.ctx.state_path())
                .sessions
                .get(other)
                .map(|t| t.pane.pane_id.clone())
        };
        let mut pane = pane_of();
        if pane.is_none() {
            let _ = std::fs::write(self.ctx.reopen_path(), "");
            let deadline = Instant::now() + NEW_TAB;
            while pane.is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(200));
                pane = pane_of();
            }
        }
        if let Some(pane) = pane {
            let _ = self.herdr.focus_agent(&pane);
        }
    }
}

/// The session Claude shows in a pane, from its terminal title (Claude sets
/// it to the open session's name, or "claude agents" in the list) or, with
/// no telling title, from the screen: the open session's name sits in the
/// rule under its prompt (`──── fix auth ─`). None in the list.
fn shown(title: Option<&str>, screen: Option<&str>) -> Option<String> {
    if let Some(t) = title.map(str::trim).filter(|t| !t.is_empty() && *t != LIST_TITLE) {
        return Some(t.to_string());
    }
    screen?.lines().rev().find_map(|line| {
        let line = line.trim();
        let inner = line.strip_suffix('─')?.trim_end();
        let name = inner.trim_start_matches('─');
        (line.starts_with('─') && name.len() < inner.len() && !name.trim().is_empty() && !name.contains('─'))
            .then(|| name.trim().to_string())
    })
}

fn end(pid: libc::pid_t, signal: libc::c_int) {
    // SAFETY: signalling our own child.
    unsafe { libc::kill(pid, signal) };
}

/// Whether a process started as `… attach <id>` now runs something else:
/// `←` relaunches it in place as `claude agents`. (A `claude` that is a
/// script runs as `sh <script> attach <id>`, so `attach` is looked for
/// anywhere.)
fn in_agent_list(args: &[String]) -> bool {
    !args.iter().skip(1).any(|a| a == "attach")
}

/// A running process's arguments.
#[cfg(target_os = "macos")]
fn args_of(pid: libc::pid_t) -> Option<Vec<String>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size: libc::size_t = 0;
    // SAFETY: the first call asks for the size, the second fills a buffer of it.
    unsafe {
        let null = std::ptr::null_mut();
        if libc::sysctl(mib.as_mut_ptr(), 3, null, &mut size, null, 0) != 0 {
            return None;
        }
        let mut buf = vec![0u8; size];
        if libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr().cast(), &mut size, null, 0) != 0 {
            return None;
        }
        buf.truncate(size);
        procargs2(&buf)
    }
}

#[cfg(not(target_os = "macos"))]
fn args_of(pid: libc::pid_t) -> Option<Vec<String>> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let raw = raw.strip_suffix(&[0]).unwrap_or(&raw);
    (!raw.is_empty()).then(|| {
        raw.split(|&b| b == 0)
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .collect()
    })
}

/// macOS `KERN_PROCARGS2`: `argc`, the executable's path, NUL padding, then
/// `argc` NUL-terminated arguments (and the environment after them).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn procargs2(buf: &[u8]) -> Option<Vec<String>> {
    let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?) as usize;
    let rest = &buf[4..];
    let rest = &rest[rest.iter().position(|&b| b == 0)?..];
    let rest = &rest[rest.iter().position(|&b| b != 0)?..];
    Some(
        rest.split(|&b| b == 0)
            .take(argc)
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .collect(),
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_agent_list_is_an_attach_that_no_longer_attaches() {
        assert!(!in_agent_list(&args(&["claude", "attach", "abc"])));
        assert!(!in_agent_list(&args(&["/bin/sh", "/tmp/claude", "attach", "abc"])));
        assert!(in_agent_list(&args(&["/Users/me/.local/bin/claude", "agents"])));
        assert!(in_agent_list(&args(&["/bin/sh", "/tmp/claude", "agents"])));
        // The program itself being called "attach" doesn't count.
        assert!(in_agent_list(&args(&["attach"])));
    }

    #[test]
    fn the_open_session_comes_from_the_title_or_the_screen() {
        assert_eq!(shown(Some("fix auth"), None).as_deref(), Some("fix auth"));
        // The list itself: nothing open, whatever the screen says.
        let list = "Working\n✳ fix auth   Reading\n────────────\n❯ describe a task\n────────────\n";
        assert_eq!(shown(Some("claude agents"), Some(list)), None);
        // No title: the name in the rule under the prompt.
        let session = "● done\n──────────────── cli-file-browser-tree (2) ─\n❯ \n────────────────\n  ⏵⏵ auto mode";
        assert_eq!(shown(None, Some(session)).as_deref(), Some("cli-file-browser-tree (2)"));
        assert_eq!(
            shown(Some("claude agents"), Some(session)).as_deref(),
            Some("cli-file-browser-tree (2)")
        );
        assert_eq!(shown(None, Some(list)), None);
        assert_eq!(shown(None, None), None);
        assert_eq!(shown(Some("  "), None), None);
    }

    #[test]
    fn a_running_process_reports_its_arguments() {
        let mut child = Command::new("sleep").arg("5").spawn().unwrap();
        let a = args_of(child.id() as libc::pid_t).unwrap();
        assert_eq!(a.last().map(String::as_str), Some("5"));
        assert!(a[0].ends_with("sleep"));
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn procargs2_skips_the_path_and_its_padding() {
        let mut buf = 2i32.to_ne_bytes().to_vec();
        buf.extend_from_slice(b"/bin/claude\0\0\0\0claude\0agents\0HOME=/x\0");
        assert_eq!(procargs2(&buf).unwrap(), args(&["claude", "agents"]));
        assert_eq!(procargs2(&[1, 0]), None);
    }
}
