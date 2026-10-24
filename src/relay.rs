//! Running `claude attach` in a terminal of its own and relaying it to the
//! tab's, so the tab can add what Claude leaves out.
//!
//! What it adds: the kitty keyboard protocol. A normal Claude session asks
//! for it, so Shift-Enter arrives distinct from Enter (a new line rather than
//! "send"); `claude attach` doesn't, and Herdr encodes keys only the way the
//! program in the pane asked for. Herdr keeps a mode per screen (main and
//! alternate) and Claude draws on the alternate one, so the mode has to be
//! set after Claude switched to it: right after that switch in its output,
//! where nothing else can be cut in half. Leaving the alternate screen
//! brings the main screen's plain keys back by itself.

use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

/// Kitty keyboard flags for the current screen: 1 (tell Shift-Enter, Esc …
/// apart) + 4 (report alternate keys), what Claude sessions use.
pub const KITTY: &[u8] = b"\x1b[=5;1u";
/// What switches to the alternate screen.
const ENTER_ALT: [&[u8]; 3] = [b"\x1b[?1049h", b"\x1b[?1047h", b"\x1b[?47h"];
/// The longest of them, minus one: what may be split across two reads.
const KEEP: usize = 7;
/// How long the relay threads wait before checking whether to stop.
const POLL_MS: libc::c_int = 100;

pub struct Relay {
    master: Arc<OwnedFd>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    size: Option<libc::winsize>,
}

/// Starts `command` on a new pseudo-terminal sized like the tab's and relays
/// keys to it and its output back. Putting the tab's terminal in [`raw_mode`]
/// (and restoring it) is the caller's.
pub fn spawn(command: &mut Command) -> std::io::Result<(Child, Relay)> {
    let (master, slave) = open_pty()?;
    let size = window_size(1);
    if let Some(ws) = size {
        set_window_size(master.as_raw_fd(), &ws);
    }
    let fd = slave.as_raw_fd();
    // SAFETY: duplicating a descriptor we own, each handed to the child once.
    let (i, o, e) = unsafe { (libc::dup(fd), libc::dup(fd), libc::dup(fd)) };
    // SAFETY: the dups above are fresh and owned by these Stdios alone.
    unsafe {
        command
            .stdin(Stdio::from_raw_fd(i))
            .stdout(Stdio::from_raw_fd(o))
            .stderr(Stdio::from_raw_fd(e))
            // A session of its own, with the new terminal as its terminal:
            // its Ctrl-Z, job control and size are its own.
            .pre_exec(|| {
                if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
    }
    let child = command.spawn()?;
    drop(slave);
    let master = Arc::new(master);
    let stop = Arc::new(AtomicBool::new(false));
    let threads = vec![
        keys_in(master.clone(), stop.clone()),
        output_out(master.clone(), stop.clone()),
    ];
    Ok((
        child,
        Relay {
            master,
            stop,
            threads,
            size,
        },
    ))
}

impl Relay {
    /// Passes a new tab size on (the child then gets its SIGWINCH).
    pub fn resize(&mut self) {
        let now = window_size(1);
        if let Some(ws) = now
            && self
                .size
                .is_none_or(|old| old.ws_row != ws.ws_row || old.ws_col != ws.ws_col)
        {
            set_window_size(self.master.as_raw_fd(), &ws);
            self.size = now;
        }
    }

    /// After the child has gone: the rest of its output, then the threads.
    pub fn finish(self) {
        self.stop.store(true, Ordering::SeqCst);
        for t in self.threads {
            let _ = t.join();
        }
    }
}

/// The tab's keys to the child.
fn keys_in(master: Arc<OwnedFd>, stop: Arc<AtomicBool>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while !stop.load(Ordering::SeqCst) {
            if !readable(0) {
                continue;
            }
            match read(0, &mut buf) {
                Some(n) if n > 0 => {
                    if write_all(master.as_raw_fd(), &buf[..n]).is_err() {
                        return;
                    }
                }
                _ => return,
            }
        }
    })
}

/// The child's output to the tab, with the keyboard mode added.
fn output_out(master: Arc<OwnedFd>, stop: Arc<AtomicBool>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 16384];
        let mut tail = Vec::new();
        loop {
            if !readable(master.as_raw_fd()) {
                // Nothing left once the child has gone.
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                continue;
            }
            match read(master.as_raw_fd(), &mut buf) {
                Some(n) if n > 0 => {
                    let out = inject(&mut tail, &buf[..n]);
                    let mut stdout = std::io::stdout().lock();
                    if stdout.write_all(&out).and_then(|()| stdout.flush()).is_err() {
                        return;
                    }
                }
                // EOF, or EIO once every copy of the child's side is closed.
                _ => return,
            }
        }
    })
}

/// `chunk` with [`KITTY`] added right after every switch to the alternate
/// screen, also one split across reads (`tail` carries the end of the last).
pub fn inject(tail: &mut Vec<u8>, chunk: &[u8]) -> Vec<u8> {
    let data = [tail.as_slice(), chunk].concat();
    let mut ends: Vec<usize> = Vec::new();
    for seq in ENTER_ALT {
        let mut from = 0;
        while let Some(at) = find(&data[from..], seq) {
            let end = from + at + seq.len();
            // Ending in the tail: added last time already.
            if end > tail.len() {
                ends.push(end - tail.len());
            }
            from += at + 1;
        }
    }
    ends.sort_unstable();
    let mut out = Vec::with_capacity(chunk.len() + ends.len() * KITTY.len());
    let mut last = 0;
    for end in ends {
        out.extend_from_slice(&chunk[last..end]);
        out.extend_from_slice(KITTY);
        last = end;
    }
    out.extend_from_slice(&chunk[last..]);
    *tail = data[data.len().saturating_sub(KEEP)..].to_vec();
    out
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn open_pty() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let (mut master, mut slave): (RawFd, RawFd) = (-1, -1);
    // SAFETY: openpty fills both descriptors on success; the rest may be null.
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: both are fresh descriptors we now own.
    unsafe { Ok((OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave))) }
}

fn window_size(fd: RawFd) -> Option<libc::winsize> {
    // SAFETY: TIOCGWINSZ fills the struct on success.
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        (libc::ioctl(fd, libc::TIOCGWINSZ as _, &mut ws) == 0 && ws.ws_col > 0).then_some(ws)
    }
}

fn set_window_size(fd: RawFd, ws: &libc::winsize) {
    // SAFETY: TIOCSWINSZ reads the struct.
    unsafe { libc::ioctl(fd, libc::TIOCSWINSZ as _, ws) };
}

/// Keys go through as typed: the child's terminal does the line editing.
pub fn raw_mode() {
    // SAFETY: reading and setting attributes of our own terminal.
    unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(0, &mut t) == 0 {
            libc::cfmakeraw(&mut t);
            libc::tcsetattr(0, libc::TCSANOW, &t);
        }
    }
}

/// Whether `fd` has something to read within [`POLL_MS`].
fn readable(fd: RawFd) -> bool {
    let mut p = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: polling one descriptor we hold.
    let n = unsafe { libc::poll(&mut p, 1, POLL_MS) };
    n > 0 && p.revents != 0
}

fn read(fd: RawFd, buf: &mut [u8]) -> Option<usize> {
    loop {
        // SAFETY: reading into a buffer we own.
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n >= 0 {
            return Some(n as usize);
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return None;
        }
    }
}

fn write_all(fd: RawFd, mut buf: &[u8]) -> std::io::Result<()> {
    while !buf.is_empty() {
        // SAFETY: writing from a buffer we own.
        let n = unsafe { libc::write(fd, buf.as_ptr().cast(), buf.len()) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        buf = &buf[n as usize..];
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&[u8]]) -> Vec<u8> {
        let mut tail = Vec::new();
        chunks.iter().flat_map(|c| inject(&mut tail, c)).collect()
    }

    #[test]
    fn the_keyboard_mode_follows_every_switch_to_the_alternate_screen() {
        assert_eq!(run(&[b"plain output"]), b"plain output");
        assert_eq!(run(&[b"\x1b[?1049h\x1b[H"]), b"\x1b[?1049h\x1b[=5;1u\x1b[H");
        assert_eq!(
            run(&[b"a\x1b[?47hb\x1b[?1047hc"]),
            b"a\x1b[?47h\x1b[=5;1ub\x1b[?1047h\x1b[=5;1uc"
        );
        // Leaving and coming back: again.
        assert_eq!(
            run(&[b"\x1b[?1049hx\x1b[?1049ly\x1b[?1049hz"]),
            b"\x1b[?1049h\x1b[=5;1ux\x1b[?1049ly\x1b[?1049h\x1b[=5;1uz"
        );
    }

    #[test]
    fn a_switch_split_across_reads_gets_its_mode_once() {
        assert_eq!(run(&[b"ab\x1b[?10", b"49hcd"]), b"ab\x1b[?1049h\x1b[=5;1ucd");
        assert_eq!(run(&[b"\x1b", b"[?1049", b"h"]), b"\x1b[?1049h\x1b[=5;1u");
        // Right at the end of a read: added there, not again with the next.
        assert_eq!(run(&[b"\x1b[?1049h", b"next"]), b"\x1b[?1049h\x1b[=5;1unext");
        assert_eq!(run(&[b"", b"\x1b[?1049h"]), b"\x1b[?1049h\x1b[=5;1u");
    }

    #[test]
    fn a_command_runs_on_a_terminal_of_its_own() {
        // `tty` says it has one; its output comes back through the relay.
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "test -t 0 && test -t 1 && printf ok"]);
        let (mut child, relay) = spawn(&mut cmd).unwrap();
        let status = child.wait().unwrap();
        relay.finish();
        assert!(status.success());
    }
}
