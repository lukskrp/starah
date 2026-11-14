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

pub async fn start(conn: &zbus::Connection, l: &Launch<'_>) -> zbus::Result<()> {
    let manager = ManagerProxy::new(conn).await?;
    let exec = vec![(l.argv[0].clone(), l.argv.to_vec(), false)];
    let env = vec![
        format!("STARAH_AGENT={}", l.agent),
        format!("HOME={}", l.workspace),
        format!("PATH={}", l.path),
        "LANG=C.UTF-8".to_string(),
    ];
    let props: Vec<(&str, Value<'_>)> = vec![
        ("Description", Value::from(format!("Starah agent {}", l.agent))),
        ("ExecStart", Value::from(exec)),
        ("User", Value::from(l.user)),
        ("WorkingDirectory", Value::from(l.workspace)),
        ("Environment", Value::from(env)),
        ("StandardInputFileDescriptor", Value::Fd(Fd::from(l.stdin))),
        ("StandardOutputFileDescriptor", Value::Fd(Fd::from(l.stdout))),
        ("StandardErrorFileDescriptor", Value::Fd(Fd::from(l.stderr))),
        // Keeps the unit (and its exit status) until starahd collects it in Wait.
        ("RemainAfterExit", Value::from(true)),
        // The sandbox.
        ("NoNewPrivileges", Value::from(true)),
        ("RestrictSUIDSGID", Value::from(true)),
        ("CapabilityBoundingSet", Value::from(0u64)),
        ("AmbientCapabilities", Value::from(0u64)),
        ("ProtectSystem", Value::from("strict")),
        ("ProtectHome", Value::from("yes")),
        ("ReadWritePaths", Value::from(vec![l.workspace.to_string()])),
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
    ];
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
        let active: String = unit_proxy.get_property("ActiveState").await.unwrap_or_default();
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
