//! The person's rules on disk: `<state>/rules.rhai` (root, 0600), its earlier
//! versions in `<state>/rules-history/`, and reloading when the file changes.
//!
//! The file can be changed two ways: `SetRules` (what `starah rules edit` and the
//! Edit Rules… button use: checked before it is written), or by hand as root
//! (`sudoedit /etc/starah/rules.rhai`). starahd looks at the file's modification
//! time before each decision and loads it again when it changed. A file that
//! doesn't compile is refused: the rules before stay in use, and the error is
//! shown by `GetRules` until the file is fixed.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use starah_policy::{Broker, Rules, RulesError};

/// Earlier versions kept for `UndoRules`.
const HISTORY: usize = 20;

#[derive(Debug, Clone, Default, Serialize)]
pub struct Status {
    /// "ok", "error" (the file doesn't compile; the last good rules are in use)
    /// or "none" (no file: the level table alone).
    pub state: String,
    /// The rules in use (empty when none).
    pub hash: String,
    /// Whether they define decide() (the example file doesn't).
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RulesError>,
    /// When the file last changed (Unix time).
    pub changed: u64,
}

pub struct RulesFile {
    pub path: PathBuf,
    history: PathBuf,
    mtime: Option<SystemTime>,
    pub status: Status,
}

pub enum Loaded {
    /// Nothing changed.
    Same,
    /// New rules are in use.
    New(String),
    /// The file changed but doesn't compile.
    Refused(RulesError),
}

fn secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The local time zone's offset from UTC now, in seconds.
pub fn utc_offset() -> i64 {
    // SAFETY: localtime_r only writes the struct we pass.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            0
        } else {
            tm.tm_gmtoff
        }
    }
}

impl RulesFile {
    pub fn new(state_dir: &Path) -> Self {
        RulesFile {
            path: state_dir.join("rules.rhai"),
            history: state_dir.join("rules-history"),
            mtime: None,
            status: Status {
                state: "none".into(),
                ..Default::default()
            },
        }
    }

    /// A new system starts with `initial` (commented examples that change nothing).
    pub fn seed(&self, initial: &str) -> std::io::Result<()> {
        if self.path.exists() {
            return Ok(());
        }
        write_private(&self.path, initial.as_bytes())
    }

    /// Loads the file again if it changed since the last look.
    pub fn refresh(&mut self, broker: &mut Broker) -> Loaded {
        broker.utc_offset = utc_offset();
        let mtime = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok();
        if mtime == self.mtime {
            return Loaded::Same;
        }
        self.mtime = mtime;
        let Some(mtime) = mtime else {
            broker.rules = None;
            self.status = Status {
                state: "none".into(),
                ..Default::default()
            };
            return Loaded::New(String::new());
        };
        let text = std::fs::read_to_string(&self.path).unwrap_or_default();
        match Rules::compile(&text) {
            Ok(rules) => {
                let hash = rules.hash.clone();
                self.status = Status {
                    state: "ok".into(),
                    hash: hash.clone(),
                    active: rules.has_decide(),
                    error: None,
                    changed: secs(mtime),
                };
                broker.rules = Some(rules);
                Loaded::New(hash)
            }
            Err(e) => {
                // The last good rules stay.
                self.status.state = "error".into();
                self.status.error = Some(e.clone());
                self.status.changed = secs(mtime);
                Loaded::Refused(e)
            }
        }
    }

    pub fn text(&self) -> String {
        std::fs::read_to_string(&self.path).unwrap_or_default()
    }

    /// Writes checked rules, keeping the version before in the history.
    pub fn install(&mut self, text: &str) -> std::io::Result<()> {
        let old = std::fs::read(&self.path).ok();
        if let Some(old) = old {
            std::fs::create_dir_all(&self.history)?;
            std::fs::set_permissions(&self.history, std::fs::Permissions::from_mode(0o700))?;
            let name = format!("{}.rhai", now_nanos());
            write_private(&self.history.join(name), &old)?;
            self.prune()?;
        }
        write_private(&self.path, text.as_bytes())
    }

    /// Puts the newest earlier version back (and drops it from the history).
    pub fn undo(&mut self) -> std::io::Result<Option<String>> {
        let Some(last) = self.versions()?.pop() else {
            return Ok(None);
        };
        let text = std::fs::read_to_string(&last)?;
        write_private(&self.path, text.as_bytes())?;
        std::fs::remove_file(last)?;
        Ok(Some(text))
    }

    fn versions(&self) -> std::io::Result<Vec<PathBuf>> {
        let mut v: Vec<PathBuf> = match std::fs::read_dir(&self.history) {
            Ok(d) => d.filter_map(|e| e.ok().map(|e| e.path())).collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };
        v.sort();
        Ok(v)
    }

    fn prune(&self) -> std::io::Result<()> {
        let v = self.versions()?;
        for old in v.iter().take(v.len().saturating_sub(HISTORY)) {
            std::fs::remove_file(old)?;
        }
        Ok(())
    }
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Atomically, readable by root only.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use starah_policy::{Config, Policy};

    fn broker() -> Broker {
        Broker::new(
            Config {
                agents: Default::default(),
                classes: Default::default(),
                grant_minutes: 10,
            },
            Policy::default(),
        )
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("starah-rules-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch_later(path: &Path) {
        // mtime granularity: make sure the next write is seen as a change.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let _ = path;
    }

    #[test]
    fn seed_load_refuse_install_undo() {
        let d = dir("cycle");
        let mut f = RulesFile::new(&d);
        let mut b = broker();
        f.seed(starah_policy::rules::EXAMPLE).unwrap();
        assert!(matches!(f.refresh(&mut b), Loaded::New(_)));
        assert_eq!(f.status.state, "ok");
        assert!(!f.status.active);
        assert!(matches!(f.refresh(&mut b), Loaded::Same));

        // A broken hand edit is refused; the rules before stay.
        touch_later(&f.path);
        std::fs::write(&f.path, "fn decide(req) {").unwrap();
        assert!(matches!(f.refresh(&mut b), Loaded::Refused(_)));
        assert_eq!(f.status.state, "error");
        assert!(b.rules.is_some());

        touch_later(&f.path);
        f.install("fn decide(req) { \"default\" }").unwrap();
        assert!(matches!(f.refresh(&mut b), Loaded::New(_)));
        assert!(f.status.active);
        assert_eq!(
            std::fs::metadata(&f.path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        // Undo: the version before the install (the broken one) comes back.
        assert_eq!(f.undo().unwrap().as_deref(), Some("fn decide(req) {"));
        assert_eq!(f.undo().unwrap(), None);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn no_file_no_rules() {
        let d = dir("none");
        let mut f = RulesFile::new(&d);
        let mut b = broker();
        assert!(matches!(f.refresh(&mut b), Loaded::Same));
        assert_eq!(f.status.state, "none");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
