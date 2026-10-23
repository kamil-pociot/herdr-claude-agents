//! What Herdr hands a plugin command (its environment), the user's config,
//! and where the watcher keeps its state.

use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

pub const PLUGIN_ID: &str = "kamilpociot.claude-agents";
/// The `[[panes]]` entrypoint of a session tab in herdr-plugin.toml.
pub const SESSION: &str = "session";
/// How a session pane learns which session it shows.
pub const SESSION_ENV: &str = "CLAUDE_AGENTS_SESSION";
pub const NAME_ENV: &str = "CLAUDE_AGENTS_NAME";
/// Report sources: lifecycle state, and `$tokens`.
pub const SOURCE: &str = "claude-agents";
pub const TOKEN_SOURCE: &str = "claude-agents.tokens";

/// `config.toml` in the plugin's config dir (`herdr plugin config-dir
/// kamilpociot.claude-agents`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Claude Code's command: a name on Herdr's `PATH` or a full path.
    pub claude: String,
    /// Seconds between looks when no file change wakes the watcher.
    pub poll_seconds: u64,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            claude: "claude".into(),
            poll_seconds: 2,
        }
    }
}

impl Config {
    pub fn load(dir: Option<&Path>) -> Result<Config, String> {
        let Some(path) = dir.map(|d| d.join("config.toml")) else {
            return Ok(Config::default());
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => Config::parse(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(_) => Ok(Config::default()),
        }
    }

    pub fn parse(text: &str) -> Result<Config, String> {
        let mut c: Config = toml::from_str(text).map_err(|e| e.message().to_string())?;
        c.poll_seconds = c.poll_seconds.max(1);
        Ok(c)
    }

    pub fn poll(&self) -> Duration {
        Duration::from_secs(self.poll_seconds)
    }
}

/// The plugin's runtime context from Herdr's environment.
#[derive(Debug, Clone)]
pub struct Context {
    pub plugin_id: String,
    pub config_dir: Option<PathBuf>,
    pub state_dir: PathBuf,
    /// Which Herdr server: state is kept per server (named sessions differ).
    pub server_key: String,
}

impl Context {
    pub fn from_env() -> Context {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let state_dir = var("HERDR_PLUGIN_STATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::claude::home().join(".local/state/herdr-claude-agents"));
        Context {
            plugin_id: var("HERDR_PLUGIN_ID").unwrap_or_else(|| PLUGIN_ID.into()),
            config_dir: var("HERDR_PLUGIN_CONFIG_DIR").map(PathBuf::from),
            state_dir,
            server_key: server_key(&var("HERDR_SOCKET_PATH").unwrap_or_default()),
        }
    }

    pub fn config(&self) -> Result<Config, String> {
        Config::load(self.config_dir.as_deref())
    }

    pub fn state_path(&self) -> PathBuf {
        self.state_dir.join(format!("{}.json", self.server_key))
    }

    pub fn lock_path(&self) -> PathBuf {
        self.state_dir.join(format!("{}.lock", self.server_key))
    }

    /// Written by "Claude: show background sessions" for the watcher: open
    /// the tabs the user closed again.
    pub fn reopen_path(&self) -> PathBuf {
        self.state_dir.join(format!("{}.reopen", self.server_key))
    }
}

/// A short stable name for a socket path (FNV-1a).
pub fn server_key(socket: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in socket.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("server-{h:016x}")
}

/// An exclusive `flock` held for the life of the watcher, so one server never
/// gets two. The kernel drops it when the process dies.
pub struct Lock(#[allow(dead_code)] File);

impl Lock {
    pub fn try_acquire(path: &Path) -> std::io::Result<Option<Lock>> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = File::options().create(true).truncate(false).write(true).open(path)?;
        // SAFETY: flock on a file descriptor we own.
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        Ok((rc == 0).then_some(Lock(file)))
    }

    /// Waits up to `wait` for the lock: a watcher from before a Herdr restart
    /// may still be finishing its last look.
    pub fn acquire_within(path: &Path, wait: Duration) -> std::io::Result<Option<Lock>> {
        let deadline = std::time::Instant::now() + wait;
        loop {
            if let Some(lock) = Lock::try_acquire(path)? {
                return Ok(Some(lock));
            }
            if std::time::Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Whether a watcher holds the lock right now.
    #[cfg(test)]
    fn is_held(path: &Path) -> bool {
        matches!(Lock::try_acquire(path), Ok(None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_has_defaults_and_rejects_typos() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
        let c = Config::parse("claude = \"/opt/claude\"\npoll_seconds = 0\n").unwrap();
        assert_eq!(c.claude, "/opt/claude");
        assert_eq!(c.poll(), Duration::from_secs(1));
        assert!(Config::parse("claud = \"x\"").is_err());
        // v0.1's `workspace` is gone: a config still naming it says so.
        assert!(Config::parse("workspace = \"Claude\"").is_err());
        assert_eq!(Config::load(None).unwrap(), Config::default());
        assert_eq!(
            Config::load(Some(Path::new("/nonexistent"))).unwrap(),
            Config::default()
        );
    }

    #[test]
    fn each_server_gets_its_own_state() {
        assert_eq!(server_key("/a/herdr.sock"), server_key("/a/herdr.sock"));
        assert_ne!(server_key("/a/herdr.sock"), server_key("/b/herdr.sock"));
        assert!(server_key("").starts_with("server-"));
    }

    #[test]
    fn only_one_holder_of_the_lock() {
        let path = std::env::temp_dir().join(format!("hca-lock-{}/x.lock", std::process::id()));
        assert!(!Lock::is_held(&path));
        let lock = Lock::try_acquire(&path).unwrap();
        assert!(lock.is_some());
        assert!(Lock::is_held(&path));
        assert!(Lock::try_acquire(&path).unwrap().is_none());
        assert!(
            Lock::acquire_within(&path, Duration::from_millis(300))
                .unwrap()
                .is_none()
        );
        drop(lock);
        assert!(Lock::acquire_within(&path, Duration::ZERO).unwrap().is_some());
        assert!(!Lock::is_held(&path));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
