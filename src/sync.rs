//! Keeping one Herdr tab per background session: what the watcher remembers
//! and what it has to do after each look at both sides.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::claude::{Session, State};
use crate::herdr::Pane;

/// A pane the plugin opened. A pane id survives a Herdr restart (the pane comes
/// back as a plain shell), the terminal id doesn't: that's how a restored pane
/// is told from the one we opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneRef {
    pub pane_id: String,
    pub tab_id: String,
    pub terminal_id: String,
}

impl PaneRef {
    pub fn of(pane: &Pane) -> PaneRef {
        PaneRef {
            pane_id: pane.pane_id.clone(),
            tab_id: pane.tab_id.clone(),
            terminal_id: pane.terminal_id.clone(),
        }
    }
}

/// What was last reported for a session, to report again only on change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub name: String,
    pub state: State,
    pub detail: Option<String>,
}

impl Shown {
    pub fn of(s: &Session) -> Shown {
        Shown {
            name: s.name.clone(),
            state: s.state,
            detail: s.detail.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tracked {
    pub pane: PaneRef,
    /// Not saved: a new watcher reports everything once.
    #[serde(skip)]
    pub shown: Option<Shown>,
}

/// The watcher's memory, saved per Herdr server in the plugin's state dir.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    /// The last look's error, if it failed: the watcher has no screen.
    #[serde(default)]
    pub error: Option<String>,
    /// Session id → its tab.
    #[serde(default)]
    pub sessions: BTreeMap<String, Tracked>,
    /// Sessions whose tab the user closed: not opened again until they end or
    /// "Claude: show background sessions" is run.
    #[serde(default)]
    pub dismissed: BTreeSet<String>,
}

impl Saved {
    pub fn load(path: &Path) -> Saved {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// The session ended (or its restored pane is a leftover): close the pane.
    Close { id: String, pane_id: String },
    /// The user closed the session's tab.
    Dismiss(String),
    /// A dismissed session ended: forget it.
    Forget(String),
    /// The pane is a shell Herdr restored after a restart: replace it.
    Replace { id: String, pane_id: String },
    /// A session without a tab.
    Open(String),
    /// Its name, state or detail changed.
    Report(String),
}

/// What to do to bring Herdr's tabs in line with `sessions`.
pub fn plan(saved: &Saved, sessions: &[Session], panes: &[Pane]) -> Vec<Op> {
    let live = |id: &str| sessions.iter().find(|s| s.id == id);
    let pane = |r: &PaneRef| panes.iter().find(|p| p.pane_id == r.pane_id);
    let mut ops = Vec::new();
    for (id, t) in &saved.sessions {
        let session = live(id);
        match (pane(&t.pane), session) {
            (None, Some(_)) => ops.push(Op::Dismiss(id.clone())),
            (None, None) => ops.push(Op::Forget(id.clone())),
            (Some(_), None) => ops.push(Op::Close {
                id: id.clone(),
                pane_id: t.pane.pane_id.clone(),
            }),
            (Some(p), Some(_)) if p.terminal_id != t.pane.terminal_id => ops.push(Op::Replace {
                id: id.clone(),
                pane_id: t.pane.pane_id.clone(),
            }),
            (Some(_), Some(s)) => {
                if t.shown.as_ref() != Some(&Shown::of(s)) {
                    ops.push(Op::Report(id.clone()));
                }
            }
        }
    }
    for id in &saved.dismissed {
        if live(id).is_none() {
            ops.push(Op::Forget(id.clone()));
        }
    }
    for s in sessions {
        if !saved.sessions.contains_key(&s.id) && !saved.dismissed.contains(&s.id) {
            ops.push(Op::Open(s.id.clone()));
        }
    }
    ops
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn session(id: &str, state: State) -> Session {
        Session {
            id: id.into(),
            session_id: None,
            name: id.into(),
            cwd: PathBuf::from("/r/p"),
            state,
            detail: None,
        }
    }

    fn pane(id: &str, terminal: &str) -> Pane {
        Pane {
            pane_id: id.into(),
            tab_id: format!("{id}-tab"),
            workspace_id: "w1".into(),
            terminal_id: terminal.into(),
            label: None,
            cwd: Some("/r/p".into()),
        }
    }

    fn tracked(saved: &mut Saved, id: &str, p: &Pane, shown: Option<&Session>) {
        let t = Tracked {
            pane: PaneRef::of(p),
            shown: shown.map(Shown::of),
        };
        saved.sessions.insert(id.into(), t);
    }

    #[test]
    fn new_sessions_are_opened_and_known_ones_left_alone() {
        let a = session("a", State::Working);
        let b = session("b", State::Done);
        let pa = pane("w1:p2", "t1");
        let mut saved = Saved::default();
        assert_eq!(
            plan(&saved, &[a.clone(), b.clone()], &[]),
            [Op::Open("a".into()), Op::Open("b".into())]
        );
        tracked(&mut saved, "a", &pa, Some(&a));
        assert_eq!(
            plan(&saved, &[a.clone(), b], std::slice::from_ref(&pa)),
            [Op::Open("b".into())]
        );
        assert_eq!(plan(&saved, &[a], &[pa]), []);
    }

    #[test]
    fn a_change_of_state_name_or_detail_is_reported() {
        let a = session("a", State::Working);
        let pa = pane("w1:p2", "t1");
        let mut saved = Saved::default();
        tracked(&mut saved, "a", &pa, Some(&a));
        let report = [Op::Report("a".into())];
        let mut b = a.clone();
        b.state = State::Blocked;
        assert_eq!(
            plan(&saved, std::slice::from_ref(&b), std::slice::from_ref(&pa)),
            report
        );
        let mut b = a.clone();
        b.name = "renamed".into();
        assert_eq!(plan(&saved, &[b], std::slice::from_ref(&pa)), report);
        let mut b = a.clone();
        b.detail = Some("Reading a.rs".into());
        assert_eq!(plan(&saved, &[b], std::slice::from_ref(&pa)), report);
        // Nothing reported yet (a new watcher): report once.
        saved.sessions.get_mut("a").unwrap().shown = None;
        assert_eq!(plan(&saved, &[a], &[pa]), report);
    }

    #[test]
    fn an_ended_session_closes_its_tab() {
        let pa = pane("w1:p2", "t1");
        let mut saved = Saved::default();
        tracked(&mut saved, "a", &pa, None);
        assert_eq!(
            plan(&saved, &[], &[pa]),
            [Op::Close {
                id: "a".into(),
                pane_id: "w1:p2".into()
            }]
        );
    }

    #[test]
    fn a_tab_the_user_closed_stays_closed_until_the_session_ends() {
        let a = session("a", State::Working);
        let mut saved = Saved::default();
        tracked(&mut saved, "a", &pane("w1:p2", "t1"), Some(&a));
        assert_eq!(plan(&saved, std::slice::from_ref(&a), &[]), [Op::Dismiss("a".into())]);
        saved.sessions.clear();
        saved.dismissed.insert("a".into());
        assert_eq!(plan(&saved, &[a], &[]), []);
        assert_eq!(plan(&saved, &[], &[]), [Op::Forget("a".into())]);
        // Its pane gone and the session gone too: just forget it.
        tracked(&mut saved, "b", &pane("w1:p3", "t1"), None);
        saved.dismissed.clear();
        assert_eq!(plan(&saved, &[], &[]), [Op::Forget("b".into())]);
    }

    #[test]
    fn a_pane_restored_after_a_herdr_restart_is_replaced() {
        let a = session("a", State::Working);
        let mut saved = Saved::default();
        tracked(&mut saved, "a", &pane("w1:p2", "t1"), Some(&a));
        let restored = pane("w1:p2", "t2");
        assert_eq!(
            plan(&saved, &[a], std::slice::from_ref(&restored)),
            [Op::Replace {
                id: "a".into(),
                pane_id: "w1:p2".into()
            }]
        );
        // The session ended while Herdr was down: just close the leftover.
        assert_eq!(
            plan(&saved, &[], &[restored]),
            [Op::Close {
                id: "a".into(),
                pane_id: "w1:p2".into()
            }]
        );
    }

    #[test]
    fn saved_state_round_trips_without_what_was_shown() {
        let a = session("a", State::Working);
        let mut saved = Saved::default();
        tracked(&mut saved, "a", &pane("w1:p2", "t1"), Some(&a));
        saved.dismissed.insert("b".into());
        saved.error = Some("cannot run `claude`".into());
        let path = std::env::temp_dir().join(format!("hca-saved-{}/s.json", std::process::id()));
        saved.save(&path).unwrap();
        let back = Saved::load(&path);
        assert_eq!(back.sessions["a"].pane, saved.sessions["a"].pane);
        assert_eq!(back.sessions["a"].shown, None);
        assert_eq!(back.dismissed, saved.dismissed);
        assert_eq!(back.error, saved.error);
        // State from v0.1 (with its sessions tab) still loads.
        std::fs::write(
            &path,
            r#"{"watcher":{"pane_id":"w1:p1","tab_id":"w1:t1","terminal_id":"t"}}"#,
        )
        .unwrap();
        assert_eq!(Saved::load(&path), Saved::default());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        // Missing or broken: start empty.
        assert_eq!(Saved::load(Path::new("/nonexistent/s.json")), Saved::default());
    }
}
