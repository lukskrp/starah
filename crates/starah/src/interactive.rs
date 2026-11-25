//! `starah run --interactive`: an agent on this terminal, as its own user.
//!
//! The client opens a pseudo-terminal and hands its name to starahd, which has
//! systemd open it for the agent's unit (as the agent's controlling terminal).
//! This terminal goes raw, and the client relays keys in and output out, and
//! the window size, until the agent exits. If the client goes away, starahd
//! stops the agent.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use crate::proxy::Broker1Proxy;

/// Restores the terminal's settings when dropped.
struct Raw(libc::termios);

impl Drop for Raw {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(0, libc::TCSANOW, &self.0);
        }
    }
}

fn last_error(what: &str) -> String {
    format!("{what}: {}", std::io::Error::last_os_error())
}

/// A new pseudo-terminal: (master, slave held open, slave path).
fn open_pty() -> Result<(OwnedFd, File, String), String> {
    unsafe {
        let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
        if master < 0 {
            return Err(last_error("posix_openpt"));
        }
        let master = OwnedFd::from_raw_fd(master);
        if libc::grantpt(master.as_raw_fd()) != 0 || libc::unlockpt(master.as_raw_fd()) != 0 {
            return Err(last_error("unlockpt"));
        }
        let mut buf = [0 as libc::c_char; 128];
        if libc::ptsname_r(master.as_raw_fd(), buf.as_mut_ptr(), buf.len()) != 0 {
            return Err(last_error("ptsname"));
        }
        let path = std::ffi::CStr::from_ptr(buf.as_ptr())
            .to_string_lossy()
            .into_owned();
        // Held open so reading the master waits for the agent instead of failing.
        let slave = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags_noctty()
            .open(&path)
            .map_err(|e| format!("{path}: {e}"))?;
        Ok((master, slave, path))
    }
}

trait NoCtty {
    fn custom_flags_noctty(&mut self) -> &mut Self;
}

impl NoCtty for std::fs::OpenOptions {
    fn custom_flags_noctty(&mut self) -> &mut Self {
        use std::os::unix::fs::OpenOptionsExt;
        self.custom_flags(libc::O_NOCTTY | libc::O_CLOEXEC)
    }
}

fn copy_size(master: i32) {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(0, libc::TIOCGWINSZ, &mut ws) == 0 {
            libc::ioctl(master, libc::TIOCSWINSZ, &ws);
        }
    }
}

/// Environment variables passed on (starahd checks them again).
fn passed_env() -> Vec<String> {
    std::env::vars()
        .filter(|(k, _)| {
            matches!(
                k.as_str(),
                "TERM" | "COLORTERM" | "LANG" | "NO_COLOR" | "TZ"
            ) || k.starts_with("LC_")
                || k.ends_with("_API_KEY")
                || k.ends_with("_BASE_URL")
                || k.ends_with("_API_BASE")
        })
        .map(|(k, v)| format!("{k}={v}"))
        .collect()
}

pub async fn run(broker: &Broker1Proxy<'_>, agent: &str, argv: &[String]) -> Result<i32, String> {
    if unsafe { libc::isatty(0) } != 1 {
        return Err("--interactive needs a terminal; use `starah run` without it".into());
    }
    let cwd = std::env::current_dir().map_err(|e| format!("current folder: {e}"))?;
    let (master, _slave, tty) = open_pty()?;
    copy_size(master.as_raw_fd());

    let unit = broker
        .run_interactive(agent, argv, &cwd.to_string_lossy(), &passed_env(), &tty)
        .await
        .map_err(|e| match e {
            zbus::Error::MethodError(_, Some(msg), _) => msg,
            other => other.to_string(),
        })?;

    // This terminal goes raw: every key goes to the agent (Ctrl-C included).
    let saved = unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(0, &mut t) != 0 {
            return Err(last_error("tcgetattr"));
        }
        let mut raw = t;
        libc::cfmakeraw(&mut raw);
        libc::tcsetattr(0, libc::TCSANOW, &raw);
        Raw(t)
    };

    let mut to_agent = File::from(master.try_clone().map_err(|e| e.to_string())?);
    let mut from_agent = File::from(master.try_clone().map_err(|e| e.to_string())?);
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 4096];
        while let Ok(n) = stdin.read(&mut buf) {
            if n == 0 || to_agent.write_all(&buf[..n]).is_err() {
                break;
            }
        }
    });
    std::thread::spawn(move || {
        let mut stdout = std::io::stdout();
        let mut buf = [0u8; 4096];
        while let Ok(n) = from_agent.read(&mut buf) {
            if n == 0 || stdout.write_all(&buf[..n]).is_err() {
                break;
            }
            let _ = stdout.flush();
        }
    });

    let mut winch = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change())
        .map_err(|e| e.to_string())?;
    let waited = broker.wait(&unit);
    tokio::pin!(waited);
    let status = loop {
        tokio::select! {
            status = &mut waited => break status,
            _ = winch.recv() => copy_size(master.as_raw_fd()),
        }
    };
    // Let the last output through before the terminal is restored.
    std::thread::sleep(std::time::Duration::from_millis(100));
    drop(saved);
    status.map_err(|e| e.to_string())
}
