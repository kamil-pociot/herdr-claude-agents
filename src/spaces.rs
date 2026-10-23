//! Which Herdr space (workspace) a session belongs in: the one already open
//! on its project, matched by the folders of the space's panes. Herdr's
//! workspaces carry no folder of their own; their panes do.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::herdr::Pane;

/// A session's project: the top of its git repository (a worktree's own
/// top for a worktree), or its folder when it isn't in one.
pub fn project_root(cwd: &Path) -> PathBuf {
    let top = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--show-toplevel"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
        .filter(|p| !p.as_os_str().is_empty());
    canonical(&top.unwrap_or_else(|| cwd.to_path_buf()))
}

/// The space for a session in `cwd` whose project is `root`: one with a pane
/// in that project, preferring a pane in the session's own folder; the first
/// such space otherwise. A project that is the home folder or `/` only
/// matches a pane exactly there, or every space would.
pub fn space_for(root: &Path, cwd: &Path, panes: &[Pane]) -> Option<String> {
    let cwd = canonical(cwd);
    let broad = root == Path::new("/") || root == canonical(&crate::claude::home());
    let folders: Vec<(&Pane, PathBuf)> = panes
        .iter()
        .filter_map(|p| p.cwd.as_deref().map(|c| (p, canonical(c))))
        .filter(|(_, c)| if broad { c == root } else { c.starts_with(root) })
        .collect();
    folders
        .iter()
        .find(|(_, c)| *c == cwd)
        .or(folders.first())
        .map(|(p, _)| p.workspace_id.clone())
}

/// `/tmp` and `/private/tmp` are one folder on macOS.
fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(ws: &str, cwd: &str) -> Pane {
        Pane {
            pane_id: format!("{ws}:p1"),
            workspace_id: ws.into(),
            cwd: Some(cwd.into()),
            ..Pane::default()
        }
    }

    #[test]
    fn a_session_goes_to_the_space_open_on_its_project() {
        let panes = [pane("w1", "/r/api"), pane("w2", "/r/web"), pane("w3", "/r/web/app")];
        let root = Path::new("/r/web");
        assert_eq!(space_for(root, Path::new("/r/web/src"), &panes).as_deref(), Some("w2"));
        // A pane in the session's own folder wins.
        assert_eq!(space_for(root, Path::new("/r/web/app"), &panes).as_deref(), Some("w3"));
        // No space on the project: none (a new one is created).
        assert_eq!(space_for(Path::new("/r/docs"), Path::new("/r/docs"), &panes), None);
        // A pane above the project is not on it.
        let above = [pane("w1", "/r")];
        assert_eq!(space_for(root, root, &above), None);
        // Panes without a folder are skipped.
        let none = [Pane {
            workspace_id: "w9".into(),
            ..Pane::default()
        }];
        assert_eq!(space_for(root, root, &none), None);
    }

    #[test]
    fn the_home_folder_only_matches_exactly() {
        let home = canonical(&crate::claude::home());
        let panes = [pane("w1", &home.join("r/api").display().to_string())];
        assert_eq!(space_for(&home, &home, &panes), None);
        let panes = [pane("w2", &home.display().to_string())];
        assert_eq!(space_for(&home, &home, &panes).as_deref(), Some("w2"));
        assert_eq!(space_for(Path::new("/"), Path::new("/"), &[pane("w1", "/r")]), None);
    }

    #[test]
    fn the_project_is_the_git_top_level() {
        let dir = std::env::temp_dir().join(format!("hca-root-{}", std::process::id()));
        let sub = dir.join("repo/src/deep");
        std::fs::create_dir_all(&sub).unwrap();
        let git = |args: &[&str]| Command::new("git").arg("-C").arg(dir.join("repo")).args(args).output();
        git(&["init", "-q"]).unwrap();
        assert_eq!(project_root(&sub), canonical(&dir.join("repo")));
        // Not a repository: the folder itself.
        let plain = dir.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(project_root(&plain), canonical(&plain));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
