//! End to end: a real `herdr server`, isolated by its own XDG dirs, with this
//! plugin linked and a fake `claude` whose session list a test rewrites.
//! Skipped when `herdr` is not on PATH.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const PLUGIN: &str = "kamilpociot.claude-agents";
const WAIT: Duration = Duration::from_secs(20);

struct Herdr {
    root: PathBuf,
    server: Option<Child>,
}

impl Herdr {
    /// A fresh isolated Herdr with the plugin linked, not started yet.
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
        for d in ["config", "state", "data", "cache", "claude/sessions", "claude/jobs"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
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
        let log = std::fs::File::create(self.root.join("server.log")).unwrap();
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

    fn agent(&self, name: &str) -> Option<Value> {
        self.agents().into_iter().find(|a| a["display_agent"] == name)
    }

    fn panes(&self) -> Vec<Value> {
        let v = self.cli(&["pane", "list"]);
        v["result"]["panes"].as_array().cloned().unwrap_or_default()
    }

    fn wait_for(&self, what: &str, mut ok: impl FnMut() -> bool) {
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline {
            if ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let watcher = self.panes().into_iter().find(|p| p["label"] == "Claude sessions");
        let screen = watcher
            .and_then(|p| p["pane_id"].as_str().map(String::from))
            .map(|id| self.cli_text(&["pane", "read", &id, "--source", "recent"]))
            .unwrap_or_default();
        panic!(
            "timed out waiting for {what}\nagents: {:#?}\npanes: {:#?}\nlog: {}\nsessions tab:\n{screen}",
            self.agents(),
            self.panes(),
            self.cli_text(&["plugin", "log", "list", "--plugin", PLUGIN])
        );
    }

    fn status_of(&self, name: &str) -> Option<String> {
        self.agent(name)
            .and_then(|a| a["agent_status"].as_str().map(String::from))
    }
}

impl Drop for Herdr {
    fn drop(&mut self) {
        self.stop();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn session(id: &str, name: &str, state: &str, status: &str) -> Value {
    json!({"pid": 1, "id": id, "cwd": format!("/r/{name}-repo"), "kind": "background",
           "sessionId": format!("{id}-uuid"), "name": name, "state": state, "status": status})
}

fn workspace_of(h: &Herdr, label: &str) -> Option<String> {
    let v = h.cli(&["workspace", "list"]);
    v["result"]["workspaces"]
        .as_array()?
        .iter()
        .find(|w| w["label"] == label)
        .and_then(|w| w["workspace_id"].as_str().map(String::from))
}

#[test]
fn background_sessions_show_up_as_agents_and_follow_changes() {
    let Some(mut h) = Herdr::new() else { return };
    std::fs::create_dir_all(h.root.join("claude/jobs/aaaa")).unwrap();
    std::fs::write(
        h.root.join("claude/jobs/aaaa/state.json"),
        r#"{"state":"working","detail":"Reading src/main.rs"}"#,
    )
    .unwrap();
    h.sessions(&[
        session("aaaa", "fix auth", "working", "busy"),
        session("bbbb", "write docs", "working", "waiting"),
    ]);
    h.start();

    // The startup hook opened the sessions tab in a "Claude" workspace, which
    // opened a tab per session; Herdr lists them as agents with their state.
    h.wait_for("both sessions as agents", || {
        h.status_of("fix auth").as_deref() == Some("working") && h.status_of("write docs").as_deref() == Some("blocked")
    });
    let ws = workspace_of(&h, "Claude").expect("a Claude workspace");
    let a = h.agent("fix auth").unwrap();
    assert_eq!(a["workspace_id"], ws.as_str());
    assert_eq!(a["agent"], "claude");
    assert_eq!(a["name"], "fix-auth");
    assert_eq!(a["title"], "fix auth-repo");
    assert_eq!(a["tokens"]["summary"], "Reading src/main.rs");
    assert_eq!(a["tokens"]["claude_session"], "aaaa");
    assert_eq!(a["tokens"]["claude_state"], "working");
    // The tab is named after the session; the pane runs `claude attach <id>`.
    let tab = h.cli(&["tab", "get", a["tab_id"].as_str().unwrap()]);
    assert_eq!(tab["result"]["tab"]["label"], "fix auth");
    let pane = a["pane_id"].as_str().unwrap().to_string();
    h.wait_for("the attach", || {
        h.cli_text(&["pane", "read", &pane, "--source", "recent"])
            .contains("fake attach aaaa")
    });
    // Sessions tab plus one per session: the new workspace's shell is gone.
    let in_ws: Vec<_> = h
        .panes()
        .into_iter()
        .filter(|p| p["workspace_id"] == ws.as_str())
        .collect();
    assert_eq!(in_ws.len(), 3, "{in_ws:#?}");
    assert!(in_ws.iter().any(|p| p["label"] == "Claude sessions"));

    // One finishes its turn, one goes away, a new one starts.
    h.sessions(&[
        session("aaaa", "fix auth", "working", "idle"),
        session("cccc", "refactor", "working", "busy"),
    ]);
    h.wait_for("the changes", || {
        let done = matches!(h.status_of("fix auth").as_deref(), Some("done" | "idle"));
        done && h.agent("write docs").is_none() && h.status_of("refactor").as_deref() == Some("working")
    });
    let in_ws = h
        .panes()
        .into_iter()
        .filter(|p| p["workspace_id"] == ws.as_str())
        .count();
    assert_eq!(in_ws, 3);

    // A renamed session renames its agent and tab.
    h.sessions(&[
        session("aaaa", "fix auth properly", "working", "idle"),
        session("cccc", "refactor", "working", "busy"),
    ]);
    h.wait_for("the rename", || h.agent("fix auth properly").is_some());
    let tab_id = h.agent("fix auth properly").unwrap()["tab_id"]
        .as_str()
        .unwrap()
        .to_string();
    let tab = h.cli(&["tab", "get", &tab_id]);
    assert_eq!(tab["result"]["tab"]["label"], "fix auth properly");
    h.stop();
}

#[test]
fn a_closed_tab_stays_closed_until_the_open_action() {
    let Some(mut h) = Herdr::new() else { return };
    let mut s = session("aaaa", "fix auth", "working", "busy");
    s["cwd"] = json!(h.root.join("claude").display().to_string());
    h.sessions(&[s]);
    h.start();
    h.wait_for("the session", || h.agent("fix auth").is_some());
    // The tab opens in the session's folder.
    let cwd = h.agent("fix auth").unwrap()["cwd"].as_str().unwrap().to_string();
    let want = h.root.join("claude");
    assert!(cwd.ends_with(want.to_str().unwrap()), "{cwd}");
    let pane = h.agent("fix auth").unwrap()["pane_id"].as_str().unwrap().to_string();
    h.cli(&["pane", "close", &pane]);
    // Several polls later it is still closed.
    std::thread::sleep(Duration::from_secs(3));
    assert!(h.agent("fix auth").is_none());

    let invoked = h.cli(&["plugin", "action", "invoke", &format!("{PLUGIN}.open")]);
    assert!(invoked.get("result").is_some(), "{invoked}");
    h.wait_for("the reopened session", || h.agent("fix auth").is_some());
    // Only one sessions tab.
    let watchers = h
        .panes()
        .into_iter()
        .filter(|p| p["label"] == "Claude sessions")
        .count();
    assert_eq!(watchers, 1);
    h.stop();
}

#[test]
fn a_herdr_restart_brings_the_sessions_back_without_leftovers() {
    let Some(mut h) = Herdr::new() else { return };
    h.sessions(&[
        session("aaaa", "fix auth", "working", "busy"),
        session("bbbb", "write docs", "done", "idle"),
    ]);
    h.start();
    h.wait_for("both sessions", || {
        h.agent("fix auth").is_some() && h.agent("write docs").is_some()
    });
    h.stop();

    // While Herdr is down, one session ends.
    h.sessions(&[session("aaaa", "fix auth", "working", "busy")]);
    h.start();
    h.wait_for("the session after the restart", || {
        h.status_of("fix auth").as_deref() == Some("working") && h.agent("write docs").is_none()
    });
    // Herdr restored the old tabs as shells; they were replaced or closed, and
    // the old sessions tab with them.
    h.wait_for("no leftover panes", || {
        let ws = workspace_of(&h, "Claude").unwrap();
        let panes: Vec<_> = h
            .panes()
            .into_iter()
            .filter(|p| p["workspace_id"] == ws.as_str())
            .collect();
        panes.len() == 2 && panes.iter().filter(|p| p["label"] == "Claude sessions").count() == 1
    });
    let claude_workspaces = h.cli(&["workspace", "list"])["result"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["label"] == "Claude")
        .count();
    assert_eq!(claude_workspaces, 1);
    h.stop();
}
