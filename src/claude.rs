//! Claude Code's side: which background sessions are running and what they are
//! doing, from `claude agents --json` plus each job's `state.json`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// What a session is doing, as the agent view sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    Working,
    /// Waiting for an answer or a permission.
    Blocked,
    /// Its turn ended; ready for the next prompt.
    Done,
    Failed,
    Stopped,
}

impl State {
    /// From `state` (what the agent view makes of the session) and `status`
    /// (what its process is doing: `busy`, `shell`, `idle`, `waiting`). A
    /// session that ended its turn is `idle` while its state may still read
    /// `working`, so the process status wins.
    pub fn parse(state: &str, status: &str) -> State {
        match (state, status) {
            ("blocked", _) | (_, "waiting") => State::Blocked,
            ("failed", _) => State::Failed,
            ("stopped", _) => State::Stopped,
            ("done" | "completed", _) | (_, "idle") => State::Done,
            _ => State::Working,
        }
    }

    /// The Herdr lifecycle state: it drives the sidebar mark, waits and
    /// notifications (`working` → `idle` is a finished turn, `blocked` asks
    /// for attention).
    pub fn herdr(self) -> &'static str {
        match self {
            State::Working => "working",
            State::Blocked => "blocked",
            State::Done | State::Failed | State::Stopped => "idle",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            State::Working => "working",
            State::Blocked => "needs input",
            State::Done => "done",
            State::Failed => "failed",
            State::Stopped => "stopped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// Short id: what `claude attach` takes.
    pub id: String,
    /// The conversation id, reported to Herdr as the agent session.
    pub session_id: Option<String>,
    pub name: String,
    pub cwd: PathBuf,
    pub state: State,
    /// What it is doing right now ("Reading src/main.rs"), from `state.json`.
    pub detail: Option<String>,
}

impl Session {
    /// The folder name, for the tab and the agent's title.
    pub fn project(&self) -> String {
        self.cwd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Raw {
    id: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

/// Parses `claude agents --json`, keeping only background sessions: an
/// interactive one already lives in some terminal (and Herdr sees it there if
/// that terminal is a Herdr pane).
pub fn parse(json: &str) -> Result<Vec<Session>> {
    let raw: Vec<Raw> = serde_json::from_str(json).context("unexpected `claude agents --json` output")?;
    Ok(raw
        .into_iter()
        .filter(|r| r.kind.as_deref().is_none_or(|k| k == "background" || k == "bg"))
        .map(|r| Session {
            name: r.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| r.id.clone()),
            state: State::parse(r.state.as_deref().unwrap_or(""), r.status.as_deref().unwrap_or("")),
            cwd: r.cwd.unwrap_or_default(),
            session_id: r.session_id.filter(|s| !s.is_empty()),
            detail: None,
            id: r.id,
        })
        .collect())
}

/// Claude Code's config directory: `CLAUDE_CONFIG_DIR`, or `~/.claude`.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    home().join(".claude")
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

/// The `detail` line of a background job's `state.json`, if it has one.
pub fn detail(config_dir: &Path, id: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct JobState {
        detail: Option<String>,
    }
    let text = std::fs::read_to_string(config_dir.join("jobs").join(id).join("state.json")).ok()?;
    let detail = serde_json::from_str::<JobState>(&text).ok()?.detail?;
    let line = detail.lines().next().unwrap_or("").trim();
    (!line.is_empty()).then(|| line.to_string())
}

/// Runs `<claude> agents --json` and reads each session's detail.
pub fn list(claude: &str) -> Result<Vec<Session>> {
    let out = Command::new(claude)
        .args(["agents", "--json"])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("cannot run `{claude}` (set `claude` in the plugin's config.toml)"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("`{claude} agents --json` failed: {}", err.trim());
    }
    let mut sessions = parse(&String::from_utf8_lossy(&out.stdout))?;
    let dir = config_dir();
    for s in &mut sessions {
        s.detail = detail(&dir, &s.id);
    }
    Ok(sessions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_background_sessions_from_claude_agents() {
        let json = r#"[
            {"pid": 1, "id": "1a2b3c4d", "cwd": "/r/shop", "kind": "background", "name": "fix-login",
             "sessionId": "1a2b3c4d-0001", "status": "busy", "state": "working"},
            {"id": "5e6f7a8b", "cwd": "/r/blog", "kind": "background", "name": "  ", "state": "blocked"},
            {"id": "inter", "kind": "interactive", "state": "working"},
            {"id": "old", "state": "done"}
        ]"#;
        let s = parse(json).unwrap();
        let ids: Vec<_> = s.iter().map(|s| s.id.as_str()).collect();
        // Interactive sessions are left out; a missing kind is kept.
        assert_eq!(ids, ["1a2b3c4d", "5e6f7a8b", "old"]);
        assert_eq!(s[0].name, "fix-login");
        assert_eq!(s[0].session_id.as_deref(), Some("1a2b3c4d-0001"));
        assert_eq!(s[0].cwd, PathBuf::from("/r/shop"));
        assert_eq!(s[0].project(), "shop");
        assert_eq!(s[0].state, State::Working);
        // No name: the id stands in.
        assert_eq!(s[1].name, "5e6f7a8b");
        assert_eq!(s[1].state, State::Blocked);
        assert!(parse("not json").is_err());
        assert!(parse("[]").unwrap().is_empty());
    }

    #[test]
    fn the_process_status_wins_over_a_stale_state() {
        use State::*;
        let st = State::parse;
        assert_eq!(st("working", "busy"), Working);
        assert_eq!(st("working", "shell"), Working);
        assert_eq!(st("working", "waiting"), Blocked);
        assert_eq!(st("working", "idle"), Done);
        assert_eq!(st("done", "busy"), Done);
        assert_eq!(st("completed", ""), Done);
        assert_eq!(st("failed", "idle"), Failed);
        assert_eq!(st("stopped", "idle"), Stopped);
        assert_eq!(st("", ""), Working);
    }

    #[test]
    fn herdr_sees_three_lifecycle_states() {
        assert_eq!(State::Working.herdr(), "working");
        assert_eq!(State::Blocked.herdr(), "blocked");
        for s in [State::Done, State::Failed, State::Stopped] {
            assert_eq!(s.herdr(), "idle");
        }
        assert_eq!(State::Blocked.label(), "needs input");
    }

    #[test]
    fn detail_is_the_first_line_of_the_job_state() {
        let dir = std::env::temp_dir().join(format!("hca-detail-{}", std::process::id()));
        let job = dir.join("jobs").join("abc");
        std::fs::create_dir_all(&job).unwrap();
        std::fs::write(
            job.join("state.json"),
            r#"{"state":"working","detail":"Reading a.rs\nmore"}"#,
        )
        .unwrap();
        assert_eq!(detail(&dir, "abc").as_deref(), Some("Reading a.rs"));
        assert_eq!(detail(&dir, "missing"), None);
        std::fs::write(job.join("state.json"), r#"{"detail":"  "}"#).unwrap();
        assert_eq!(detail(&dir, "abc"), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
