//! "Claude: new background session": a dialog that starts a background
//! session in a folder of the current space, with `claude --bg` or your own
//! command (an alias such as `po` works: it runs in your interactive shell).
//! The watcher then gives the session its tab, and the dialog shows it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::{cursor, execute, queue, style, terminal};

use crate::claude;
use crate::herdr::Herdr;
use crate::plugin::{self, Context};
use crate::sync::Saved;

/// The folder names are relative to (the current space's folder).
pub const BASE_ENV: &str = "CLAUDE_AGENTS_BASE";
/// The current space's name, for the dialog's title.
pub const SPACE_ENV: &str = "CLAUDE_AGENTS_SPACE";
/// Commands remembered for ↑/↓.
const HISTORY: usize = 10;
/// How long to wait for the new session, and then for its tab.
const NEW_SESSION: Duration = Duration::from_secs(20);
const NEW_TAB: Duration = Duration::from_secs(10);
/// Suggestions shown under the folder.
const SUGGESTIONS: usize = 8;

/// The `new` action: opens the dialog as a popup over the current space,
/// telling it the space's folder and name.
pub fn open(ctx: &Context) -> Result<()> {
    let (base, space) = space_of_context(std::env::var("HERDR_PLUGIN_CONTEXT_JSON").ok().as_deref());
    let base = base.unwrap_or_else(claude::home).display().to_string();
    let space = space.unwrap_or_default();
    let env = [(BASE_ENV, base.as_str()), (SPACE_ENV, space.as_str())];
    Herdr::from_env().open_popup(&ctx.plugin_id, plugin::NEW, &env)
}

/// The space's folder and name from Herdr's plugin context.
fn space_of_context(json: Option<&str>) -> (Option<PathBuf>, Option<String>) {
    let Some(v) = json.and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok()) else {
        return (None, None);
    };
    let text = |k: &str| v[k].as_str().filter(|s| !s.is_empty()).map(String::from);
    (
        text("workspace_cwd")
            .or_else(|| text("focused_pane_cwd"))
            .map(PathBuf::from),
        text("workspace_label"),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Folder,
    Command,
    Prompt,
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Continue,
    Cancel,
    Submit,
}

/// The dialog's state, apart from the terminal.
#[derive(Debug)]
struct Form {
    base: PathBuf,
    space: String,
    folder: String,
    command: String,
    prompt: String,
    field: Field,
    /// Most recent first; ↑/↓ in the command field walk it.
    history: Vec<String>,
    at: Option<usize>,
    error: Option<String>,
}

impl Form {
    fn new(base: PathBuf, space: String, history: Vec<String>, default: String) -> Form {
        Form {
            command: history.first().cloned().unwrap_or(default),
            base,
            space,
            folder: String::new(),
            prompt: String::new(),
            field: Field::Folder,
            history,
            at: None,
            error: None,
        }
    }

    fn text(&mut self) -> &mut String {
        match self.field {
            Field::Folder => &mut self.folder,
            Field::Command => &mut self.command,
            Field::Prompt => &mut self.prompt,
        }
    }

    fn next(&mut self) {
        self.field = match self.field {
            Field::Folder => Field::Command,
            Field::Command => Field::Prompt,
            Field::Prompt => Field::Folder,
        };
    }

    fn previous(&mut self) {
        self.field = match self.field {
            Field::Folder => Field::Prompt,
            Field::Command => Field::Folder,
            Field::Prompt => Field::Command,
        };
    }

    /// One step through the command history: `back` towards older ones.
    fn recall(&mut self, back: bool) {
        if self.history.is_empty() {
            return;
        }
        let at = match (self.at, back) {
            (None, true) => 0,
            (None, false) => return,
            (Some(i), true) => (i + 1).min(self.history.len() - 1),
            (Some(0), false) => return,
            (Some(i), false) => i - 1,
        };
        self.at = Some(at);
        self.command = self.history[at].clone();
    }

    fn key(&mut self, key: KeyEvent) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        self.error = None;
        match key.code {
            KeyCode::Esc => return Outcome::Cancel,
            KeyCode::Char('c') if ctrl => return Outcome::Cancel,
            KeyCode::Enter => return Outcome::Submit,
            KeyCode::Tab if self.field == Field::Folder && self.complete() => {}
            KeyCode::Tab => self.next(),
            KeyCode::BackTab => self.previous(),
            KeyCode::Up if self.field == Field::Command => self.recall(true),
            KeyCode::Down if self.field == Field::Command => self.recall(false),
            KeyCode::Up => self.previous(),
            KeyCode::Down => self.next(),
            KeyCode::Backspace => {
                self.text().pop();
            }
            KeyCode::Char('u') if ctrl => self.text().clear(),
            KeyCode::Char('w') if ctrl => {
                let t = self.text();
                let kept = t
                    .trim_end_matches('/')
                    .trim_end_matches(|c: char| c != '/' && c != ' ')
                    .len();
                t.truncate(kept);
            }
            KeyCode::Char(c) if !ctrl => self.text().push(c),
            _ => {}
        }
        Outcome::Continue
    }

    /// The folder typed so far, split into the folder to list and the start
    /// of the name being typed.
    fn listing(&self) -> (PathBuf, String) {
        let (parent, prefix) = match self.folder.rfind('/') {
            Some(i) => (&self.folder[..=i], &self.folder[i + 1..]),
            None => ("", self.folder.as_str()),
        };
        (resolve(&self.base, parent), prefix.to_string())
    }

    /// Subfolders matching what is typed: by prefix, else ignoring case.
    /// Hidden ones only once a `.` is typed.
    fn suggestions(&self) -> Vec<String> {
        let (dir, prefix) = self.listing();
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| prefix.starts_with('.') || !n.starts_with('.'))
            .collect();
        names.sort();
        let exact: Vec<String> = names.iter().filter(|n| n.starts_with(&prefix)).cloned().collect();
        if !exact.is_empty() {
            return exact;
        }
        let lower = prefix.to_lowercase();
        names
            .into_iter()
            .filter(|n| n.to_lowercase().starts_with(&lower))
            .collect()
    }

    /// Completes the folder name as far as the matches agree; a single match
    /// gets its `/`. Whether anything changed.
    fn complete(&mut self) -> bool {
        let matches = self.suggestions();
        let (_, prefix) = self.listing();
        let stem = &self.folder[..self.folder.len() - prefix.len()];
        let done = match matches.as_slice() {
            [] => return false,
            [one] => format!("{stem}{one}/"),
            many => format!("{stem}{}", common_prefix(many)),
        };
        if done.len() <= self.folder.len() {
            return false;
        }
        self.folder = done;
        true
    }

    fn folder_path(&self) -> Result<PathBuf, String> {
        let path = resolve(&self.base, &self.folder);
        match std::fs::canonicalize(&path) {
            Ok(p) if p.is_dir() => Ok(p),
            Ok(_) => Err(format!("not a folder: {}", path.display())),
            Err(_) => Err(format!("no such folder: {}", path.display())),
        }
    }

    /// What the shell runs: the command, and the prompt as its last argument.
    fn shell_command(&self) -> String {
        let command = self.command.trim();
        match self.prompt.trim() {
            "" => command.to_string(),
            prompt => format!("{command} {}", sh_quote(prompt)),
        }
    }

    /// The dialog's lines, and where the cursor goes (column, row).
    fn render(&self, width: usize) -> (Vec<String>, (u16, u16)) {
        const DIM: &str = "\x1b[2m";
        const BOLD: &str = "\x1b[1m";
        const RESET: &str = "\x1b[0m";
        let space = if self.space.is_empty() {
            String::new()
        } else {
            format!(" {}", self.space)
        };
        let mut lines = vec![
            format!("{BOLD}New Claude background session{RESET}"),
            format!("{DIM}in space{space} · {}{RESET}", tilde(&self.base)),
            String::new(),
        ];
        let row = |label: &str, value: &str, hint: &str, active: bool| {
            let mark = if active { "›" } else { " " };
            let shown = if value.is_empty() && !hint.is_empty() {
                format!("{DIM}{hint}{RESET}")
            } else {
                value.to_string()
            };
            format!("{mark} {label:<8} {shown}")
        };
        lines.push(row(
            "Folder",
            &self.folder,
            "./ (the space's folder)",
            self.field == Field::Folder,
        ));
        let folder_row = lines.len() - 1;
        let suggestions = self.suggestions();
        let mut shown = String::new();
        for s in suggestions.iter().take(SUGGESTIONS) {
            if 11 + shown.len() + s.len() + 3 > width {
                break;
            }
            shown.push_str(&format!("{s}/  "));
        }
        if suggestions.len() > SUGGESTIONS {
            shown.push('…');
        }
        lines.push(format!("           {DIM}{shown}{RESET}"));
        lines.push(row("Command", &self.command, "", self.field == Field::Command));
        let command_row = lines.len() - 1;
        lines.push(row(
            "Prompt",
            &self.prompt,
            "(optional) the first task",
            self.field == Field::Prompt,
        ));
        let prompt_row = lines.len() - 1;
        lines.push(String::new());
        if let Some(e) = &self.error {
            lines.push(format!("\x1b[31m{e}{RESET}"));
        } else {
            lines.push(String::new());
        }
        lines.push(format!(
            "{DIM}Tab complete / next · ↑↓ field, recent commands · Enter start · Esc cancel{RESET}"
        ));
        let (row, len) = match self.field {
            Field::Folder => (folder_row, self.folder.chars().count()),
            Field::Command => (command_row, self.command.chars().count()),
            Field::Prompt => (prompt_row, self.prompt.chars().count()),
        };
        (lines, ((11 + len) as u16, row as u16))
    }
}

/// `~`, absolute, or relative to `base`; empty is `base` itself.
fn resolve(base: &Path, typed: &str) -> PathBuf {
    let typed = typed.trim();
    if typed == "~" {
        return claude::home();
    }
    if let Some(rest) = typed.strip_prefix("~/") {
        return claude::home().join(rest);
    }
    let p = Path::new(typed);
    if p.is_absolute() { p.to_path_buf() } else { base.join(p) }
}

fn common_prefix(names: &[String]) -> String {
    let first = &names[0];
    let mut end = first.len();
    for n in &names[1..] {
        end = end.min(first.bytes().zip(n.bytes()).take_while(|(a, b)| a == b).count());
    }
    while !first.is_char_boundary(end) {
        end -= 1;
    }
    first[..end].to_string()
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn tilde(p: &Path) -> String {
    let home = claude::home();
    match p.strip_prefix(&home) {
        Ok(rest) if !home.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => p.display().to_string(),
    }
}

fn history_path(ctx: &Context) -> PathBuf {
    ctx.state_dir.join("commands.json")
}

fn load_history(ctx: &Context) -> Vec<String> {
    std::fs::read_to_string(history_path(ctx))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// `command` first, without repeats, at most [`HISTORY`].
fn remember(history: &[String], command: &str) -> Vec<String> {
    let mut out = vec![command.to_string()];
    out.extend(history.iter().filter(|c| *c != command).cloned());
    out.truncate(HISTORY);
    out
}

/// The `dialog` pane: ask, start, show the new session's tab.
pub fn run(ctx: &Context) -> Result<()> {
    let config = ctx.config().map_err(anyhow::Error::msg)?;
    let base = std::env::var(BASE_ENV)
        .ok()
        .filter(|b| !b.is_empty())
        .map(PathBuf::from)
        .or_else(|| space_of_context(std::env::var("HERDR_PLUGIN_CONTEXT_JSON").ok().as_deref()).0)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(claude::home);
    let space = std::env::var(SPACE_ENV).unwrap_or_default();
    let history = load_history(ctx);
    let mut form = Form::new(base, space, history, format!("{} --bg", config.claude));
    if !ask(&mut form)? {
        return Ok(());
    }
    let folder = form.folder_path().map_err(anyhow::Error::msg)?;
    let command = form.shell_command();
    let _ = std::fs::create_dir_all(&ctx.state_dir);
    let _ = std::fs::write(
        history_path(ctx),
        serde_json::to_string(&remember(&form.history, form.command.trim()))?,
    );
    if let Err(e) = start(ctx, &config.claude, &folder, &command) {
        println!("\r\n\x1b[31m{e:#}\x1b[0m\r\nPress Enter to close.");
        let _ = std::io::stdin().read_line(&mut String::new());
    }
    Ok(())
}

/// The form until Enter (with a valid folder and a command) or Esc.
fn ask(form: &mut Form) -> Result<bool> {
    let mut out = std::io::stdout();
    terminal::enable_raw_mode()?;
    let result = (|| -> Result<bool> {
        loop {
            let width = terminal::size().map(|(w, _)| w as usize).unwrap_or(80);
            let (lines, (col, row)) = form.render(width);
            queue!(out, cursor::MoveTo(0, 0), terminal::Clear(terminal::ClearType::All))?;
            for (i, line) in lines.iter().enumerate() {
                queue!(out, cursor::MoveTo(1, i as u16), style::Print(line))?;
            }
            queue!(out, cursor::MoveTo(col + 1, row), cursor::Show)?;
            out.flush()?;
            let Event::Key(key) = event::read()? else { continue };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match form.key(key) {
                Outcome::Continue => {}
                Outcome::Cancel => return Ok(false),
                Outcome::Submit => match form.folder_path() {
                    Err(e) => form.error = Some(e),
                    Ok(_) if form.command.trim().is_empty() => form.error = Some("no command".into()),
                    Ok(_) => return Ok(true),
                },
            }
        }
    })();
    terminal::disable_raw_mode()?;
    execute!(out, terminal::Clear(terminal::ClearType::All), cursor::MoveTo(0, 0))?;
    result
}

/// Runs `command` in `folder` through the interactive shell (so aliases
/// work), its output on screen; then waits for the new session's tab and
/// shows it.
fn start(ctx: &Context, claude: &str, folder: &Path, command: &str) -> Result<()> {
    let before: Vec<String> = claude::list(claude)
        .map(|all| all.into_iter().map(|s| s.id).collect())
        .unwrap_or_default();
    println!("Starting \x1b[1m{command}\x1b[0m in {}…\n", tilde(folder));
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".into());
    let mut run = Command::new(&shell);
    run.args(["-ic", command]).current_dir(folder);
    // Not this popup's Herdr identity: a new session (or a daemon started
    // here) would carry it around.
    for (k, _) in std::env::vars() {
        if k.starts_with("HERDR_") || k.starts_with("CLAUDE_AGENTS_") {
            run.env_remove(k);
        }
    }
    let status = run.status()?;
    if !status.success() {
        bail!("`{command}` failed ({status})");
    }
    println!("\nWaiting for the new session…");
    let deadline = Instant::now() + NEW_SESSION;
    let session = loop {
        let new = claude::list(claude)
            .map(|all| all.into_iter().filter(|s| !before.contains(&s.id)).collect::<Vec<_>>())
            .unwrap_or_default();
        let here = new
            .iter()
            .find(|s| std::fs::canonicalize(&s.cwd).ok().as_deref() == Some(folder));
        if let Some(s) = here.or(new.first()) {
            break s.clone();
        }
        if Instant::now() > deadline {
            bail!("no new background session appeared");
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    let deadline = Instant::now() + NEW_TAB;
    loop {
        let pane = Saved::load(&ctx.state_path())
            .sessions
            .get(&session.id)
            .map(|t| t.pane.pane_id.clone());
        if let Some(pane) = pane {
            Herdr::from_env().focus_agent(&pane)?;
            return Ok(());
        }
        if Instant::now() > deadline {
            bail!(
                "{} started, but its tab didn't appear (is the plugin running?)",
                session.name
            );
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn typed(form: &mut Form, s: &str) {
        for c in s.chars() {
            form.key(key(KeyCode::Char(c)));
        }
    }

    /// base/{api,app,apps/web,docs,.git}
    fn tree() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hca-new-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        for d in ["api", "app", "apps/web", "docs", ".git"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        std::fs::write(dir.join("README.md"), "").unwrap();
        dir
    }

    fn form(base: &Path) -> Form {
        Form::new(base.to_path_buf(), "web".into(), vec![], "claude --bg".into())
    }

    #[test]
    fn folders_complete_like_a_shell() {
        let base = tree();
        let mut f = form(&base);
        // Everything but hidden folders and files.
        assert_eq!(f.suggestions(), ["api", "app", "apps", "docs"]);
        typed(&mut f, "a");
        assert_eq!(f.suggestions(), ["api", "app", "apps"]);
        // Several matches: as far as they agree ("a" + "p").
        assert_eq!(f.key(key(KeyCode::Tab)), Outcome::Continue);
        assert_eq!(f.folder, "ap");
        typed(&mut f, "ps");
        f.key(key(KeyCode::Tab));
        assert_eq!(f.folder, "apps/");
        assert_eq!(f.suggestions(), ["web"]);
        f.key(key(KeyCode::Tab));
        assert_eq!(f.folder, "apps/web/");
        // Nothing left to complete: Tab moves on.
        f.key(key(KeyCode::Tab));
        assert_eq!(f.field, Field::Command);
        // Case is ignored when nothing matches as typed; dots show hidden ones.
        let mut f = form(&base);
        typed(&mut f, "D");
        assert_eq!(f.suggestions(), ["docs"]);
        let mut f = form(&base);
        typed(&mut f, ".");
        assert_eq!(f.suggestions(), [".git"]);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn the_folder_is_relative_to_the_space() {
        let base = tree();
        let mut f = form(&base);
        assert_eq!(f.folder_path().unwrap(), std::fs::canonicalize(&base).unwrap());
        typed(&mut f, "apps/web");
        assert_eq!(
            f.folder_path().unwrap(),
            std::fs::canonicalize(base.join("apps/web")).unwrap()
        );
        f.folder = "nope".into();
        assert!(f.folder_path().unwrap_err().starts_with("no such folder"));
        f.folder = "README.md".into();
        assert!(f.folder_path().unwrap_err().starts_with("not a folder"));
        f.folder = base.join("api").display().to_string();
        assert_eq!(
            f.folder_path().unwrap(),
            std::fs::canonicalize(base.join("api")).unwrap()
        );
        assert_eq!(resolve(&base, "~"), claude::home());
        assert_eq!(resolve(&base, "~/x"), claude::home().join("x"));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn the_prompt_is_the_commands_last_argument() {
        let mut f = form(Path::new("/"));
        assert_eq!(f.shell_command(), "claude --bg");
        f.prompt = "fix the login, it's broken".into();
        assert_eq!(f.shell_command(), r"claude --bg 'fix the login, it'\''s broken'");
        f.command = "  po ".into();
        f.prompt = " ".into();
        assert_eq!(f.shell_command(), "po");
    }

    #[test]
    fn keys_move_between_fields_and_edit() {
        let mut f = form(Path::new("/nonexistent"));
        assert_eq!(f.field, Field::Folder);
        f.key(key(KeyCode::Down));
        assert_eq!(f.field, Field::Command);
        f.key(key(KeyCode::Tab));
        assert_eq!(f.field, Field::Prompt);
        f.key(key(KeyCode::Tab));
        assert_eq!(f.field, Field::Folder);
        f.key(key(KeyCode::BackTab));
        assert_eq!(f.field, Field::Prompt);
        typed(&mut f, "one two");
        f.key(ctrl('w'));
        assert_eq!(f.prompt, "one ");
        f.key(key(KeyCode::Backspace));
        assert_eq!(f.prompt, "one");
        f.key(ctrl('u'));
        assert_eq!(f.prompt, "");
        f.field = Field::Folder;
        typed(&mut f, "apps/web/");
        f.key(ctrl('w'));
        assert_eq!(f.folder, "apps/");
        assert_eq!(f.key(key(KeyCode::Enter)), Outcome::Submit);
        assert_eq!(f.key(key(KeyCode::Esc)), Outcome::Cancel);
        assert_eq!(f.key(ctrl('c')), Outcome::Cancel);
    }

    #[test]
    fn recent_commands_come_back() {
        let history = vec!["po".to_string(), "claude --bg --model opus".into()];
        let mut f = Form::new("/".into(), String::new(), history.clone(), "claude --bg".into());
        // The last one used is the default.
        assert_eq!(f.command, "po");
        f.field = Field::Command;
        f.key(key(KeyCode::Up));
        assert_eq!(f.command, "po");
        f.key(key(KeyCode::Up));
        assert_eq!(f.command, "claude --bg --model opus");
        f.key(key(KeyCode::Up));
        assert_eq!(f.command, "claude --bg --model opus");
        f.key(key(KeyCode::Down));
        assert_eq!(f.command, "po");
        // No history: `claude --bg`.
        assert_eq!(form(Path::new("/")).command, "claude --bg");
        // Remembering: first, once, at most ten.
        assert_eq!(
            remember(&history, "claude --bg --model opus"),
            ["claude --bg --model opus", "po"]
        );
        let many: Vec<String> = (0..12).map(|i| format!("c{i}")).collect();
        assert_eq!(remember(&many, "new").len(), HISTORY);
    }

    #[test]
    fn the_space_comes_from_herdrs_context() {
        let json =
            r#"{"workspace_id":"w1","workspace_label":"web","workspace_cwd":"/r/web","focused_pane_cwd":"/r/web/src"}"#;
        assert_eq!(
            space_of_context(Some(json)),
            (Some("/r/web".into()), Some("web".into()))
        );
        let json = r#"{"focused_pane_cwd":"/r/web/src"}"#;
        assert_eq!(space_of_context(Some(json)), (Some("/r/web/src".into()), None));
        assert_eq!(space_of_context(Some("nonsense")), (None, None));
        assert_eq!(space_of_context(None), (None, None));
    }

    #[test]
    fn the_dialog_shows_the_fields_and_puts_the_cursor_in_the_active_one() {
        let base = tree();
        let mut f = form(&base);
        typed(&mut f, "ap");
        let (lines, (col, row)) = f.render(100);
        let text = lines.join("\n");
        assert!(text.contains("New Claude background session"));
        assert!(text.contains("in space web"));
        assert!(text.contains("api/  app/  apps/"));
        assert!(text.contains("claude --bg"));
        assert!(text.contains("(optional) the first task"));
        assert_eq!((col, row), (13, 3));
        f.error = Some("no such folder: x".into());
        assert!(f.render(100).0.join("\n").contains("no such folder: x"));
        std::fs::remove_dir_all(&base).unwrap();
    }
}
