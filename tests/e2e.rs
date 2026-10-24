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
                // `left` stands for `←`: the real attach relaunches itself in
                // place as `claude agents`, the agent list, titled so; `open
                // <name>` opens a session there, titling the terminal with it.
                // `--bg` records where and how it was started; a test then adds
                // the session to the list, as Claude would.
                "#!/bin/sh\ncase \"$1\" in\n  \
                 --bg) echo \"$PWD|$*\" > '{started}'; echo 'fake started' ;;\n  \
                 agents) if [ \"$2\" = --json ]; then cat '{list}'; else \
                 printf '\\033]0;claude agents\\007'; echo 'fake agent list'; \
                 while read l; do case \"$l\" in 'open '*) n=${{l#open }}; \
                 printf '\\033]0;%s\\007' \"$n\"; echo \"fake viewing $n\" ;; esac; done; fi ;;\n  \
                 attach) if [ \"$2\" = keys ]; then printf '\\033[?1049h'; echo 'fake raw attach'; \
                 stty raw -echo; dd bs=64 count=1 2>/dev/null > '{keys}'; stty sane; printf '\\033[?1049l'; \
                 echo 'fake keys read'; while read l; do :; done; fi; echo \"fake attach $2\"; while read l; do [ \"$l\" = left ] && exec \"$0\" agents; \
                 echo \"got $l\"; done ;;\nesac\n",
                list = root.join("list.json").display(),
                started = root.join("bg-started").display(),
                keys = root.join("keys").display()
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
            // Panes, and the dialog's `$SHELL -ic`, use a predictable shell.
            .env("SHELL", "/bin/sh")
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
fn opening_another_session_from_the_agent_list_shows_its_own_tab() {
    let Some(mut h) = Herdr::new() else { return };
    h.sessions(&[
        session("aaaa", "fix auth", &h.dir("alpha"), "working", "busy"),
        session("bbbb", "write docs", &h.dir("alpha"), "working", "busy"),
    ]);
    h.start();
    h.wait_for("both sessions", || {
        h.agent("aaaa").is_some() && h.agent("bbbb").is_some()
    });
    let pane = h.agent("aaaa").unwrap()["pane_id"].as_str().unwrap().to_string();
    let other = h.agent("bbbb").unwrap()["pane_id"].as_str().unwrap().to_string();
    let screen = || h.cli_text(&["pane", "read", &pane, "--source", "recent"]);
    h.wait_for("the attach", || screen().contains("fake attach aaaa"));

    // `←` opens the agent list in the tab, as in any terminal.
    h.cli(&["pane", "run", &pane, "left"]);
    h.wait_for("the agent list", || screen().contains("fake agent list"));
    // Opening this tab's own session there changes nothing.
    h.cli(&["pane", "run", &pane, "open fix auth"]);
    h.wait_for("its own session", || screen().contains("fake viewing fix auth"));
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(screen().matches("fake attach aaaa").count(), 1);

    // Opening another: its own tab is shown, and this one goes back to its session.
    h.cli(&["pane", "run", &pane, "open write docs"]);
    h.wait_for("the other session's tab", || {
        let focused = h.panes().into_iter().find(|p| p["focused"] == true);
        focused.is_some_and(|p| p["pane_id"] == other.as_str())
    });
    let deadline = Instant::now() + WAIT;
    while screen().matches("fake attach aaaa").count() < 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert_eq!(screen().matches("fake attach aaaa").count(), 2, "{}", screen());
    assert!(!screen().contains("still running in the background"));
    // Both stay in the sidebar, each in its own tab.
    assert_eq!(h.agent("aaaa").unwrap()["pane_id"], pane.as_str());
    assert_eq!(h.agent("bbbb").unwrap()["pane_id"], other.as_str());
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

#[test]
fn the_new_session_dialog_starts_one_in_a_folder_of_the_space() {
    let Some(mut h) = Herdr::new() else { return };
    h.start();
    let alpha = h.open_space("alpha");
    let pane = h
        .panes()
        .into_iter()
        .find(|p| p["workspace_id"] == alpha.as_str())
        .unwrap()["pane_id"]
        .as_str()
        .unwrap()
        .to_string();
    let screen = || h.cli_text(&["pane", "read", &pane, "--source", "visible"]);
    // The dialog as the action's popup runs it, here in an ordinary pane so
    // the test can type into it and read it (a popup is outside the API).
    let state = h.root.join("state/herdr/plugins").join(PLUGIN);
    let config = h.cli_text(&["plugin", "config-dir", PLUGIN]).trim().to_string();
    let run = format!(
        "CLAUDE_AGENTS_BASE='{}' CLAUDE_AGENTS_SPACE=alpha HERDR_PLUGIN_STATE_DIR='{}' HERDR_PLUGIN_CONFIG_DIR='{}' '{}' dialog",
        h.dir("alpha"),
        state.display(),
        config,
        env!("CARGO_BIN_EXE_herdr-claude-agents")
    );
    h.cli(&["pane", "run", &pane, &run]);
    h.wait_for("the dialog", || screen().contains("New Claude background session"));
    assert!(screen().contains("in space alpha"));
    assert!(screen().contains("src/"), "{}", screen());

    // A folder of the space, completed with Tab; a command of your own and a prompt.
    h.cli(&["pane", "send-text", &pane, "sr"]);
    h.cli(&["pane", "send-keys", &pane, "tab"]);
    h.wait_for("the completed folder", || screen().contains("Folder   src/"));
    h.cli(&["pane", "send-keys", &pane, "down", "ctrl+u"]);
    let fake = h.root.join("claude-fake").display().to_string();
    h.cli(&["pane", "send-text", &pane, &format!("{fake} --bg --model opus")]);
    // (↑↓ in the command field walk recent commands; Tab moves on.)
    h.cli(&["pane", "send-keys", &pane, "tab"]);
    h.cli(&["pane", "send-text", &pane, "fix it's login"]);
    h.cli(&["pane", "send-keys", &pane, "enter"]);

    // It ran in that folder, through the shell, with the prompt as one argument.
    let started = h.root.join("bg-started");
    let deadline = Instant::now() + WAIT;
    while !started.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(started.exists(), "the command didn't run:\n{}", screen());
    let alpha_src = std::fs::canonicalize(h.root.join("repos/alpha/src")).unwrap();
    assert_eq!(
        std::fs::read_to_string(&started).unwrap().trim(),
        format!("{}|--bg --model opus fix it's login", alpha_src.display())
    );
    // The session appears; the watcher opens its tab in alpha's space, and the
    // dialog shows that tab.
    h.sessions(&[session(
        "nnnn",
        "fix login",
        alpha_src.to_str().unwrap(),
        "working",
        "busy",
    )]);
    h.wait_for("the new session's tab, shown", || {
        let Some(a) = h.agent("nnnn") else { return false };
        a["workspace_id"] == alpha.as_str()
            && h.panes()
                .into_iter()
                .any(|p| p["focused"] == true && p["pane_id"] == a["pane_id"])
    });
    // The dialog has finished and remembers the command for next time.
    // (A popup closes; in this pane the shell's prompt comes back.)
    h.wait_for("the dialog to end", || {
        let s = screen();
        let last = s.lines().map(str::trim_end).rfind(|l| !l.is_empty()).unwrap_or("");
        last.ends_with('$') || last.ends_with('#')
    });
    let history = std::fs::read_to_string(state.join("commands.json")).unwrap();
    assert!(history.contains("--bg --model opus"), "{history}");
    h.stop();
}

#[test]
fn shift_enter_in_a_session_tab_is_not_enter() {
    let Some(mut h) = Herdr::new() else { return };
    // `keys`: the fake attach switches to the alternate screen, as Claude's
    // does, and keeps the bytes of the next key it gets.
    h.sessions(&[session("keys", "type here", &h.dir("alpha"), "working", "busy")]);
    h.start();
    h.wait_for("the session", || h.agent("keys").is_some());
    let pane = h.agent("keys").unwrap()["pane_id"].as_str().unwrap().to_string();
    h.wait_for("the attach", || {
        h.cli_text(&["pane", "read", &pane, "--source", "visible"])
            .contains("fake raw attach")
    });
    // `claude attach` never asks for the kitty keyboard protocol; the tab
    // sets it on the alternate screen, so Herdr sends Shift-Enter as such
    // (a new line in Claude) instead of a plain Enter (send).
    h.cli(&["pane", "send-keys", &pane, "shift+enter"]);
    let keys = h.root.join("keys");
    h.wait_for("the key", || std::fs::metadata(&keys).is_ok_and(|m| m.len() > 0));
    assert_eq!(std::fs::read(&keys).unwrap(), b"\x1b[13;2u");
    // Back on the main screen, keys are plain again.
    h.wait_for("the main screen", || {
        h.cli_text(&["pane", "read", &pane, "--source", "visible"])
            .contains("fake keys read")
    });
    h.stop();
}
