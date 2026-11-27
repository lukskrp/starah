//! Client side of org.starah.Broker1.

use zbus::zvariant::Fd;

#[zbus::proxy(
    interface = "org.starah.Broker1",
    default_service = "org.starah.Broker1",
    default_path = "/org/starah/Broker1"
)]
pub trait Broker1 {
    fn get_policy(&self) -> zbus::Result<String>;
    fn set_policy(&self, levels: &str) -> zbus::Result<()>;
    fn check(&self, action: &str, user: &str, detail: &str) -> zbus::Result<(bool, String)>;
    fn ask(&self, action: &str, description: &str) -> zbus::Result<(u64, String, String)>;
    fn status(&self, id: u64) -> zbus::Result<String>;
    fn requests(&self) -> zbus::Result<String>;
    fn decide(&self, id: u64, approve: bool, minutes: u32) -> zbus::Result<()>;
    fn run(
        &self,
        agent: &str,
        argv: &[String],
        stdin: Fd<'_>,
        stdout: Fd<'_>,
        stderr: Fd<'_>,
    ) -> zbus::Result<String>;
    fn run_interactive(
        &self,
        agent: &str,
        argv: &[String],
        cwd: &str,
        env: &[String],
        tty: &str,
    ) -> zbus::Result<String>;
    fn set_home_access(&self, agent: &str, allow: bool) -> zbus::Result<()>;
    fn wait(&self, unit: &str) -> zbus::Result<i32>;
    fn audit_log(&self, limit: u32) -> zbus::Result<String>;
    fn get_rules(&self) -> zbus::Result<String>;
    fn check_rules(&self, text: &str) -> zbus::Result<String>;
    fn set_rules(&self, text: &str) -> zbus::Result<String>;
    fn undo_rules(&self) -> zbus::Result<String>;
    fn try_rules(
        &self,
        agent: &str,
        action: &str,
        detail: &str,
        description: &str,
    ) -> zbus::Result<String>;

    #[zbus(signal)]
    fn request_added(&self, id: u64, request: &str) -> zbus::Result<()>;
    #[zbus(signal)]
    fn decided(&self, id: u64, approved: bool, status: &str) -> zbus::Result<()>;
}
