//! The watcher: one headless process per Herdr server (the startup hook, or
//! the `open` action when none runs). It looks at Claude's background
//! sessions and Herdr's panes whenever Claude's files change (and every
//! couple of seconds anyway), keeps one tab per session in the space open on
//! the session's project, and reports each session's state to Herdr.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use notify::{RecursiveMode, Watcher};

use crate::claude::{self, Session};
use crate::herdr::{self, Herdr, Pane};
use crate::plugin::{self, Context, Lock};
use crate::spaces;
use crate::sync::{self, Op, PaneRef, Saved, Shown, Tracked};

/// File events come in bursts (a turn rewrites several files): wait this long
/// for the burst to end before looking.
const SETTLE: Duration = Duration::from_millis(150);
/// How often (in looks) to check the plugin is still enabled.
const ENABLED_EVERY: u32 = 15;

/// Runs until the Herdr server goes away or the plugin is disabled. Returns
/// at once when another watcher holds this server after `wait`.
pub fn run(ctx: &Context, wait: Duration) -> Result<()> {
    let Some(_lock) = Lock::acquire_within(&ctx.lock_path(), wait)? else {
        return Ok(());
    };
    let herdr = Herdr::from_env();
    let mut watch = Watch::new(ctx);
    let mut saved = Saved::load(&ctx.state_path());
    let mut seq = Seq::default();
    for n in 0.. {
        if n % ENABLED_EVERY == ENABLED_EVERY - 1 && !herdr.plugin_enabled(&ctx.plugin_id).unwrap_or(true) {
            return Ok(());
        }
        let config = ctx.config();
        let result = match &config {
            Ok(config) => look(&herdr, ctx, config, &mut saved, &mut seq),
            Err(e) => Err(anyhow::anyhow!("{e}")),
        };
        if let Err(e) = &result {
            // Stopped, or restarting: the next server starts its own watcher.
            if herdr::server_gone(e) {
                return Ok(());
            }
            if saved.error.as_deref() != Some(&e.to_string()) {
                eprintln!("herdr-claude-agents: {e:#}");
            }
        }
        saved.error = result.err().map(|e| e.to_string());
        if let Err(e) = saved.save(&ctx.state_path()) {
            eprintln!("herdr-claude-agents: cannot save state: {e}");
        }
        watch.wait(config.map(|c| c.poll()).unwrap_or(Duration::from_secs(2)));
    }
    Ok(())
}

/// One look at both sides and the changes it calls for.
fn look(herdr: &Herdr, ctx: &Context, config: &plugin::Config, saved: &mut Saved, seq: &mut Seq) -> Result<()> {
    if std::fs::remove_file(ctx.reopen_path()).is_ok() {
        saved.dismissed.clear();
    }
    // Herdr first: once its server is gone there is nothing to run Claude for.
    let mut panes = herdr.panes()?;
    let sessions = claude::list(&config.claude)?;
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
                panes.retain(|p| p.pane_id != pane_id);
                open(herdr, ctx, &mut panes, saved, seq, session(&id).unwrap())
            }
            Op::Open(id) => open(herdr, ctx, &mut panes, saved, seq, session(&id).unwrap()),
            Op::Report(id) => report(herdr, saved, seq, session(&id).unwrap()),
        };
        if let Err(e) = done {
            if herdr::server_gone(&e) {
                return Err(e);
            }
            errors.push(e.to_string());
        }
    }
    if let Some(e) = errors.first() {
        bail!("{e}");
    }
    Ok(())
}

/// Opens a session's tab in the space open on its project, creating that
/// space when there is none. `panes` learns the new panes, so the next
/// session in the same project finds the space.
fn open(
    herdr: &Herdr,
    ctx: &Context,
    panes: &mut Vec<Pane>,
    saved: &mut Saved,
    seq: &mut Seq,
    s: &Session,
) -> Result<()> {
    let root = spaces::project_root(&s.cwd);
    let space = match spaces::space_for(&root, &s.cwd, panes) {
        Some(space) => space,
        None => {
            let first = herdr.create_workspace(&root)?;
            let space = first.workspace_id.clone();
            panes.push(first);
            space
        }
    };
    let env = [
        (plugin::SESSION_ENV, s.id.as_str()),
        (plugin::NAME_ENV, s.name.as_str()),
    ];
    let pane = herdr.open_tab(&ctx.plugin_id, plugin::SESSION, &space, Some(&s.cwd), &env)?;
    // So it looks like any pane running Claude, not "Claude session".
    herdr.clear_pane_label(&pane.pane_id)?;
    saved.sessions.insert(
        s.id.clone(),
        Tracked {
            pane: PaneRef::of(&pane),
            name: s.name.clone(),
            shown: None,
        },
    );
    panes.push(pane);
    report(herdr, saved, seq, s)
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
    herdr.report_tokens(&pane, plugin::TOKEN_SOURCE, &tokens(s), seq.next())?;
    if renamed {
        herdr.rename_tab(&t.pane.tab_id, &s.name)?;
    }
    t.name = s.name.clone();
    t.shown = Some(Shown::of(s));
    Ok(())
}

/// `$summary` / `$project` / `$claude_state` / `$claude_session` for custom
/// Agent sidebar rows. Nothing else: the default rows show the agent as any
/// other Claude.
pub fn tokens(s: &Session) -> Vec<(&'static str, String)> {
    vec![
        ("summary", s.detail.clone().unwrap_or_default()),
        ("project", s.project()),
        ("claude_state", s.state.label().to_string()),
        ("claude_session", s.id.clone()),
    ]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::State;

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
    fn tokens_carry_what_the_default_rows_leave_out() {
        let t = tokens(&session(State::Blocked));
        assert!(t.contains(&("summary", "Reading a.rs".into())));
        assert!(t.contains(&("project", "api".into())));
        assert!(t.contains(&("claude_state", "needs input".into())));
        assert!(t.contains(&("claude_session", "abc".into())));
        // No detail: the token is cleared.
        let mut s = session(State::Done);
        s.detail = None;
        assert!(tokens(&s).contains(&("summary", String::new())));
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
}
