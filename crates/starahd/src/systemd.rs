//! Starting an agent as a sandboxed transient systemd service, and waiting for it.

use std::os::fd::OwnedFd;

use zbus::zvariant::{Fd, OwnedObjectPath, Value};

#[zbus::proxy(
    interface = "org.freedesktop.systemd1.Manager",
    default_service = "org.freedesktop.systemd1",
    default_path = "/org/freedesktop/systemd1"
)]
trait Manager {
    fn start_transient_unit(
        &self,
        name: &str,
        mode: &str,
        properties: Vec<(&str, Value<'_>)>,
        aux: Vec<(&str, Vec<(&str, Value<'_>)>)>,
    ) -> zbus::Result<OwnedObjectPath>;
    fn get_unit(&self, name: &str) -> zbus::Result<OwnedObjectPath>;
    fn stop_unit(&self, name: &str, mode: &str) -> zbus::Result<OwnedObjectPath>;
    fn reset_failed_unit(&self, name: &str) -> zbus::Result<()>;
}

/// The sandbox every agent process runs in. The agent is its own system user; it
/// cannot gain privileges (no setuid, no sudo, no capabilities), sees the system
/// read-only, cannot see home directories, and writes only its workspace.
pub struct Launch<'a> {
    pub unit: &'a str,
    pub agent: &'a str,
    pub user: &'a str,
    pub workspace: &'a str,
    pub argv: &'a [String],
    pub path: &'a str,
    pub stdin: &'a OwnedFd,
    pub stdout: &'a OwnedFd,
    pub stderr: &'a OwnedFd,
}

/// The hardening every agent unit gets, whatever it runs.
fn sandbox<'a>(home: &'a str, writable: Vec<String>) -> Vec<(&'static str, Value<'a>)> {
    vec![
        ("NoNewPrivileges", Value::from(true)),
        ("RestrictSUIDSGID", Value::from(true)),
        ("CapabilityBoundingSet", Value::from(0u64)),
        ("AmbientCapabilities", Value::from(0u64)),
        ("ProtectSystem", Value::from("strict")),
        ("ProtectHome", Value::from(home)),
        ("ReadWritePaths", Value::from(writable)),
        ("PrivateTmp", Value::from(true)),
        ("PrivateDevices", Value::from(true)),
        ("ProtectKernelTunables", Value::from(true)),
        ("ProtectKernelModules", Value::from(true)),
        ("ProtectKernelLogs", Value::from(true)),
        ("ProtectControlGroups", Value::from(true)),
        ("ProtectClock", Value::from(true)),
        ("ProtectHostname", Value::from(true)),
        ("LockPersonality", Value::from(true)),
        ("RestrictRealtime", Value::from(true)),
    ]
}

/// An interactive agent: on the caller's terminal, in their project folder (or
/// home), mounted from `staging` (see session.rs) at `shared`.
pub struct Interactive<'a> {
    pub unit: &'a str,
    pub agent: &'a str,
    pub user: &'a str,
    pub workspace: &'a str,
    pub argv: &'a [String],
    pub path: &'a str,
    pub tty: &'a str,
    pub cwd: &'a str,
    pub staging: &'a str,
    pub shared: &'a str,
    pub env: &'a [String],
}

pub async fn start_interactive(conn: &zbus::Connection, l: &Interactive<'_>) -> zbus::Result<()> {
    let manager = ManagerProxy::new(conn).await?;
    let exec = vec![(l.argv[0].clone(), l.argv.to_vec(), false)];
    let mut env = vec![
        format!("STARAH_AGENT={}", l.agent),
        format!("PATH={}", l.path),
    ];
    env.extend(l.env.iter().cloned());
    if !l.env.iter().any(|v| v.starts_with("LANG=")) {
        env.push("LANG=C.UTF-8".into());
    }
    // (source, destination, ignore if missing, mount flags)
    let binds = vec![(l.staging.to_string(), l.shared.to_string(), false, 0u64)];
    let mut props: Vec<(&str, Value<'_>)> = vec![
        (
            "Description",
            Value::from(format!("Starah agent {} (interactive)", l.agent)),
        ),
        ("ExecStart", Value::from(exec)),
        ("User", Value::from(l.user)),
        ("WorkingDirectory", Value::from(l.cwd)),
        ("Environment", Value::from(env)),
        // systemd opens the terminal (as root) and makes it the agent's own.
        ("TTYPath", Value::from(l.tty)),
        ("StandardInput", Value::from("tty-force")),
        ("StandardOutput", Value::from("tty")),
        ("StandardError", Value::from("tty")),
        ("RemainAfterExit", Value::from(true)),
        ("BindPaths", Value::from(binds)),
    ];
    // Home folders are an empty tmpfs; only the shared folder is mounted into it.
    props.extend(sandbox("tmpfs", vec![l.workspace.to_string()]));
    manager
        .start_transient_unit(l.unit, "fail", props, Vec::new())
        .await?;
    Ok(())
}

/// Runs `starahd ARGS…` (this program) as root in a transient unit and waits for
/// it: the session steps starahd's own sandbox does not allow. True on success.
pub async fn run_root(conn: &zbus::Connection, unit: &str, args: &[String]) -> zbus::Result<bool> {
    let manager = ManagerProxy::new(conn).await?;
    let me = std::env::current_exe()
        .map_err(|e| zbus::Error::Failure(e.to_string()))?
        .to_string_lossy()
        .into_owned();
    let mut argv = vec![me.clone()];
    argv.extend(args.iter().cloned());
    let props: Vec<(&str, Value<'_>)> = vec![
        (
            "Description",
            Value::from("Starah: prepare or end an agent session"),
        ),
        ("ExecStart", Value::from(vec![(me, argv, false)])),
        ("Type", Value::from("oneshot")),
        ("RemainAfterExit", Value::from(true)),
        (
            "Environment",
            Value::from(vec![
                "PATH=/run/current-system/sw/bin:/run/wrappers/bin".to_string()
            ]),
        ),
    ];
    manager
        .start_transient_unit(unit, "fail", props, Vec::new())
        .await?;
    Ok(wait(conn, unit).await? == 0)
}

/// Returns when an interactive session is over: the unit is gone (its starter
/// collected it with Wait), or its starter left the bus (the unit is stopped
/// then), or the agent exited and nobody collected it within a minute.
pub async fn watch_session(conn: &zbus::Connection, unit: &str, starter: &str) {
    let Ok(manager) = ManagerProxy::new(conn).await else {
        return;
    };
    let Ok(dbus) = zbus::fdo::DBusProxy::new(conn).await else {
        return;
    };
    let mut exited_for = 0u32;
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let Ok(path) = manager.get_unit(unit).await else {
            return;
        };
        let starter_here = match zbus::names::BusName::try_from(starter) {
            Ok(name) => dbus.name_has_owner(name).await.unwrap_or(false),
            Err(_) => false,
        };
        let exited = match uncached(conn, &path, "org.freedesktop.systemd1.Service").await {
            Ok(service) => {
                service
                    .get_property::<u64>("ExecMainExitTimestampMonotonic")
                    .await
                    .unwrap_or(0)
                    != 0
            }
            Err(_) => true,
        };
        exited_for = if exited { exited_for + 1 } else { 0 };
        if !starter_here || exited_for > 120 {
            let _ = manager.stop_unit(unit, "replace").await;
            let _ = manager.reset_failed_unit(unit).await;
            return;
        }
    }
}

pub async fn start(conn: &zbus::Connection, l: &Launch<'_>) -> zbus::Result<()> {
    let manager = ManagerProxy::new(conn).await?;
    let exec = vec![(l.argv[0].clone(), l.argv.to_vec(), false)];
    let env = vec![
        format!("STARAH_AGENT={}", l.agent),
        format!("HOME={}", l.workspace),
        format!("PATH={}", l.path),
        "LANG=C.UTF-8".to_string(),
    ];
    let mut props: Vec<(&str, Value<'_>)> = vec![
        (
            "Description",
            Value::from(format!("Starah agent {}", l.agent)),
        ),
        ("ExecStart", Value::from(exec)),
        ("User", Value::from(l.user)),
        ("WorkingDirectory", Value::from(l.workspace)),
        ("Environment", Value::from(env)),
        ("StandardInputFileDescriptor", Value::Fd(Fd::from(l.stdin))),
        (
            "StandardOutputFileDescriptor",
            Value::Fd(Fd::from(l.stdout)),
        ),
        ("StandardErrorFileDescriptor", Value::Fd(Fd::from(l.stderr))),
        // Keeps the unit (and its exit status) until starahd collects it in Wait.
        ("RemainAfterExit", Value::from(true)),
    ];
    props.extend(sandbox("yes", vec![l.workspace.to_string()]));
    manager
        .start_transient_unit(l.unit, "fail", props, Vec::new())
        .await?;
    Ok(())
}

/// Waits until the unit's main process has exited; returns its exit status
/// (128 + signal if killed) and removes the unit.
pub async fn wait(conn: &zbus::Connection, unit: &str) -> zbus::Result<i32> {
    let manager = ManagerProxy::new(conn).await?;
    let path = manager.get_unit(unit).await?;
    let unit_proxy = uncached(conn, &path, "org.freedesktop.systemd1.Unit").await?;
    let service = uncached(conn, &path, "org.freedesktop.systemd1.Service").await?;
    // A fresh unit is "inactive" until its start job runs, so wait for the main
    // process's exit (or a failure to start it) rather than for a state.
    loop {
        let exited: u64 = service
            .get_property("ExecMainExitTimestampMonotonic")
            .await
            .unwrap_or(0);
        let active: String = unit_proxy
            .get_property("ActiveState")
            .await
            .unwrap_or_default();
        if exited != 0 || active == "failed" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let code: i32 = service.get_property("ExecMainCode").await.unwrap_or(0);
    let status: i32 = service.get_property("ExecMainStatus").await.unwrap_or(1);
    let _ = manager.stop_unit(unit, "replace").await;
    let _ = manager.reset_failed_unit(unit).await;
    // CLD_EXITED = 1: status is the exit code; otherwise it is a signal number.
    Ok(if code == 1 { status } else { 128 + status })
}

/// A proxy that reads properties from systemd each time instead of a cache.
async fn uncached<'a>(
    conn: &zbus::Connection,
    path: &'a OwnedObjectPath,
    interface: &'static str,
) -> zbus::Result<zbus::Proxy<'a>> {
    zbus::proxy::Builder::<zbus::Proxy<'a>>::new(conn)
        .destination("org.freedesktop.systemd1")?
        .path(path.as_ref())?
        .interface(interface)?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
}
