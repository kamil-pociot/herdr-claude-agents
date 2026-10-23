//! A session tab: `claude attach <id>`, and what happens when you leave it.
//! The tab shows one session and has no shell behind it, so leaving the
//! session (`←` towards the agent list, or `Ctrl-Z`) asks whether to attach
//! again or close the tab.

use std::io::{BufRead, Write};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};

use crate::claude;
use crate::plugin::{self, Context};

/// How often the attach is checked for having turned into the agent list.
const CHECK: Duration = Duration::from_millis(100);
/// How long it gets to quit before it is killed.
const GRACE: Duration = Duration::from_secs(2);

pub fn run(ctx: &Context) -> Result<()> {
    let Some(id) = std::env::var(plugin::SESSION_ENV).ok().filter(|v| !v.is_empty()) else {
        bail!(
            "{} is not set: this pane is opened by the plugin for a session",
            plugin::SESSION_ENV
        );
    };
    let name = std::env::var(plugin::NAME_ENV).unwrap_or_else(|_| id.clone());
    let config = ctx.config().map_err(anyhow::Error::msg)?;
    loop {
        let left = attach(&config.claude, &id)?;
        // Ended (stopped or removed): the tab goes; the watcher forgets it.
        let running = claude::list(&config.claude).map(|all| all.iter().any(|s| s.id == id));
        if !running.unwrap_or(true) {
            return Ok(());
        }
        if left == Left::AgentList {
            print!(
                "\r\nThe agent list isn't available here: this tab shows \x1b[1m{name}\x1b[0m only \
                 (Herdr's Agents sidebar lists the others).\r\n"
            );
        } else {
            print!("\r\n\x1b[1m{name}\x1b[0m is still running in the background.\r\n");
        }
        print!(
            "Enter: attach again · q: close this tab \x1b[2m(it stays closed until you run \
             \"Claude: show background sessions\")\x1b[0m\r\n"
        );
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin().lock().read_line(&mut line).unwrap_or(0) == 0 || line.trim() == "q" {
            return Ok(());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Left {
    /// It quit (or the session ended).
    Quit,
    /// `Ctrl-Z`.
    Stopped,
    /// `←`: Claude relaunches the attach as `claude agents`, where any
    /// session can be opened — not this tab's, and not what Herdr is told.
    AgentList,
}

/// Runs `claude attach <id>` until it ends, or ends it when it turns into the
/// agent list. `Ctrl-Z` would stop it with no shell to bring it back, so a
/// stop ends the attach too (the session itself keeps running).
fn attach(claude: &str, id: &str) -> Result<Left> {
    let saved = Termios::get();
    let child = Command::new(claude)
        .args(["attach", id])
        .spawn()
        .with_context(|| format!("cannot run `{claude} attach {id}`"))?;
    let pid = child.id() as libc::pid_t;
    let mut left = Left::Quit;
    let mut ending: Option<Instant> = None;
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
            None if args_of(pid).is_some_and(|a| in_agent_list(&a)) => {
                left = Left::AgentList;
                end(pid, libc::SIGTERM);
                ending = Some(Instant::now());
            }
            Some(since) if since.elapsed() > GRACE => end(pid, libc::SIGKILL),
            _ => {}
        }
        std::thread::sleep(CHECK);
    }
    // Whatever state it left the terminal in: cooked mode, main screen, cursor.
    if let Some(t) = saved {
        t.set();
    }
    print!("\x1b[?1049l\x1b[?25h\x1b[0m");
    let _ = std::io::stdout().flush();
    Ok(left)
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
