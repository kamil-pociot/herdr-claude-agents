//! Herdr's side, through its CLI (`HERDR_BIN_PATH`): the plugin API Herdr
//! documents. Every command prints `{"result": …}` or `{"error": …}`.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Result, anyhow, bail};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Pane {
    pub pane_id: String,
    pub tab_id: String,
    pub workspace_id: String,
    pub terminal_id: String,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Workspace {
    pub workspace_id: String,
    pub label: String,
}

/// What a session pane shows besides its lifecycle state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Display {
    pub agent_name: String,
    pub title: String,
    pub labels: Vec<(&'static str, &'static str)>,
    pub tokens: Vec<(&'static str, String)>,
}

pub struct Herdr {
    bin: String,
}

impl Herdr {
    /// The Herdr that launched this process; plain `herdr` outside a plugin.
    pub fn from_env() -> Herdr {
        let bin = std::env::var("HERDR_BIN_PATH").ok().filter(|b| !b.is_empty());
        Herdr {
            bin: bin.unwrap_or_else(|| "herdr".into()),
        }
    }

    fn run<S: AsRef<str>>(&self, args: &[S]) -> Result<Value> {
        let args: Vec<&str> = args.iter().map(|a| a.as_ref()).collect();
        let out = Command::new(&self.bin)
            .args(&args)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| anyhow!("cannot run {}: {e}", self.bin))?;
        // Reports print nothing when they succeed.
        if out.status.success() && out.stdout.iter().all(u8::is_ascii_whitespace) {
            return Ok(Value::Null);
        }
        let text = if out.stdout.is_empty() {
            &out.stderr
        } else {
            &out.stdout
        };
        parse_reply(&String::from_utf8_lossy(text), &args)
    }

    pub fn panes(&self) -> Result<Vec<Pane>> {
        let v = self.run(&["pane", "list"])?;
        Ok(serde_json::from_value(v["panes"].clone())?)
    }

    pub fn workspaces(&self) -> Result<Vec<Workspace>> {
        let v = self.run(&["workspace", "list"])?;
        Ok(serde_json::from_value(v["workspaces"].clone())?)
    }

    /// A new workspace without taking focus; returns it and its first pane.
    pub fn create_workspace(&self, label: &str) -> Result<(String, String)> {
        let v = self.run(&["workspace", "create", "--label", label, "--no-focus"])?;
        let id = v["workspace"]["workspace_id"].as_str().unwrap_or_default().to_string();
        let root = v["root_pane"]["pane_id"].as_str().unwrap_or_default().to_string();
        if id.is_empty() {
            bail!("workspace create returned no id");
        }
        Ok((id, root))
    }

    /// Opens one of the plugin's `[[panes]]` as a tab of `workspace`, in
    /// `cwd` (else the plugin's folder).
    pub fn open_tab(
        &self,
        plugin: &str,
        entrypoint: &str,
        workspace: &str,
        cwd: Option<&Path>,
        env: &[(&str, &str)],
        focus: bool,
    ) -> Result<Pane> {
        let mut args = vec![
            "plugin".to_string(),
            "pane".into(),
            "open".into(),
            "--plugin".into(),
            plugin.into(),
            "--entrypoint".into(),
            entrypoint.into(),
            "--placement".into(),
            "tab".into(),
            "--workspace".into(),
            workspace.into(),
            if focus { "--focus" } else { "--no-focus" }.into(),
        ];
        if let Some(cwd) = cwd.filter(|c| c.is_dir()) {
            args.push("--cwd".into());
            args.push(cwd.display().to_string());
        }
        for (k, v) in env {
            args.push("--env".into());
            args.push(format!("{k}={v}"));
        }
        let v = self.run(&args)?;
        Ok(serde_json::from_value(v["plugin_pane"]["pane"].clone())?)
    }

    pub fn close_pane(&self, pane: &str) -> Result<()> {
        self.run(&["pane", "close", pane]).map(drop)
    }

    pub fn rename_tab(&self, tab: &str, label: &str) -> Result<()> {
        self.run(&["tab", "rename", tab, label]).map(drop)
    }

    pub fn focus_tab(&self, tab: &str) -> Result<()> {
        self.run(&["tab", "focus", tab]).map(drop)
    }

    /// Takes over the pane's lifecycle state (Herdr stops reading the screen
    /// for it), which drives the sidebar, waits and notifications.
    pub fn report_agent(&self, pane: &str, source: &str, state: &str, session: Option<&str>, seq: u64) -> Result<()> {
        let seq = seq.to_string();
        let mut args = vec![
            "pane",
            "report-agent",
            pane,
            "--source",
            source,
            "--agent",
            "claude",
            "--state",
            state,
            "--seq",
            &seq,
        ];
        if let Some(id) = session {
            args.extend(["--agent-session-id", id]);
        }
        self.run(&args).map(drop)
    }

    /// Display-only extras: the name in the sidebar, a title, state labels
    /// and `$tokens` for custom sidebar rows.
    pub fn report_display(&self, pane: &str, source: &str, display: &Display, seq: u64) -> Result<()> {
        let mut args: Vec<String> = ["pane", "report-metadata", pane, "--source", source, "--agent", "claude"]
            .map(String::from)
            .into();
        args.extend(["--display-agent".into(), display.agent_name.clone()]);
        args.extend(["--title".into(), display.title.clone()]);
        // The labels sent replace the earlier ones.
        for (state, label) in &display.labels {
            args.extend(["--state-label".into(), format!("{state}={label}")]);
        }
        for (name, value) in &display.tokens {
            if value.is_empty() {
                args.extend(["--clear-token".into(), name.to_string()]);
            } else {
                args.extend(["--token".into(), format!("{name}={value}")]);
            }
        }
        args.extend(["--seq".into(), seq.to_string()]);
        self.run(&args).map(drop)
    }

    /// Gives the agent a name `herdr agent prompt <name>` accepts.
    pub fn rename_agent(&self, pane: &str, name: &str) -> Result<()> {
        self.run(&["agent", "rename", pane, name]).map(drop)
    }
}

fn parse_reply(text: &str, args: &[&str]) -> Result<Value> {
    // One JSON document, possibly after other output.
    let parsed = serde_json::from_str(text.trim()).ok().or_else(|| {
        let line = text.lines().rev().find(|l| l.trim_start().starts_with('{'))?;
        serde_json::from_str(line).ok()
    });
    let Some(mut v): Option<Value> = parsed else {
        bail!("herdr {}: {}", args.join(" "), text.trim());
    };
    if let Some(err) = v.get("error") {
        let code = err["code"].as_str().unwrap_or("error");
        let msg = err["message"].as_str().unwrap_or("");
        bail!("herdr {}: {code}: {msg}", args.first().copied().unwrap_or(""));
    }
    Ok(v["result"].take())
}

/// A name Herdr accepts for an agent (`[a-z][a-z0-9_-]{0,31}`), from a
/// session name such as "Fix auth (2)" → "fix-auth-2".
pub fn agent_slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-');
    let out = out.trim_start_matches(|c: char| !c.is_ascii_lowercase());
    let mut out: String = out.chars().take(32).collect();
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() { "claude".into() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_are_results_or_errors() {
        let ok = parse_reply(r#"{"id":"x","result":{"type":"pane_list","panes":[]}}"#, &["pane"]).unwrap();
        assert_eq!(ok["type"], "pane_list");
        let err = parse_reply(
            r#"{"id":"x","error":{"code":"not_found","message":"pane not found"}}"#,
            &["pane"],
        );
        assert_eq!(err.unwrap_err().to_string(), "herdr pane: not_found: pane not found");
        assert!(parse_reply("Usage: herdr …", &["pane"]).is_err());
    }

    #[test]
    fn panes_deserialize_from_pane_list() {
        let v = parse_reply(
            r#"{"result":{"panes":[{"agent_status":"unknown","pane_id":"w1:p2","tab_id":"w1:t2","workspace_id":"w1",
                "terminal_id":"term_1","label":"Claude session","revision":0}]}}"#,
            &[],
        )
        .unwrap();
        let panes: Vec<Pane> = serde_json::from_value(v["panes"].clone()).unwrap();
        assert_eq!(panes[0].pane_id, "w1:p2");
        assert_eq!(panes[0].label.as_deref(), Some("Claude session"));
    }

    #[test]
    fn agent_names_follow_herdr_rules() {
        assert_eq!(agent_slug("fix-login"), "fix-login");
        assert_eq!(agent_slug("cli-file-browser-tree (2)"), "cli-file-browser-tree-2");
        assert_eq!(agent_slug("Fix  AUTH!"), "fix-auth");
        assert_eq!(agent_slug("42 things"), "things");
        assert_eq!(agent_slug("éé"), "claude");
        assert_eq!(agent_slug(&"a".repeat(40)).len(), 32);
        assert_eq!(
            agent_slug("abcdefghijklmnopqrstuvwxyz01234 tail"),
            "abcdefghijklmnopqrstuvwxyz01234"
        );
    }
}
