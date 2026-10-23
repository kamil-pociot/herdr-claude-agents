//! End to end: a real `herdr server`, isolated by its own XDG dirs, with this
//! plugin linked and a fake `claude` whose session list a test rewrites.
//! Skipped when `herdr` is not on PATH.

use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const PLUGIN: &str = "kamilpociot.claude-agents";
const WAIT: Duration = Duration::from_secs(25);

struct Herdr {
    root: PathBuf,
    server: Option<Child>,
}

impl Herdr {
    /// A fresh isolated Herdr with the plugin linked, not started yet, and two
    /// projects: `repos/alpha` (a git repository) and `repos/beta` (a folder).
    fn new() -> Option<Herdr> {
        if Command::new("herdr").arg("--version").output().is_err() {
            eprintln!("herdr not on PATH: skipping");
            return None;
        }
        static N: AtomicUsize = AtomicUsize::new(0);
        // Short: the server's Unix socket path must stay under ~100 bytes.
        let root = PathBuf::from(format!(
            "/tmp/hca-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        for d in [
            "config",
            "state",
            "data",
            "cache",
            "claude/sessions",
            "claude/jobs",
            "repos/alpha/src",
            "repos/beta",
        ] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        Command::new("git")
            .arg("-C")
            .arg(root.join("repos/alpha"))
            .args(["init", "-q"])
            .status()
            .unwrap();
        let fake = root.join("claude-fake");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\ncase \"$1\" in\n  agents) cat '{list}' ;;\n  \
                 attach) echo \"fake attach $2\"; while read l; do echo \"got $l\"; done ;;\nesac\n",
                list = root.join("list.json").display()
            ),
        )
        .unwrap();
        Command::new("chmod").arg("+x").arg(&fake).status().unwrap();
        let h = Herdr { root, server: None };
        h.sessions(&[]);
        let linked = h.cli(&["plugin", "link", env!("CARGO_MANIFEST_DIR")]);
        assert!(linked.get("result").is_some(), "plugin link: {linked}");
        let config = PathBuf::from(h.cli_text(&["plugin", "config-dir", PLUGIN]).trim());
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("config.toml"),
            format!("claude = '{}'\npoll_seconds = 1\n", fake.display()),
        )
        .unwrap();
        Some(h)
    }

    fn dir(&self, project: &str) -> String {
        self.root.join("repos").join(project).display().to_string()
    }

    fn command(&self) -> Command {
        let mut c = Command::new("herdr");
        for (k, _) in std::env::vars() {
            if k.starts_with("HERDR_") {
                c.env_remove(k);
            }
        }
        c.env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("CLAUDE_CONFIG_DIR", self.root.join("claude"))
            .env("HERDR_CLAUDE_AGENTS_BIN", env!("CARGO_BIN_EXE_herdr-claude-agents"))
            .stdin(Stdio::null());
        c
    }

    fn start(&mut self) {
        let log = File::create(self.root.join("server.log")).unwrap();
        let child = self
            .command()
            .arg("server")
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap();
        self.server = Some(child);
        self.wait_for("the server", || {
            self.cli(&["workspace", "list"]).get("result").is_some()
        });
    }

    fn stop(&mut self) {
        let _ = self.command().args(["server", "stop"]).output();
        if let Some(mut c) = self.server.take() {
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                if let Ok(Some(_)) = c.try_wait() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    fn cli_text(&self, args: &[&str]) -> String {
        let out = self.command().args(args).output().unwrap();
        let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
        s.push_str(&String::from_utf8_lossy(&out.stderr));
        s
    }

    fn cli(&self, args: &[&str]) -> Value {
        let text = self.cli_text(args);
        serde_json::from_str(text.trim()).unwrap_or(Value::Null)
    }

    /// What the fake `claude agents --json` prints from now on.
    fn sessions(&self, sessions: &[Value]) {
        let tmp = self.root.join("list.tmp");
        std::fs::write(&tmp, Value::Array(sessions.to_vec()).to_string()).unwrap();
        std::fs::rename(tmp, self.root.join("list.json")).unwrap();
    }

    fn agents(&self) -> Vec<Value> {
        let v = self.cli(&["agent", "list"]);
        v["result"]["agents"].as_array().cloned().unwrap_or_default()
    }

    /// The agent showing session `id`.
    fn agent(&self, id: &str) -> Option<Value> {
        self.agents().into_iter().find(|a| a["tokens"]["claude_session"] == id)
    }

    fn status_of(&self, id: &str) -> Option<String> {
        self.agent(id)
            .and_then(|a| a["agent_status"].as_str().map(String::from))
    }

    fn panes(&self) -> Vec<Value> {
        let v = self.cli(&["pane", "list"]);
        v["result"]["panes"].as_array().cloned().unwrap_or_default()
    }

    fn workspaces(&self) -> Vec<Value> {
        let v = self.cli(&["workspace", "list"]);
        v["result"]["workspaces"].as_array().cloned().unwrap_or_default()
    }

    /// A space opened on `project`, as a user would.
    fn open_space(&self, project: &str) -> String {
        let v = self.cli(&["workspace", "create", "--cwd", &self.dir(project), "--no-focus"]);
        v["result"]["workspace"]["workspace_id"].as_str().unwrap().to_string()
    }

    /// The space with a pane opened in `project`'s folder.
    fn space_of(&self, project: &str) -> Option<String> {
        let dir = self.dir(project);
        self.panes()
            .into_iter()
            .find(|p| p["cwd"].as_str().is_some_and(|c| c.ends_with(&dir)))
            .and_then(|p| p["workspace_id"].as_str().map(String::from))
    }

    fn panes_in(&self, space: &str) -> usize {
        self.panes().into_iter().filter(|p| p["workspace_id"] == space).count()
    }

    /// Whether the plugin's watcher runs: it holds a lock in its state dir.
    fn watcher_running(&self) -> bool {
        let dir = self.root.join("state/herdr/plugins").join(PLUGIN);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return false;
        };
        entries
            .flatten()
            .any(|e| e.path().extension().is_some_and(|x| x == "lock") && locked(&e.path()))
    }

    fn wait_for(&self, what: &str, mut ok: impl FnMut() -> bool) {
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline {
            if ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let state = std::fs::read_dir(self.root.join("state/herdr/plugins").join(PLUGIN))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
            .collect::<String>();
        panic!(
            "timed out waiting for {what}\nagents: {:#?}\npanes: {:#?}\nlog: {}\nwatcher state: {state}",
            self.agents(),
            self.panes(),
            self.cli_text(&["plugin", "log", "list", "--plugin", PLUGIN])
        );
    }
}

impl Drop for Herdr {
    fn drop(&mut self) {
        self.stop();
        // The watcher outlives the server by up to a poll; its folders go
        // only after it has stopped.
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.watcher_running() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn locked(path: &Path) -> bool {
    let Ok(file) = File::options().write(true).open(path) else {
        return false;
    };
    // SAFETY: flock on a file we own; the lock goes with the file.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) != 0 }
}

fn session(id: &str, name: &str, cwd: &str, state: &str, status: &str) -> Value {
    json!({"pid": 1, "id": id, "cwd": cwd, "kind": "background",
           "sessionId": format!("{id}-uuid"), "name": name, "state": state, "status": status})
}

#[test]
fn sessions_open_in_the_space_of_their_project_and_follow_changes() {
    let Some(mut h) = Herdr::new() else { return };
    h.start();
    let alpha = h.open_space("alpha");
    std::fs::create_dir_all(h.root.join("claude/jobs/aaaa")).unwrap();
    std::fs::write(
        h.root.join("claude/jobs/aaaa/state.json"),
        r#"{"state":"working","detail":"Reading src/main.rs"}"#,
    )
    .unwrap();
    let alpha_src = format!("{}/src", h.dir("alpha"));
    h.sessions(&[
        session("aaaa", "fix auth", &alpha_src, "working", "busy"),
        session("bbbb", "write docs", &h.dir("beta"), "working", "waiting"),
    ]);

    h.wait_for("both sessions as agents", || {
        h.status_of("aaaa").as_deref() == Some("working") && h.status_of("bbbb").as_deref() == Some("blocked")
    });
    // In alpha's git repository: alpha's space, in the session's folder.
    let a = h.agent("aaaa").unwrap();
    assert_eq!(a["workspace_id"], alpha.as_str());
    assert!(a["cwd"].as_str().unwrap().ends_with(&alpha_src), "{a:#}");
    // No space on beta: one was created for it, as Herdr labels a new space.
    let b = h.agent("bbbb").unwrap();
    let beta = h.space_of("beta").expect("a space for beta");
    assert_eq!(b["workspace_id"], beta.as_str());
    assert_ne!(beta, alpha);
    let beta_label = h
        .workspaces()
        .into_iter()
        .find(|w| w["workspace_id"] == beta.as_str())
        .unwrap()["label"]
        .clone();
    assert_eq!(beta_label, "beta");
    // Shown like any Claude: no display name, agent name, title, labels or pane label.
    for x in [&a, &b] {
        assert_eq!(x["agent"], "claude");
        for field in ["display_agent", "name", "title", "label"] {
            assert!(x.get(field).is_none(), "{field} set: {x:#}");
        }
        assert!(
            x.get("state_labels")
                .is_none_or(|l| l.as_object().is_none_or(|o| o.is_empty()))
        );
    }
    assert!(h.panes().iter().all(|p| p.get("label").is_none()));
    // The extras are tokens, for custom sidebar rows.
    assert_eq!(a["tokens"]["summary"], "Reading src/main.rs");
    assert_eq!(a["tokens"]["project"], "src");
    assert_eq!(a["tokens"]["claude_state"], "working");
    // The tab is named after the session and runs `claude attach <id>`.
    let tab = h.cli(&["tab", "get", a["tab_id"].as_str().unwrap()]);
    assert_eq!(tab["result"]["tab"]["label"], "fix auth");
    let pane = a["pane_id"].as_str().unwrap().to_string();
    h.wait_for("the attach", || {
        h.cli_text(&["pane", "read", &pane, "--source", "recent"])
            .contains("fake attach aaaa")
    });
    assert!(h.workspaces().iter().all(|w| w["label"] != "Claude"));

    // One finishes its turn, one goes away, a new one starts in alpha.
    h.sessions(&[
        session("aaaa", "fix auth", &alpha_src, "working", "idle"),
        session("cccc", "refactor", &h.dir("alpha"), "working", "busy"),
    ]);
    h.wait_for("the changes", || {
        let done = matches!(h.status_of("aaaa").as_deref(), Some("done" | "idle"));
        done && h.agent("bbbb").is_none() && h.status_of("cccc").as_deref() == Some("working")
    });
    assert_eq!(h.agent("cccc").unwrap()["workspace_id"], alpha.as_str());
    // alpha: its own shell and two sessions; beta keeps the shell it was made with.
    assert_eq!(h.panes_in(&alpha), 3);
    assert_eq!(h.panes_in(&beta), 1);

    // A renamed session renames its tab.
    h.sessions(&[
        session("aaaa", "fix auth properly", &alpha_src, "working", "idle"),
        session("cccc", "refactor", &h.dir("alpha"), "working", "busy"),
    ]);
    h.wait_for("the rename", || {
        let tab_id = h.agent("aaaa").unwrap()["tab_id"].as_str().unwrap().to_string();
        h.cli(&["tab", "get", &tab_id])["result"]["tab"]["label"] == "fix auth properly"
    });
    h.stop();
}

#[test]
fn a_closed_tab_stays_closed_until_the_open_action() {
    let Some(mut h) = Herdr::new() else { return };
    h.sessions(&[session("aaaa", "fix auth", &h.dir("alpha"), "working", "busy")]);
    h.start();
    h.wait_for("the session", || h.agent("aaaa").is_some());
    let pane = h.agent("aaaa").unwrap()["pane_id"].as_str().unwrap().to_string();
    h.cli(&["pane", "close", &pane]);
    // Several polls later it is still closed.
    std::thread::sleep(Duration::from_secs(3));
    assert!(h.agent("aaaa").is_none());

    let invoked = h.cli(&["plugin", "action", "invoke", &format!("{PLUGIN}.open")]);
    assert!(invoked.get("result").is_some(), "{invoked}");
    h.wait_for("the reopened session", || h.agent("aaaa").is_some());
    // One tab, in the space created for alpha the first time.
    let space = h.space_of("alpha").unwrap();
    assert_eq!(h.agent("aaaa").unwrap()["workspace_id"], space.as_str());
    assert_eq!(h.agents().len(), 1);
    h.stop();
}

#[test]
fn a_herdr_restart_brings_the_sessions_back_without_leftovers() {
    let Some(mut h) = Herdr::new() else { return };
    h.sessions(&[
        session("aaaa", "fix auth", &h.dir("alpha"), "working", "busy"),
        session("bbbb", "write docs", &h.dir("alpha"), "done", "idle"),
    ]);
    h.start();
    h.wait_for("both sessions", || {
        h.agent("aaaa").is_some() && h.agent("bbbb").is_some()
    });
    let space = h.space_of("alpha").unwrap();
    assert_eq!(h.panes_in(&space), 3);
    h.stop();
    // The watcher went with the server.
    h.wait_for("the watcher to stop", || !h.watcher_running());

    // While Herdr is down, one session ends.
    h.sessions(&[session("aaaa", "fix auth", &h.dir("alpha"), "working", "busy")]);
    h.start();
    h.wait_for("the session after the restart", || {
        h.status_of("aaaa").as_deref() == Some("working") && h.agent("bbbb").is_none()
    });
    // Herdr restored the old tabs as shells; they were replaced or closed.
    h.wait_for("no leftover panes", || h.panes_in(&space) == 2);
    assert_eq!(h.agent("aaaa").unwrap()["workspace_id"], space.as_str());
    assert_eq!(h.workspaces().len(), 1);
    h.stop();
}

#[test]
fn disabling_the_plugin_stops_the_watcher() {
    let Some(mut h) = Herdr::new() else { return };
    h.start();
    h.wait_for("the watcher", || h.watcher_running());
    h.cli(&["plugin", "disable", PLUGIN]);
    h.wait_for("the watcher to stop", || !h.watcher_running());
    h.stop();
}
