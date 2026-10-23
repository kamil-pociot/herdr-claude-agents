//! Herdr's side, through its CLI (`HERDR_BIN_PATH`): the plugin API Herdr
//! documents. Every command prints `{"result": …}` or `{"error": …}`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Result, anyhow, bail};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct Pane {
    pub pane_id: String,
    pub tab_id: String,
    pub workspace_id: String,
    pub terminal_id: String,
    #[serde(default)]
    pub label: Option<String>,
    /// The pane's own folder (not where its shell has `cd`'d to since).
    #[serde(default)]
    pub cwd: Option<PathBuf>,
}

pub struct Herdr {
    bin: String,
}

/// The error Herdr's CLI gives when no server listens on the socket.
const SERVER_GONE: &str = "server_not_running";

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

    /// A new workspace for `cwd`, labelled by Herdr as usual, without taking
    /// focus; returns its first pane.
    pub fn create_workspace(&self, cwd: &Path) -> Result<Pane> {
        let cwd = cwd.display().to_string();
        let v = self.run(&["workspace", "create", "--cwd", &cwd, "--no-focus"])?;
        Ok(serde_json::from_value(v["root_pane"].clone())?)
    }

    /// Opens one of the plugin's `[[panes]]` as a tab of `workspace`, in
    /// `cwd` (else the plugin's folder), without taking focus.
    pub fn open_tab(
        &self,
        plugin: &str,
        entrypoint: &str,
        workspace: &str,
        cwd: Option<&Path>,
        env: &[(&str, &str)],
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
            "--no-focus".into(),
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

    /// Plugin panes are labelled with their entrypoint's title; an ordinary
    /// pane has none.
    pub fn clear_pane_label(&self, pane: &str) -> Result<()> {
        self.run(&["pane", "rename", pane, "--clear"]).map(drop)
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

    /// `$tokens` for custom Agent sidebar rows; the default rows don't show
    /// them, so the agent looks like any other Claude. Empty values clear.
    pub fn report_tokens(&self, pane: &str, source: &str, tokens: &[(&str, String)], seq: u64) -> Result<()> {
        let mut args: Vec<String> = ["pane", "report-metadata", pane, "--source", source]
            .map(String::from)
            .into();
        for (name, value) in tokens {
            if value.is_empty() {
                args.extend(["--clear-token".into(), name.to_string()]);
            } else {
                args.extend(["--token".into(), format!("{name}={value}")]);
            }
        }
        args.extend(["--seq".into(), seq.to_string()]);
        self.run(&args).map(drop)
    }

    /// Whether `plugin` is linked or installed, and enabled.
    pub fn plugin_enabled(&self, plugin: &str) -> Result<bool> {
        let v = self.run(&["plugin", "list", "--json", "--plugin", plugin])?;
        let plugins = v["plugins"].as_array().cloned().unwrap_or_default();
        Ok(plugins
            .iter()
            .any(|p| p["plugin_id"] == plugin && p["enabled"].as_bool().unwrap_or(true)))
    }
}

/// Whether `err` says the Herdr server is gone (stopped, or restarting).
pub fn server_gone(err: &anyhow::Error) -> bool {
    err.to_string().contains(SERVER_GONE)
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
    fn a_stopped_server_is_recognised() {
        let err = parse_reply(
            r#"{"id":"cli:pane:list","error":{"code":"server_not_running","message":"no herdr server"}}"#,
            &["pane"],
        )
        .unwrap_err();
        assert!(server_gone(&err));
        assert!(!server_gone(&anyhow!("herdr pane: not_found: pane not found")));
    }

    #[test]
    fn panes_deserialize_from_pane_list() {
        let v = parse_reply(
            r#"{"result":{"panes":[{"agent_status":"unknown","pane_id":"w1:p2","tab_id":"w1:t2","workspace_id":"w1",
                "terminal_id":"term_1","label":"Claude session","cwd":"/r/api","revision":0}]}}"#,
            &[],
        )
        .unwrap();
        let panes: Vec<Pane> = serde_json::from_value(v["panes"].clone()).unwrap();
        assert_eq!(panes[0].pane_id, "w1:p2");
        assert_eq!(panes[0].label.as_deref(), Some("Claude session"));
        assert_eq!(panes[0].cwd.as_deref(), Some(Path::new("/r/api")));
    }
}
