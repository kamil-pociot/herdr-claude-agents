//! The watcher: the "Claude sessions" tab. It looks at Claude's background
//! sessions and Herdr's panes whenever Claude's files change (and every couple
//! of seconds anyway), keeps one tab per session, reports each session's state
//! to Herdr, and lists the sessions on its own screen.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use notify::{RecursiveMode, Watcher};

use crate::claude::{self, Session, State};
use crate::herdr::{self, Display, Herdr, Pane};
use crate::plugin::{self, Config, Context, Lock};
use crate::sync::{self, Op, PaneRef, Saved, Shown, Tracked};

/// File events come in bursts (a turn rewrites several files): wait this long
/// for the burst to end before looking.
const SETTLE: Duration = Duration::from_millis(150);

pub fn run(ctx: &Context) -> Result<()> {
    let Some(_lock) = Lock::try_acquire(&ctx.lock_path())? else {
        // "Claude: show background sessions" never opens a second one, but a
        // stray one closes itself.
        println!("The Claude sessions tab is already open in this Herdr.");
        std::thread::sleep(Duration::from_secs(3));
        return Ok(());
    };
    let herdr = Herdr::from_env();
    let mut screen = Screen::default();
    let mut watcher = Watch::new(ctx);
    let mut saved = Saved::load(&ctx.state_path());
    let mut seq = Seq::default();
    adopt_watcher_pane(&herdr, ctx, &mut saved);
    loop {
        let config = ctx.config();
        let result = match &config {
            Ok(config) => look(&herdr, ctx, config, &mut saved, &mut seq),
            Err(e) => Err(anyhow::anyhow!("{e}")),
        };
        if let Err(e) = saved.save(&ctx.state_path()) {
            eprintln!("cannot save state: {e}");
        }
        screen.draw(&render(&result, &saved, &config.clone().unwrap_or_default()));
        let poll = config.map(|c| c.poll()).unwrap_or(Duration::from_secs(2));
        watcher.wait(poll);
    }
}

/// Makes this pane the watcher, closing the previous one when it is a shell
/// Herdr restored after a restart.
fn adopt_watcher_pane(herdr: &Herdr, ctx: &Context, saved: &mut Saved) {
    let panes = herdr.panes().unwrap_or_default();
    let me = ctx
        .pane_id
        .as_deref()
        .and_then(|id| panes.iter().find(|p| p.pane_id == id));
    if let Some(old) = &saved.watcher
        && Some(old.pane_id.as_str()) != ctx.pane_id.as_deref()
        && panes
            .iter()
            .any(|p| p.pane_id == old.pane_id && p.label.as_deref() == Some(plugin::WATCHER_TITLE))
    {
        let _ = herdr.close_pane(&old.pane_id);
    }
    saved.watcher = me.map(PaneRef::of);
}

/// One look at both sides and the changes it calls for.
fn look(herdr: &Herdr, ctx: &Context, config: &Config, saved: &mut Saved, seq: &mut Seq) -> Result<Vec<Session>> {
    if std::fs::remove_file(ctx.reopen_path()).is_ok() {
        saved.dismissed.clear();
    }
    let sessions = claude::list(&config.claude)?;
    let panes = herdr.panes()?;
    let workspace = workspace(ctx, saved, &panes);
    let session = |id: &str| sessions.iter().find(|s| s.id == id);
    let mut errors = Vec::new();
    for op in sync::plan(saved, &sessions, &panes) {
        let done = match op {
            Op::Close { id, pane_id } => {
                saved.sessions.remove(&id);
                herdr.close_pane(&pane_id)
            }
            Op::Dismiss(id) => {
                saved.sessions.remove(&id);
                saved.dismissed.insert(id);
                Ok(())
            }
            Op::Forget(id) => {
                saved.sessions.remove(&id);
                saved.dismissed.remove(&id);
                Ok(())
            }
            Op::Replace { id, pane_id } => {
                saved.sessions.remove(&id);
                let _ = herdr.close_pane(&pane_id);
                open(herdr, ctx, workspace.as_deref(), saved, seq, session(&id).unwrap())
            }
            Op::Open(id) => open(herdr, ctx, workspace.as_deref(), saved, seq, session(&id).unwrap()),
            Op::Report(id) => report(herdr, saved, seq, session(&id).unwrap()),
        };
        if let Err(e) = done {
            errors.push(e.to_string());
        }
    }
    if let Some(e) = errors.first() {
        bail!("{e}");
    }
    Ok(sessions)
}

/// Where session tabs go: the watcher's own workspace.
fn workspace(ctx: &Context, saved: &Saved, panes: &[Pane]) -> Option<String> {
    let me = saved
        .watcher
        .as_ref()
        .map(|w| w.pane_id.as_str())
        .or(ctx.pane_id.as_deref());
    panes
        .iter()
        .find(|p| Some(p.pane_id.as_str()) == me)
        .map(|p| p.workspace_id.clone())
        .or_else(|| ctx.workspace_id.clone())
}

fn open(
    herdr: &Herdr,
    ctx: &Context,
    workspace: Option<&str>,
    saved: &mut Saved,
    seq: &mut Seq,
    s: &Session,
) -> Result<()> {
    let Some(workspace) = workspace else {
        bail!("no workspace for session tabs");
    };
    let env = [
        (plugin::SESSION_ENV, s.id.as_str()),
        (plugin::NAME_ENV, s.name.as_str()),
    ];
    let pane = herdr.open_tab(&ctx.plugin_id, plugin::SESSION, workspace, Some(&s.cwd), &env, false)?;
    saved.sessions.insert(
        s.id.clone(),
        Tracked {
            pane: PaneRef::of(&pane),
            shown: None,
        },
    );
    report(herdr, saved, seq, s)?;
    // A name for `herdr agent prompt <name>`; names are unique, so a second
    // session with the same name gets its id appended.
    let name = herdr::agent_slug(&s.name);
    if herdr.rename_agent(&pane.pane_id, &name).is_err() {
        let _ = herdr.rename_agent(&pane.pane_id, &format!("{}-{}", truncate(&name, 23), s.id));
    }
    Ok(())
}

fn truncate(s: &str, n: usize) -> &str {
    s.char_indices().nth(n).map(|(i, _)| &s[..i]).unwrap_or(s)
}

fn report(herdr: &Herdr, saved: &mut Saved, seq: &mut Seq, s: &Session) -> Result<()> {
    let Some(t) = saved.sessions.get_mut(&s.id) else {
        return Ok(());
    };
    let pane = t.pane.pane_id.clone();
    let renamed = t.shown.as_ref().is_none_or(|old| old.name != s.name);
    herdr.report_agent(
        &pane,
        plugin::SOURCE,
        s.state.herdr(),
        s.session_id.as_deref(),
        seq.next(),
    )?;
    herdr.report_display(&pane, plugin::DISPLAY_SOURCE, &display(s), seq.next())?;
    if renamed {
        herdr.rename_tab(&t.pane.tab_id, &s.name)?;
    }
    t.shown = Some(Shown::of(s));
    Ok(())
}

/// The sidebar's view of a session: its name, its folder as the title, and
/// `$summary` / `$project` / `$claude_state` for custom Agent rows.
pub fn display(s: &Session) -> Display {
    let mut labels = vec![("blocked", "needs input")];
    if matches!(s.state, State::Failed | State::Stopped) {
        labels.extend([("idle", s.state.label()), ("done", s.state.label())]);
    }
    Display {
        agent_name: s.name.clone(),
        title: s.project(),
        labels,
        tokens: vec![
            ("summary", s.detail.clone().unwrap_or_default()),
            ("project", s.project()),
            ("claude_state", s.state.label().to_string()),
            ("claude_session", s.id.clone()),
        ],
    }
}

/// Increasing report numbers, across watcher restarts too (Herdr ignores a
/// report numbered at or below the last one it took from a source).
#[derive(Default)]
struct Seq(u64);

impl Seq {
    fn next(&mut self) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.0 = now.max(self.0 + 1);
        self.0
    }
}

/// Wakes the watcher when Claude's session registry, a job's state or the
/// plugin's own state dir (the reopen request) changes.
struct Watch {
    rx: mpsc::Receiver<()>,
    _watchers: Vec<notify::RecommendedWatcher>,
}

impl Watch {
    fn new(ctx: &Context) -> Watch {
        let (tx, rx) = mpsc::channel();
        let claude = claude::config_dir();
        let dirs: [(PathBuf, RecursiveMode); 3] = [
            (claude.join("sessions"), RecursiveMode::NonRecursive),
            (claude.join("jobs"), RecursiveMode::Recursive),
            (ctx.state_dir.clone(), RecursiveMode::NonRecursive),
        ];
        let reopen = ctx.reopen_path();
        let mut watchers = Vec::new();
        for (dir, mode) in dirs {
            let tx = tx.clone();
            let reopen = reopen.clone();
            let w = notify::recommended_watcher(move |ev: notify::Result<notify::Event>| {
                if let Ok(ev) = ev
                    && ev.paths.iter().any(|p| relevant(p, &reopen))
                {
                    let _ = tx.send(());
                }
            });
            if let Ok(mut w) = w
                && w.watch(&dir, mode).is_ok()
            {
                watchers.push(w);
            }
        }
        Watch {
            rx,
            _watchers: watchers,
        }
    }

    /// Until a relevant file changes (and settles) or `poll` passes.
    fn wait(&mut self, poll: Duration) {
        if self.rx.recv_timeout(poll).is_ok() {
            while self.rx.recv_timeout(SETTLE).is_ok() {}
        }
    }
}

/// A session registry entry, a job's `state.json`, a job folder coming or
/// going, or the reopen request; not the scratch files jobs write.
fn relevant(path: &Path, reopen: &Path) -> bool {
    if path == reopen {
        return true;
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let parent = path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str());
    match parent {
        Some("sessions") => name.ends_with(".json"),
        Some("jobs") => true,
        _ => {
            name == "state.json"
                && path.parent().and_then(Path::parent).and_then(Path::file_name) == Some("jobs".as_ref())
        }
    }
}

/// The watcher's own screen: redrawn only when it changes.
#[derive(Default)]
struct Screen {
    last: String,
}

impl Screen {
    fn draw(&mut self, text: &str) {
        if text == self.last {
            return;
        }
        let mut out = std::io::stdout().lock();
        let _ = write!(out, "\x1b[?25l\x1b[H\x1b[2J{}", text.replace('\n', "\r\n"));
        let _ = out.flush();
        self.last = text.to_string();
    }
}

pub fn render(result: &Result<Vec<Session>>, saved: &Saved, config: &Config) -> String {
    const DIM: &str = "\x1b[2m";
    const BOLD: &str = "\x1b[1m";
    const RESET: &str = "\x1b[0m";
    let mut out = String::new();
    let sessions = match result {
        Ok(s) => s.as_slice(),
        Err(_) => &[],
    };
    out.push_str(&format!(
        "{BOLD}Claude background sessions{RESET}  {DIM}{}{RESET}\n\n",
        sessions.len()
    ));
    if let Err(e) = result {
        out.push_str(&format!("\x1b[31m{e}{RESET}\n\n"));
    } else if sessions.is_empty() {
        out.push_str(&format!("{DIM}None running. Start one with `claude --bg`.{RESET}\n"));
    }
    for s in sessions {
        let (mark, colour) = match s.state {
            State::Blocked => ("●", "\x1b[33m"),
            State::Working => ("◐", "\x1b[36m"),
            State::Done => ("✓", "\x1b[32m"),
            State::Failed => ("✗", "\x1b[31m"),
            State::Stopped => ("○", DIM),
        };
        let hidden = if saved.dismissed.contains(&s.id) {
            "  (tab closed)"
        } else {
            ""
        };
        out.push_str(&format!(
            "{colour}{mark}{RESET} {BOLD}{}{RESET}  {colour}{}{RESET}  {DIM}{}{hidden}{RESET}\n",
            s.name,
            s.state.label(),
            s.project()
        ));
        if let Some(d) = &s.detail {
            out.push_str(&format!("  {DIM}{}{RESET}\n", truncate(d, 100)));
        }
    }
    out.push_str(&format!(
        "\n{DIM}Each session has its own tab in this workspace (\"{}\"). A tab you close stays closed\n\
         until the session ends or you run \"Claude: show background sessions\".{RESET}\n",
        config.workspace
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(state: State) -> Session {
        Session {
            id: "abc".into(),
            session_id: Some("abc-1".into()),
            name: "fix auth".into(),
            cwd: PathBuf::from("/r/api"),
            state,
            detail: Some("Reading a.rs".into()),
        }
    }

    #[test]
    fn the_sidebar_shows_name_folder_and_tokens() {
        let d = display(&session(State::Working));
        assert_eq!(d.agent_name, "fix auth");
        assert_eq!(d.title, "api");
        assert_eq!(d.labels, [("blocked", "needs input")]);
        assert!(d.tokens.contains(&("summary", "Reading a.rs".into())));
        assert!(d.tokens.contains(&("claude_session", "abc".into())));
        // Failed and stopped are idle to Herdr, so they say what they are.
        let d = display(&session(State::Failed));
        assert!(d.labels.contains(&("idle", "failed")));
        assert!(d.labels.contains(&("done", "failed")));
        // No detail: the token is cleared.
        let mut s = session(State::Done);
        s.detail = None;
        assert!(display(&s).tokens.contains(&("summary", String::new())));
    }

    #[test]
    fn only_claude_state_files_wake_the_watcher() {
        let reopen = Path::new("/s/server-1.reopen");
        assert!(relevant(reopen, reopen));
        assert!(relevant(Path::new("/c/sessions/123.json"), reopen));
        assert!(!relevant(Path::new("/c/sessions/123.key"), reopen));
        assert!(relevant(Path::new("/c/jobs/abc"), reopen));
        assert!(relevant(Path::new("/c/jobs/abc/state.json"), reopen));
        assert!(!relevant(Path::new("/c/jobs/abc/timeline.jsonl"), reopen));
        assert!(!relevant(Path::new("/c/jobs/abc/tmp/state.json"), reopen));
        assert!(!relevant(Path::new("/s/server-1.json"), reopen));
    }

    #[test]
    fn report_numbers_always_grow() {
        let mut seq = Seq::default();
        let a = seq.next();
        let b = seq.next();
        assert!(b > a);
        seq.0 = u64::MAX / 2;
        assert_eq!(seq.next(), u64::MAX / 2 + 1);
    }

    #[test]
    fn the_screen_lists_sessions_and_errors() {
        let saved = Saved::default();
        let config = Config::default();
        let text = render(&Ok(vec![session(State::Blocked)]), &saved, &config);
        assert!(text.contains("fix auth"));
        assert!(text.contains("needs input"));
        assert!(text.contains("Reading a.rs"));
        let text = render(&Ok(vec![]), &saved, &config);
        assert!(text.contains("None running"));
        let text = render(&Err(anyhow::anyhow!("cannot run `claude`")), &saved, &config);
        assert!(text.contains("cannot run `claude`"));
        let mut saved = Saved::default();
        saved.dismissed.insert("abc".into());
        assert!(render(&Ok(vec![session(State::Done)]), &saved, &config).contains("(tab closed)"));
    }
}
