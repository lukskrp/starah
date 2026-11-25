//! Interactive agent sessions (`starah run --interactive`): the parts that need root
//! outside starahd's own sandbox. starahd runs them as short transient units:
//!
//!   starahd --prepare-session CALLER_UID AGENT_USER CWD STAGING AGENT_HOME [LINK=TARGET…]
//!   starahd --end-session STAGING
//!
//! Preparing checks that CWD is a folder of the caller's, inside their home and not
//! the home itself; mounts it at STAGING through an idmapped bind mount, so the agent
//! (its own user) works on the files as if they were its own while they stay the
//! caller's on disk; and makes the agent's home for this person (AGENT_HOME) with the
//! links the agent's configuration asks for (its skills, say). The agent's unit binds
//! STAGING back at CWD and sees nothing else of the home.

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

/// An /etc/passwd entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
}

fn parse_passwd(text: &str, want: impl Fn(&str, u32) -> bool) -> Option<Account> {
    text.lines().find_map(|line| {
        let f: Vec<&str> = line.split(':').collect();
        if f.len() < 7 {
            return None;
        }
        let uid = f[2].parse().ok()?;
        let gid = f[3].parse().ok()?;
        want(f[0], uid).then(|| Account {
            name: f[0].to_string(),
            uid,
            gid,
            home: PathBuf::from(f[5]),
        })
    })
}

pub fn account_by_uid(uid: u32) -> Option<Account> {
    parse_passwd(&std::fs::read_to_string("/etc/passwd").ok()?, |_, u| {
        u == uid
    })
}

pub fn account_by_name(name: &str) -> Option<Account> {
    parse_passwd(&std::fs::read_to_string("/etc/passwd").ok()?, |n, _| {
        n == name
    })
}

/// Where a session's project folder is mounted for its unit.
pub const SESSIONS: &str = "/run/starah/sessions";

/// Environment variables an interactive agent may take from the person's
/// terminal: how to draw, the language, and model-provider settings.
pub fn allowed_env(name: &str) -> bool {
    matches!(name, "TERM" | "COLORTERM" | "LANG" | "NO_COLOR" | "TZ")
        || name.starts_with("LC_")
        || name.ends_with("_API_KEY")
        || name.ends_with("_BASE_URL")
        || name.ends_with("_API_BASE")
}

/// `/dev/pts/N`, nothing else.
pub fn is_pts(path: &str) -> bool {
    path.strip_prefix("/dev/pts/")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The project folder an agent may work in: CWD, resolved, a directory owned by
/// `caller`, inside their home and not the home itself.
pub fn check_project(cwd: &Path, caller: &Account) -> Result<PathBuf, String> {
    let real = cwd
        .canonicalize()
        .map_err(|e| format!("{}: {e}", cwd.display()))?;
    let home = caller
        .home
        .canonicalize()
        .map_err(|e| format!("{}: {e}", caller.home.display()))?;
    if real == home || !real.starts_with(&home) {
        return Err(format!(
            "{} is not a project folder inside your home: start the agent in one (not in the home itself)",
            real.display()
        ));
    }
    if real.components().any(|c| c.as_os_str() == ".snapshots") {
        return Err("not inside snapshots".into());
    }
    let meta = std::fs::metadata(&real).map_err(|e| format!("{}: {e}", real.display()))?;
    if !meta.is_dir() || meta.uid() != caller.uid {
        return Err(format!("{} is not a folder of yours", real.display()));
    }
    Ok(real)
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let out = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{cmd:?}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn chown(path: &Path, uid: u32, gid: u32) -> Result<(), String> {
    std::os::unix::fs::chown(path, Some(uid), Some(gid))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// `--prepare-session`, as root. Writes the reason for a refusal to STAGING.error.
pub fn prepare_main(args: &[String]) -> i32 {
    let staging = args.get(4).cloned().unwrap_or_default();
    match prepare(args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("starahd: {e}");
            let _ = std::fs::write(format!("{staging}.error"), &e);
            1
        }
    }
}

/// The folder a session shares, and where the agent sees it: the project folder,
/// or (when the person allowed this agent their home) the whole home.
pub fn shared_folder(cwd: &Path, caller: &Account, whole_home: bool) -> Result<PathBuf, String> {
    if !whole_home {
        return check_project(cwd, caller);
    }
    let home = caller
        .home
        .canonicalize()
        .map_err(|e| format!("{}: {e}", caller.home.display()))?;
    let real = cwd
        .canonicalize()
        .map_err(|e| format!("{}: {e}", cwd.display()))?;
    if !real.starts_with(&home) {
        return Err(format!("{} is not in your home", real.display()));
    }
    Ok(home)
}

fn prepare(args: &[String]) -> Result<(), String> {
    let [uid, agent_user, mode, cwd, staging, agent_home, links @ ..] = args else {
        return Err("usage: --prepare-session CALLER_UID AGENT_USER project|home CWD STAGING AGENT_HOME [LINK=TARGET…]".into());
    };
    let caller = uid
        .parse()
        .ok()
        .and_then(account_by_uid)
        .ok_or("unknown caller")?;
    let agent = account_by_name(agent_user).ok_or("unknown agent user")?;
    let whole_home = mode == "home";
    let project = shared_folder(Path::new(cwd), &caller, whole_home)?;
    if !whole_home && project != Path::new(cwd) {
        return Err(format!(
            "{cwd} must be given resolved ({})",
            project.display()
        ));
    }

    let staging = Path::new(staging);
    if !staging.starts_with(SESSIONS) {
        return Err("bad staging path".into());
    }
    std::fs::create_dir_all(SESSIONS).map_err(|e| e.to_string())?;
    std::fs::set_permissions(SESSIONS, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| e.to_string())?;
    std::fs::create_dir(staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    // The person's files appear as the agent's in this mount, and what the agent
    // writes is stored as the person's.
    let idmap = format!(
        "X-mount.idmap=u:{}:{}:1 g:{}:{}:1",
        caller.uid, agent.uid, caller.gid, agent.gid
    );
    run(Command::new("mount")
        .args(["--bind", "-o", &idmap])
        .arg(&project)
        .arg(staging))?;

    if whole_home {
        return Ok(()); // the agent uses the person's own home, logins and all
    }
    // The agent's home for this person: its logins and settings, theirs alone.
    let home = Path::new(agent_home);
    if let Some(parent) = home.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        chown(parent, agent.uid, agent.gid)?;
    }
    if !home.exists() {
        std::fs::create_dir(home).map_err(|e| format!("{}: {e}", home.display()))?;
    }
    chown(home, agent.uid, agent.gid)?;
    std::fs::set_permissions(home, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| e.to_string())?;
    for link in links {
        let Some((rel, target)) = link.split_once('=') else {
            continue;
        };
        if rel.starts_with('/') || rel.split('/').any(|p| p == "..") {
            continue;
        }
        let at = home.join(rel);
        if at.symlink_metadata().is_ok() {
            continue; // the agent's own file wins
        }
        if let Some(dir) = at.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            let mut d = dir.to_path_buf();
            while d.starts_with(home) && d != home {
                chown(&d, agent.uid, agent.gid)?;
                d.pop();
            }
        }
        std::os::unix::fs::symlink(target, &at).map_err(|e| format!("{}: {e}", at.display()))?;
        let _ = std::os::unix::fs::lchown(&at, Some(agent.uid), Some(agent.gid));
    }
    Ok(())
}

/// `--end-session`, as root: unmounts and removes the staging folder.
pub fn end_main(args: &[String]) -> i32 {
    let Some(staging) = args.first() else {
        return 2;
    };
    if !Path::new(staging).starts_with(SESSIONS) {
        return 2;
    }
    let _ = Command::new("umount").args(["--lazy", staging]).status();
    let _ = std::fs::remove_dir(staging);
    let _ = std::fs::remove_file(format!("{staging}.error"));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwd_lines() {
        let text = "root:x:0:0::/root:/bin/sh\nalice:x:1000:100:Alice:/home/alice:/bin/bash\n";
        let a = parse_passwd(text, |n, _| n == "alice").unwrap();
        assert_eq!(
            (a.uid, a.gid, a.home),
            (1000, 100, PathBuf::from("/home/alice"))
        );
        assert!(parse_passwd(text, |_, u| u == 5).is_none());
    }

    #[test]
    fn what_passes_from_the_terminal() {
        for ok in [
            "TERM",
            "COLORTERM",
            "LANG",
            "LC_ALL",
            "ANTHROPIC_API_KEY",
            "OPENAI_BASE_URL",
        ] {
            assert!(allowed_env(ok), "{ok}");
        }
        for no in [
            "PATH",
            "HOME",
            "SSH_AUTH_SOCK",
            "DBUS_SESSION_BUS_ADDRESS",
            "LD_PRELOAD",
            "DISPLAY",
        ] {
            assert!(!allowed_env(no), "{no}");
        }
        assert!(
            is_pts("/dev/pts/3")
                && !is_pts("/dev/pts/")
                && !is_pts("/dev/tty1")
                && !is_pts("/dev/pts/3/../0")
        );
    }

    #[test]
    fn project_must_be_a_folder_of_the_callers_in_their_home() {
        let tmp = std::env::temp_dir().join(format!("starah-test-{}", std::process::id()));
        let project = tmp.join("home/me/project");
        std::fs::create_dir_all(&project).unwrap();
        let me = Account {
            name: "me".into(),
            uid: std::fs::metadata(&tmp).unwrap().uid(),
            gid: 0,
            home: tmp.join("home/me"),
        };
        assert!(check_project(&project, &me).is_ok());
        assert!(
            check_project(&tmp.join("home/me"), &me).is_err(),
            "not the home itself"
        );
        assert!(check_project(&tmp, &me).is_err(), "not outside it");
        let other = Account {
            uid: me.uid + 1,
            ..me.clone()
        };
        assert!(
            check_project(&project, &other).is_err(),
            "not someone else's"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
